use anyhow::Result;
use kiri_ai::{
    config::{Credential, Provider, ProviderSettings, save_credential},
    provider::AiClient,
};
use kiri_core::storage::Store;
use serde_json::{Value, json};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

async fn server(
    body: Value,
    status: u16,
) -> Result<(String, tokio::task::JoinHandle<Result<String>>)> {
    server_raw(body.to_string(), status, "application/json").await
}

async fn server_raw(
    body: String,
    status: u16,
    content_type: &'static str,
) -> Result<(String, tokio::task::JoinHandle<Result<String>>)> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let endpoint = format!("http://{}", listener.local_addr()?);
    let task = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await?;
        let mut bytes = Vec::new();
        let mut buffer = [0; 4096];
        loop {
            let n = stream.read(&mut buffer).await?;
            if n == 0 {
                break;
            }
            bytes.extend_from_slice(&buffer[..n]);
            if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                let header = String::from_utf8_lossy(&bytes[..end]);
                let size: usize = header
                    .lines()
                    .find_map(|l| {
                        l.to_ascii_lowercase()
                            .strip_prefix("content-length: ")
                            .and_then(|v| v.parse().ok())
                    })
                    .unwrap_or(0);
                if bytes.len() >= end + 4 + size {
                    break;
                }
            }
        }
        stream.write_all(format!("HTTP/1.1 {status} OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await?;
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    });
    Ok((endpoint, task))
}

fn responses(text: &str) -> Value {
    json!({"id":"resp_test","object":"response","created_at":0,"status":"completed","model":"test-model","output":[{"type":"message","id":"msg_test","role":"assistant","status":"completed","content":[{"type":"output_text","text":text,"annotations":[]}]}],"usage":{"input_tokens":1,"output_tokens":1,"total_tokens":2},"parallel_tool_calls":false,"tools":[],"metadata":{}})
}

#[tokio::test]
async fn native_provider_transports_send_the_expected_protocol() -> Result<()> {
    for (provider, response, path, header) in [
        (
            Provider::Google,
            json!({"candidates":[{"content":{"role":"model","parts":[{"text":"ok"}]},"finishReason":"STOP"}]}),
            "/v1beta/models/test-model:generateContent",
            "x-goog-api-key",
        ),
        (
            Provider::Anthropic,
            json!({"id":"msg_test","type":"message","role":"assistant","model":"test-model","content":[{"type":"text","text":"ok"}],"stop_reason":"end_turn","usage":{"input_tokens":1,"output_tokens":1}}),
            "/v1/messages",
            "x-api-key",
        ),
        (
            Provider::Openai,
            responses("ok"),
            "/responses",
            "authorization",
        ),
        (
            Provider::Xai,
            responses("ok"),
            "/v1/responses",
            "authorization",
        ),
        (Provider::Azure, responses("ok"), "/responses", "api-key"),
        (
            Provider::Bedrock,
            json!({"output":{"message":{"content":[{"text":"ok"}]}}}),
            "/model/test-model/converse",
            "authorization",
        ),
    ] {
        let temp = tempfile::TempDir::new()?;
        let store = Store::at(temp.path());
        save_credential(
            &store,
            provider,
            Credential::ApiKey {
                key: "test-only-placeholder".into(),
            },
        )?;
        let (endpoint, captured) = server(response, 200).await?;
        let mut settings = ProviderSettings::defaults(provider);
        settings.model = "test-model".into();
        settings.endpoint = Some(endpoint);
        let client = AiClient::new(store, provider, settings)?;
        assert_eq!(client.complete("system", "data").await?, "ok");
        let request = captured.await??;
        assert!(
            request.starts_with(&format!("POST {path} ")),
            "{provider}: unexpected path"
        );
        assert!(request.to_ascii_lowercase().contains(&format!("{header}:")));
        assert!(request.contains("system"));
        assert!(request.contains("data"));
    }
    Ok(())
}

#[tokio::test]
async fn native_context_failures_reach_the_lossless_rechunker_without_echoing_bodies() -> Result<()>
{
    for (provider, body) in [
        (
            Provider::Google,
            json!({"error":{"code":400,"message":"The input token count exceeds the maximum number of tokens allowed. PRIVATE_PROVIDER_BODY"}}),
        ),
        (
            Provider::Openai,
            json!({"error":{"code":"context_length_exceeded","message":"PRIVATE_PROVIDER_BODY"}}),
        ),
        (
            Provider::Anthropic,
            json!({"error":{"type":"invalid_request_error","message":"prompt is too long: PRIVATE_PROVIDER_BODY"}}),
        ),
    ] {
        let temp = tempfile::tempdir()?;
        let store = Store::at(temp.path());
        save_credential(
            &store,
            provider,
            Credential::ApiKey {
                key: "test-only".into(),
            },
        )?;
        let (endpoint, captured) = server(body, 400).await?;
        let mut settings = ProviderSettings::defaults(provider);
        settings.endpoint = Some(endpoint);
        settings.model = "test-model".into();
        let client = AiClient::new(store, provider, settings)?;
        let error = match client.complete("system", "complete input").await {
            Ok(_) => anyhow::bail!("Context rejection was accepted"),
            Err(error) => error,
        };
        assert!(
            error
                .downcast_ref::<kiri_analysis::ContextOverflow>()
                .is_some(),
            "{provider}: {error}"
        );
        assert!(!error.to_string().contains("PRIVATE_PROVIDER_BODY"));
        captured.await??;
    }
    Ok(())
}

#[tokio::test]
async fn gemini_fast_and_deep_use_explicit_supported_thinking_settings() -> Result<()> {
    for (model, mode, field, expected) in [
        (
            "gemini-3.8-flash",
            kiri_analysis::AnalysisMode::Fast,
            "thinkingLevel",
            json!("low"),
        ),
        (
            "gemini-3.8-flash",
            kiri_analysis::AnalysisMode::Deep,
            "thinkingLevel",
            json!("high"),
        ),
        (
            "gemini-2.5-flash",
            kiri_analysis::AnalysisMode::Fast,
            "thinkingBudget",
            json!(1024),
        ),
        (
            "gemini-2.5-flash",
            kiri_analysis::AnalysisMode::Deep,
            "thinkingBudget",
            json!(4096),
        ),
    ] {
        let temp = tempfile::tempdir()?;
        let store = Store::at(temp.path());
        save_credential(
            &store,
            Provider::Google,
            Credential::ApiKey {
                key: "test-only".into(),
            },
        )?;
        let (endpoint, captured) = server(json!({"candidates":[{"content":{"role":"model","parts":[{"text":"ok"}]},"finishReason":"STOP"}]}), 200).await?;
        let mut settings = ProviderSettings::defaults(Provider::Google);
        settings.model = model.into();
        settings.endpoint = Some(endpoint);
        let client = AiClient::new(store, Provider::Google, settings)?.with_mode(mode);
        assert_eq!(
            client.complete("instructions", "complete evidence").await?,
            "ok"
        );
        let request = captured.await??;
        let body: Value = serde_json::from_str(
            request
                .split_once("\r\n\r\n")
                .ok_or_else(|| anyhow::anyhow!("Request body"))?
                .1,
        )?;
        assert_eq!(
            body.pointer(&format!("/generationConfig/thinkingConfig/{field}")),
            Some(&expected)
        );
    }
    Ok(())
}

#[tokio::test]
async fn provider_errors_do_not_echo_response_bodies() -> Result<()> {
    let temp = tempfile::TempDir::new()?;
    let store = Store::at(temp.path());
    save_credential(
        &store,
        Provider::Openai,
        Credential::ApiKey {
            key: "test-only".into(),
        },
    )?;
    let (endpoint, task) = server(json!({"error":"sensitive-echo-never-display"}), 401).await?;
    let mut settings = ProviderSettings::defaults(Provider::Openai);
    settings.endpoint = Some(endpoint);
    let error = AiClient::new(store, Provider::Openai, settings)?
        .complete("system", "data")
        .await
        .err()
        .ok_or_else(|| anyhow::anyhow!("expected failure"))?;
    assert!(error.to_string().contains("401"));
    assert!(!error.to_string().contains("sensitive-echo"));
    task.await??;
    Ok(())
}

#[tokio::test]
async fn drafting_uses_real_staged_evidence_but_withholds_sensitive_files() -> Result<()> {
    let temp = tempfile::TempDir::new()?;
    let git = |args: &[&str]| -> Result<()> {
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(temp.path())
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()?;
        anyhow::ensure!(output.status.success(), "fixture Git command failed");
        Ok(())
    };
    git(&["init", "--quiet", "--initial-branch=main"])?;
    std::fs::write(temp.path().join("main.rs"), "fn staged_feature() {}\n")?;
    std::fs::write(
        temp.path().join(".env"),
        "SECRET_TEST_PLACEHOLDER_DO_NOT_SEND\n",
    )?;
    git(&["add", "main.rs", ".env"])?;
    let store = Store::at(temp.path().join(".git/test-credentials"));
    save_credential(
        &store,
        Provider::Openai,
        Credential::ApiKey {
            key: "test-only".into(),
        },
    )?;
    let response =
        responses("{\"action\":\"finish\",\"result\":{\"message\":\"feat: add configuration\"}}");
    let (endpoint, captured) = server(response, 200).await?;
    let mut settings = ProviderSettings::defaults(Provider::Openai);
    settings.endpoint = Some(endpoint);
    let client = AiClient::new(store, Provider::Openai, settings)?;
    let repo = kiri_core::repo::Repository::open(temp.path()).await?;
    let draft = kiri_ai::workflow::draft(&repo, &client, None).await?;
    assert_eq!(draft.message, "feat: add configuration");
    assert!(!draft.warnings.is_empty());
    let sent = captured.await??;
    assert!(sent.contains("staged_feature"));
    assert!(!sent.contains("SECRET_TEST_PLACEHOLDER_DO_NOT_SEND"));
    draft.snapshot.verify(&repo).await?;
    Ok(())
}

#[tokio::test]
async fn subscription_stream_requires_a_completed_response() -> Result<()> {
    use rig_core::{client::CompletionClient, completion::CompletionModel, providers::chatgpt};
    for (body, success) in [
        (
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"partial\"}\n\n".to_owned(),
            false,
        ),
        (
            format!(
                "data: {}\n\n",
                json!({"type":"response.completed","response":responses("done")})
            ),
            true,
        ),
    ] {
        let (endpoint, captured) = server_raw(body, 200, "text/event-stream").await?;
        let client = chatgpt::Client::builder()
            .api_key("test-only")
            .allow_device_flow(false)
            .base_url(endpoint)
            .http_client(rig_core::http_client::ReqwestClient::new())
            .build()?;
        let result = client
            .completion_model("test-model")
            .completion_request("test")
            .preamble("system".into())
            .send()
            .await;
        assert_eq!(
            result.is_ok(),
            success,
            "unexpected subscription completion state"
        );
        captured.await??;
    }
    Ok(())
}
