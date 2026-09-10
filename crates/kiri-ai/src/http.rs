use crate::progress::{Observer, Progress, silent};
use anyhow::{Context, Result, bail};
use reqwest::{Client, RequestBuilder, Response};
use serde::de::DeserializeOwned;
use std::{
    fmt,
    time::{Duration, SystemTime},
};

pub const RESPONSE_LIMIT: usize = 1024 * 1024;

#[derive(Clone, Debug)]
pub struct ProviderFailure {
    pub status: Option<u16>,
    pub message: String,
}
impl fmt::Display for ProviderFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for ProviderFailure {}

#[derive(Clone, Copy)]
pub struct RetryPolicy {
    pub attempts: usize,
    pub base_delay: Duration,
}
impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            attempts: 3,
            base_delay: Duration::from_millis(400),
        }
    }
}

pub fn client() -> Result<Client> {
    Ok(Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(90))
        .redirect(reqwest::redirect::Policy::none())
        .user_agent(concat!("kiri/", env!("CARGO_PKG_VERSION")))
        .build()?)
}

pub async fn send(request: RequestBuilder) -> Result<Response> {
    send_with_retry(
        request,
        RetryPolicy {
            attempts: 1,
            ..RetryPolicy::default()
        },
        &silent(),
    )
    .await
}

pub async fn send_with_retry(
    request: RequestBuilder,
    policy: RetryPolicy,
    observer: &Observer,
) -> Result<Response> {
    for attempt in 0..policy.attempts.max(1) {
        let result = request
            .try_clone()
            .context("Provider request cannot be retried")?
            .send()
            .await;
        let (failure, retry, retry_after) = match result {
            Ok(response) if response.status().is_success() => return Ok(response),
            Ok(response) => {
                let status = response.status().as_u16();
                let retry_after = response
                    .headers()
                    .get("retry-after")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| {
                        v.parse::<u64>().ok().map(Duration::from_secs).or_else(|| {
                            httpdate::parse_http_date(v)
                                .ok()?
                                .duration_since(SystemTime::now())
                                .ok()
                        })
                    });
                if matches!(status, 400 | 413 | 422) {
                    let body = bytes(response).await?;
                    if context_failure(status, &body) {
                        return Err(kiri_analysis::ContextOverflow.into());
                    }
                }
                (
                    failure(status),
                    matches!(status, 408 | 429 | 500 | 502 | 503 | 504),
                    retry_after,
                )
            }
            Err(error) => (
                ProviderFailure {
                    status: None,
                    message: if error.is_timeout() {
                        "Provider timed out. Your repository was not changed."
                    } else {
                        "Could not reach the provider. Check the connection and endpoint."
                    }
                    .into(),
                },
                error.is_timeout() || error.is_connect(),
                None,
            ),
        };
        if !retry
            || attempt + 1 >= policy.attempts
            || retry_after.is_some_and(|d| d > Duration::from_secs(120))
        {
            return Err(failure.into());
        }
        let delay = retry_after.unwrap_or(policy.base_delay.saturating_mul(1 << attempt.min(4)))
            + if policy.base_delay.is_zero() {
                Duration::ZERO
            } else {
                Duration::from_millis(fastrand::u64(0..200))
            };
        observer(Progress::Retrying {
            attempt: attempt + 1,
            delay_ms: delay.as_millis() as u64,
        });
        tokio::time::sleep(delay).await;
    }
    bail!("Provider retry budget exhausted")
}

fn context_failure(status: u16, body: &[u8]) -> bool {
    if status == 413 {
        return true;
    }
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(body) else {
        return false;
    };
    let code = value
        .pointer("/error/code")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    if matches!(code, "context_length_exceeded" | "max_tokens_exceeded") {
        return true;
    }
    let message = value
        .pointer("/error/message")
        .or_else(|| value.get("message"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_ascii_lowercase();
    message.contains("maximum context length")
        || message.contains("context window")
        || message.contains("prompt is too long")
        || (message.contains("input token count") && message.contains("exceeds"))
}

pub fn failure(status: u16) -> ProviderFailure {
    let hint = match status {
        401 | 403 => "Reconnect this provider or check its permissions.",
        429 => "Rate limit or quota reached. Wait or select another provider.",
        400 | 404 => "Check the model ID, deployment name, and endpoint in provider settings.",
        _ => "Try again later or select another provider.",
    };
    ProviderFailure {
        status: Some(status),
        message: format!("Provider returned HTTP {status}. {hint}"),
    }
}

pub fn require_success(response: Response) -> Result<Response> {
    if !response.status().is_success() {
        return Err(failure(response.status().as_u16()).into());
    }
    Ok(response)
}

pub async fn bytes(mut response: Response) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if bytes.len() + chunk.len() > RESPONSE_LIMIT {
            bail!("Provider response exceeded 1 MiB");
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

pub async fn json<T: DeserializeOwned>(response: Response) -> Result<T> {
    serde_json::from_slice(&bytes(response).await?)
        .map_err(|_| anyhow::anyhow!("Provider returned an invalid response"))
}
