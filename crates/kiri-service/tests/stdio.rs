use std::time::Duration;
use tokio::io::{AsyncWriteExt, duplex};

#[tokio::test]
async fn failed_output_stops_accepting_requests_without_waiting_for_input_eof() -> anyhow::Result<()>
{
    let (mut client, input) = duplex(1024);
    let (output, reader) = duplex(1024);
    drop(reader);
    let server = tokio::spawn(kiri_service::stdio::serve(input, output));
    let schema = kiri_service::protocol::schema()?;
    let request = serde_json::to_vec(&serde_json::json!({
        "id": 1,
        "command": { "method": "hello", "version": schema["version"], "schema_hash": schema["schema_hash"] }
    }))?;
    client.write_u32(request.len() as u32).await?;
    client.write_all(&request).await?;
    let result = tokio::time::timeout(Duration::from_secs(2), server).await;
    assert!(
        result.is_ok(),
        "The engine kept accepting input after its response writer died"
    );
    assert!(result??.is_err());
    Ok(())
}
