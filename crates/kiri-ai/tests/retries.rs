use anyhow::Result;
use kiri_ai::{
    http::{self, RetryPolicy},
    progress::{Observer, Progress},
};
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

async fn server(
    statuses: Vec<u16>,
    retry_after: u64,
) -> Result<(String, tokio::task::JoinHandle<Result<usize>>)> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let url = format!("http://{}", listener.local_addr()?);
    let task = tokio::spawn(async move {
        let mut count = 0;
        for status in statuses {
            let (mut stream, _) = listener.accept().await?;
            let mut request = Vec::new();
            let mut buffer = [0; 1024];
            while !request.windows(4).any(|v| v == b"\r\n\r\n") {
                let n = stream.read(&mut buffer).await?;
                if n == 0 {
                    break;
                }
                request.extend_from_slice(&buffer[..n]);
            }
            count += 1;
            let body = "private-provider-echo";
            stream.write_all(format!("HTTP/1.1 {status} Test\r\nContent-Length: {}\r\nRetry-After: {retry_after}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await?;
        }
        Ok(count)
    });
    Ok((url, task))
}

fn observer(events: Arc<Mutex<Vec<Progress>>>) -> Observer {
    Arc::new(move |event| {
        if let Ok(mut events) = events.lock() {
            events.push(event);
        }
    })
}

#[tokio::test]
async fn transient_failures_retry_but_auth_failures_do_not() -> Result<()> {
    let client = http::client()?;
    let policy = RetryPolicy {
        attempts: 3,
        base_delay: Duration::ZERO,
    };
    let events = Arc::new(Mutex::new(Vec::new()));
    let (url, task) = server(vec![429, 503, 200], 0).await?;
    let response =
        http::send_with_retry(client.get(url), policy, &observer(events.clone())).await?;
    assert!(response.status().is_success());
    assert_eq!(task.await??, 3);
    assert_eq!(
        events
            .lock()
            .map_err(|_| anyhow::anyhow!("event mutex"))?
            .len(),
        2
    );
    let events = Arc::new(Mutex::new(Vec::new()));
    let (url, task) = server(vec![401], 0).await?;
    let error = http::send_with_retry(client.get(url), policy, &observer(events.clone()))
        .await
        .err()
        .ok_or_else(|| anyhow::anyhow!("expected auth failure"))?;
    assert!(error.to_string().contains("401"));
    assert!(!error.to_string().contains("private-provider-echo"));
    assert!(
        events
            .lock()
            .map_err(|_| anyhow::anyhow!("event mutex"))?
            .is_empty()
    );
    assert_eq!(task.await??, 1);
    Ok(())
}

#[tokio::test]
async fn retry_after_is_honored_and_backoff_can_be_cancelled() -> Result<()> {
    let client = http::client()?;
    let policy = RetryPolicy {
        attempts: 3,
        base_delay: Duration::ZERO,
    };
    let events = Arc::new(Mutex::new(Vec::new()));
    let (url, task) = server(vec![429, 200], 1).await?;
    let started = std::time::Instant::now();
    http::send_with_retry(client.get(url), policy, &observer(events.clone())).await?;
    assert!(started.elapsed() >= Duration::from_secs(1));
    assert_eq!(task.await??, 2);
    assert!(matches!(
        events
            .lock()
            .map_err(|_| anyhow::anyhow!("event mutex"))?
            .first(),
        Some(Progress::Retrying { delay_ms: 1000, .. })
    ));
    let events = Arc::new(Mutex::new(Vec::new()));
    let (url, task) = server(vec![429], 60).await?;
    assert!(
        tokio::time::timeout(
            Duration::from_millis(100),
            http::send_with_retry(client.get(url), policy, &observer(events))
        )
        .await
        .is_err()
    );
    assert_eq!(task.await??, 1);
    Ok(())
}
