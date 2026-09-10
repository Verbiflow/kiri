use crate::{
    config::{Provider, ProviderSettings, Settings, api_key, validate_endpoint},
    http, oauth,
    progress::{Observer, Progress, silent},
    transport::Transport,
};
use anyhow::{Context, Result, bail};
use kiri_core::{model::digest, storage::Store};
use rig_core::{
    agent::{AgentBuilder, OutputMode},
    client::CompletionClient,
    completion::{CompletionModel, Prompt, PromptError},
    providers::{anthropic, chatgpt, gemini, openai, xai},
};
use serde_json::{Value, json};

#[derive(Clone)]
pub struct AiClient {
    client: reqwest::Client,
    store: Store,
    provider: Provider,
    settings: ProviderSettings,
    observer: Observer,
    mode: kiri_analysis::AnalysisMode,
}

impl AiClient {
    pub fn configured(store: &Store) -> Result<Self> {
        let settings = Settings::load(store)?;
        let (provider, settings) = settings.active()?;
        Self::new(store.clone(), provider, settings.clone())
    }

    pub fn new(store: Store, provider: Provider, settings: ProviderSettings) -> Result<Self> {
        settings.validate(provider)?;
        Ok(Self {
            client: http::client()?,
            store,
            provider,
            settings,
            observer: silent(),
            mode: kiri_analysis::AnalysisMode::Fast,
        })
    }

    pub fn with_mode(mut self, mode: kiri_analysis::AnalysisMode) -> Self {
        self.mode = mode;
        self
    }
    pub fn with_observer(mut self, observer: Observer) -> Self {
        self.observer = observer;
        self
    }
    pub fn notify(&self, event: Progress) {
        (self.observer)(event);
    }
    pub fn observer(&self) -> Observer {
        self.observer.clone()
    }
    pub fn label(&self) -> String {
        format!("{} / {}", self.provider, self.settings.model)
    }
    pub fn store(&self) -> &Store {
        &self.store
    }
    pub fn identity(&self) -> String {
        digest(
            format!(
                "{}:{}:{}",
                self.provider,
                serde_json::to_string(&self.settings).unwrap_or_default(),
                self.mode
            )
            .as_bytes(),
        )
    }

    pub fn with_model(&self, model: &str) -> Result<Self> {
        let mut client = self.clone();
        client.settings.model = model.to_owned();
        client.settings.validate(client.provider)?;
        Ok(client)
    }

    pub async fn complete(&self, system: &str, input: &str) -> Result<String> {
        self.complete_json(system, input, None).await
    }

    pub async fn complete_json(
        &self,
        system: &str,
        input: &str,
        schema: Option<Value>,
    ) -> Result<String> {
        if input.len() > 4 * 1024 * 1024 {
            bail!("AI input exceeded its context budget");
        }
        let transport = Transport::new(self.client.clone(), self.observer.clone());
        let model = &self.settings.model;
        let result = match self.provider {
            Provider::Google => {
                let endpoint = self
                    .settings
                    .endpoint
                    .as_deref()
                    .unwrap_or("https://generativelanguage.googleapis.com")
                    .trim_end_matches('/')
                    .trim_end_matches("/v1beta");
                let client = gemini::Client::builder()
                    .api_key(api_key(&self.store, self.provider)?)
                    .base_url(endpoint)
                    .http_client(transport)
                    .build()?;
                prompt(
                    client.completion_model(model),
                    system,
                    input,
                    schema,
                    self.gemini_parameters()?,
                )
                .await
            }
            Provider::Anthropic => {
                let endpoint = self
                    .settings
                    .endpoint
                    .as_deref()
                    .unwrap_or("https://api.anthropic.com")
                    .trim_end_matches('/')
                    .trim_end_matches("/v1");
                let client = anthropic::Client::builder()
                    .api_key(api_key(&self.store, self.provider)?)
                    .base_url(endpoint)
                    .http_client(transport)
                    .build()?;
                prompt(
                    client.completion_model(model),
                    system,
                    input,
                    schema,
                    json!({}),
                )
                .await
            }
            Provider::Xai => {
                let endpoint = self
                    .settings
                    .endpoint
                    .as_deref()
                    .unwrap_or("https://api.x.ai")
                    .trim_end_matches('/')
                    .trim_end_matches("/v1");
                let client = xai::Client::builder()
                    .api_key(api_key(&self.store, self.provider)?)
                    .base_url(endpoint)
                    .http_client(transport)
                    .build()?;
                prompt(
                    client.completion_model(model),
                    system,
                    input,
                    schema,
                    json!({}),
                )
                .await
            }
            Provider::Openai | Provider::Azure => {
                let key = api_key(&self.store, self.provider)?;
                let transport = if self.provider == Provider::Azure {
                    transport.azure(key.clone())
                } else {
                    transport
                };
                let client = openai::Client::builder()
                    .api_key(key)
                    .base_url(
                        self.settings
                            .endpoint
                            .as_deref()
                            .unwrap_or("https://api.openai.com/v1"),
                    )
                    .http_client(transport)
                    .build()?;
                prompt(
                    client.completion_model(model),
                    system,
                    input,
                    schema,
                    json!({"store":false}),
                )
                .await
            }
            Provider::Codex => {
                let (access_token, account_id) = oauth::access(&self.store).await?;
                let client = chatgpt::Client::builder()
                    .api_key(chatgpt::ChatGPTAuth::AccessToken {
                        access_token,
                        account_id,
                    })
                    .allow_device_flow(false)
                    .originator("kiri")
                    .http_client(transport)
                    .build()?;
                prompt(
                    client.completion_model(model),
                    system,
                    input,
                    schema,
                    json!({"store":false}),
                )
                .await
            }
            Provider::Bedrock => return self.bedrock(system, input).await,
        };
        result.map_err(sdk_error)
    }

    fn gemini_parameters(&self) -> Result<Value> {
        use gemini::completion::gemini_api_types::{
            AdditionalParameters, GenerationConfig, ThinkingConfig, ThinkingLevel,
        };
        let model = self.settings.model.as_str();
        let thinking = if (model.starts_with("gemini-3.") || model.starts_with("gemini-3-"))
            && !model.contains("image")
        {
            Some(ThinkingConfig {
                thinking_level: Some(match self.mode {
                    kiri_analysis::AnalysisMode::Fast => ThinkingLevel::Low,
                    kiri_analysis::AnalysisMode::Deep => ThinkingLevel::High,
                }),
                thinking_budget: None,
                include_thoughts: None,
            })
        } else if model.starts_with("gemini-2.5-") {
            Some(ThinkingConfig {
                thinking_budget: Some(match self.mode {
                    kiri_analysis::AnalysisMode::Fast => 1024,
                    kiri_analysis::AnalysisMode::Deep => 4096,
                }),
                thinking_level: None,
                include_thoughts: None,
            })
        } else {
            None
        };
        Ok(serde_json::to_value(AdditionalParameters {
            generation_config: Some(GenerationConfig {
                thinking_config: thinking,
                ..Default::default()
            }),
            ..Default::default()
        })?)
    }

    async fn bedrock(&self, system: &str, input: &str) -> Result<String> {
        let body = json!({"system":[{"text":system}],"messages":[{"role":"user","content":[{"text":input}]}],"inferenceConfig":{"maxTokens":8192}});
        let response = if let Some(profile) = &self.settings.aws_profile {
            use std::{io::Write, time::Duration};
            let mut file = tempfile::NamedTempFile::new()?;
            file.write_all(&serde_json::to_vec(&body)?)?;
            let mut command = tokio::process::Command::new("aws");
            command
                .args([
                    "bedrock-runtime",
                    "converse",
                    "--model-id",
                    &self.settings.model,
                    "--region",
                    self.settings.region.as_deref().unwrap_or("us-east-1"),
                    "--profile",
                    profile,
                    "--output",
                    "json",
                    "--no-cli-pager",
                    "--cli-input-json",
                ])
                .arg(format!("file://{}", file.path().display()))
                .env("AWS_PAGER", "");
            if let Some(endpoint) = &self.settings.endpoint {
                command.args(["--endpoint-url", endpoint]);
            }
            let output = kiri_core::process::run(
                command,
                None,
                http::RESPONSE_LIMIT,
                Duration::from_secs(90),
            )
            .await
            .context(
                "Bedrock profile authentication requires the AWS CLI and an active AWS login.",
            )?;
            if !output.status.success() || output.truncated {
                bail!(
                    "Bedrock request failed. Check AWS login, model access, region, and profile."
                );
            }
            serde_json::from_slice(&output.stdout)?
        } else {
            let default = format!(
                "https://bedrock-runtime.{}.amazonaws.com",
                self.settings.region.as_deref().unwrap_or("us-east-1")
            );
            let mut url = validate_endpoint(self.settings.endpoint.as_deref().unwrap_or(&default))?;
            url.path_segments_mut()
                .map_err(|_| anyhow::anyhow!("Invalid Bedrock endpoint"))?
                .pop_if_empty()
                .extend(["model", &self.settings.model, "converse"]);
            http::json(
                http::send_with_retry(
                    self.client
                        .post(url)
                        .bearer_auth(api_key(&self.store, Provider::Bedrock)?)
                        .json(&body),
                    http::RetryPolicy::default(),
                    &self.observer,
                )
                .await?,
            )
            .await?
        };
        bedrock_text(&response)
    }
}

async fn prompt<M: CompletionModel + 'static>(
    model: M,
    system: &str,
    input: &str,
    schema: Option<Value>,
    parameters: Value,
) -> Result<String, PromptError> {
    let mut agent = AgentBuilder::new(model)
        .preamble(system)
        .max_tokens(8192)
        .default_max_turns(1)
        .additional_params(parameters);
    if let Some(schema) = schema {
        let schema = schemars::Schema::try_from(schema).map_err(|_| {
            rig_core::completion::CompletionError::ResponseError("Invalid output schema".into())
        })?;
        agent = agent
            .output_schema_raw(schema)
            .output_mode(OutputMode::Native);
    }
    agent.build().prompt(input).await
}

fn sdk_error(error: PromptError) -> anyhow::Error {
    if let Some(status) = error.provider_response_status() {
        return http::failure(status.as_u16()).into();
    }
    let mut source: Option<&(dyn std::error::Error + 'static)> = Some(&error);
    while let Some(current) = source {
        if current
            .downcast_ref::<kiri_analysis::ContextOverflow>()
            .is_some()
        {
            return kiri_analysis::ContextOverflow.into();
        }
        if let Some(failure) = current.downcast_ref::<http::ProviderFailure>() {
            return failure.clone().into();
        }
        source = current.source();
    }
    anyhow::anyhow!(
        "The provider returned an invalid or incomplete response. Check the model and endpoint, then retry. No Git changes were made."
    )
}

impl kiri_analysis::LanguageModel for AiClient {
    fn identity(&self) -> String {
        self.identity()
    }
    fn complete<'a>(
        &'a self,
        system: &'a str,
        input: &'a str,
        schema: Value,
    ) -> futures::future::BoxFuture<'a, Result<String>> {
        Box::pin(self.complete_json(system, input, Some(schema)))
    }
}

fn bedrock_text(response: &Value) -> Result<String> {
    let text: String = response
        .pointer("/output/message/content")
        .and_then(Value::as_array)
        .context("Bedrock returned no message")?
        .iter()
        .filter_map(|part| part.get("text").and_then(Value::as_str))
        .collect();
    if text.trim().is_empty() {
        bail!("Bedrock returned no text");
    }
    Ok(text)
}
