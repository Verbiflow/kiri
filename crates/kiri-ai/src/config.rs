use anyhow::{Context, Result, bail};
use kiri_core::storage::Store;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, fmt, str::FromStr};

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Provider {
    Google,
    Xai,
    Openai,
    Codex,
    Anthropic,
    Bedrock,
    Azure,
}

impl Provider {
    pub const ALL: [Self; 7] = [
        Self::Google,
        Self::Xai,
        Self::Openai,
        Self::Codex,
        Self::Anthropic,
        Self::Bedrock,
        Self::Azure,
    ];
    pub fn id(self) -> &'static str {
        match self {
            Self::Google => "google",
            Self::Xai => "xai",
            Self::Openai => "openai",
            Self::Codex => "codex",
            Self::Anthropic => "anthropic",
            Self::Bedrock => "bedrock",
            Self::Azure => "azure",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Google => "Google Gemini",
            Self::Xai => "xAI / Grok",
            Self::Openai => "OpenAI API",
            Self::Codex => "ChatGPT subscription",
            Self::Anthropic => "Anthropic Claude",
            Self::Bedrock => "Amazon Bedrock",
            Self::Azure => "Azure OpenAI",
        }
    }
    pub fn env(self) -> &'static str {
        match self {
            Self::Google => "GEMINI_API_KEY",
            Self::Xai => "XAI_API_KEY",
            Self::Openai => "OPENAI_API_KEY",
            Self::Codex => "",
            Self::Anthropic => "ANTHROPIC_API_KEY",
            Self::Bedrock => "AWS_BEARER_TOKEN_BEDROCK",
            Self::Azure => "AZURE_OPENAI_API_KEY",
        }
    }
    pub fn default_model(self) -> &'static str {
        match self {
            Self::Google => "gemini-2.5-flash",
            Self::Xai => "grok-3-mini",
            Self::Openai => "gpt-5-mini",
            Self::Codex => "gpt-5.4-mini",
            Self::Anthropic => "claude-sonnet-4-6",
            Self::Bedrock => "us.anthropic.claude-sonnet-4-6",
            Self::Azure => "",
        }
    }
}

impl fmt::Display for Provider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.id())
    }
}

impl FromStr for Provider {
    type Err = anyhow::Error;
    fn from_str(value: &str) -> Result<Self> {
        Self::ALL.into_iter().find(|p| p.id() == value).ok_or_else(|| anyhow::anyhow!("Unknown provider. Choose google, xai, openai, codex, anthropic, bedrock, or azure."))
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderSettings {
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub region: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aws_profile: Option<String>,
}

impl ProviderSettings {
    pub fn defaults(provider: Provider) -> Self {
        Self {
            model: provider.default_model().to_owned(),
            endpoint: None,
            region: (provider == Provider::Bedrock).then(|| "us-east-1".to_owned()),
            aws_profile: None,
        }
    }

    pub fn validate(&self, provider: Provider) -> Result<()> {
        if self.model.trim().is_empty()
            || self.model.len() > 256
            || self.model.chars().any(char::is_control)
        {
            bail!("Choose a model ID. For Azure, use your deployment name.");
        }
        if provider == Provider::Azure && self.endpoint.is_none() {
            bail!("Azure needs --endpoint https://YOUR-RESOURCE.openai.azure.com/openai/v1");
        }
        if provider == Provider::Codex && self.endpoint.is_some() {
            bail!("Subscription tokens can only be sent to the ChatGPT endpoint");
        }
        if provider != Provider::Bedrock && (self.aws_profile.is_some() || self.region.is_some()) {
            bail!("AWS profile and region apply only to Bedrock");
        }
        if let Some(region) = &self.region
            && (region.is_empty()
                || !region
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-'))
        {
            bail!("Invalid AWS region");
        }
        if let Some(endpoint) = &self.endpoint {
            validate_endpoint(endpoint)?;
        }
        Ok(())
    }
}

pub fn validate_endpoint(endpoint: &str) -> Result<reqwest::Url> {
    let url = reqwest::Url::parse(endpoint).context("Invalid provider endpoint URL")?;
    let local = matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
    if url.scheme() != "https" && !(url.scheme() == "http" && local) {
        bail!("Provider endpoints must use HTTPS, except localhost for testing");
    }
    if !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        bail!("Do not put credentials, query parameters, or fragments in provider URLs");
    }
    Ok(url)
}

/// Presentation and interaction preferences for the TUI. Kept outside the sidecar protocol so
/// changing them never alters the schema digest.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct UiSettings {
    /// Theme identifier, for example `catppuccin-mocha`. `None` keeps the built-in default.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub theme: Option<String>,
    /// Panel border style: `rounded`, `plain`, `double` or `thick`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub borders: Option<String>,
    /// AI analyses estimated at this many model calls or fewer start immediately after a quick
    /// action; larger ones open the cost review first. Zero always asks.
    pub auto_approve_calls: usize,
}

impl Default for UiSettings {
    fn default() -> Self {
        Self {
            theme: None,
            borders: None,
            auto_approve_calls: 12,
        }
    }
}

impl UiSettings {
    pub fn validate(&self) -> Result<()> {
        for value in [&self.theme, &self.borders].into_iter().flatten() {
            if value.is_empty() || value.len() > 64 || value.chars().any(char::is_control) {
                bail!("Invalid UI setting in settings.json");
            }
        }
        if self.auto_approve_calls > 4096 {
            bail!("auto_approve_calls must be at most 4096");
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    pub active: Option<Provider>,
    pub providers: BTreeMap<Provider, ProviderSettings>,
    #[serde(default)]
    pub analysis: crate::analysis::AnalysisOptions,
    #[serde(default)]
    pub ui: UiSettings,
}

impl Settings {
    pub fn load(store: &Store) -> Result<Self> {
        let settings: Self = store.load("settings.json")?;
        settings.analysis.validate()?;
        settings.ui.validate()?;
        for (provider, config) in &settings.providers {
            config.validate(*provider)?;
        }
        Ok(settings)
    }

    pub fn select(store: &Store, provider: Provider, config: ProviderSettings) -> Result<()> {
        config.validate(provider)?;
        store.update::<Self, _>("settings.json", |settings| {
            settings.providers.insert(provider, config);
            settings.active = Some(provider);
            Ok(())
        })
    }

    /// Persist a presentation preference without touching provider settings.
    pub fn update_ui(store: &Store, apply: impl FnOnce(&mut UiSettings)) -> Result<()> {
        store.update::<Self, _>("settings.json", |settings| {
            apply(&mut settings.ui);
            settings.ui.validate()
        })
    }

    pub fn active(&self) -> Result<(Provider, &ProviderSettings)> {
        let provider = self.active.context("Connect a provider first. Press P in the TUI or run `kiri provider connect <provider>`.")?;
        Ok((
            provider,
            self.providers
                .get(&provider)
                .context("Active provider has no settings. Reconnect it.")?,
        ))
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Credential {
    ApiKey {
        key: String,
    },
    OAuth {
        access: String,
        refresh: String,
        expires_at: u64,
        account_id: Option<String>,
    },
}

pub type Credentials = BTreeMap<Provider, Credential>;

pub fn save_credential(store: &Store, provider: Provider, credential: Credential) -> Result<()> {
    store.update::<Credentials, _>("credentials.json", |credentials| {
        credentials.insert(provider, credential);
        Ok(())
    })
}

pub fn api_key(store: &Store, provider: Provider) -> Result<String> {
    if let Ok(value) = std::env::var(provider.env())
        && !value.trim().is_empty()
    {
        return Ok(value.trim().to_owned());
    }
    if provider == Provider::Google
        && let Ok(value) = std::env::var("GOOGLE_API_KEY")
        && !value.trim().is_empty()
    {
        return Ok(value.trim().to_owned());
    }
    match store
        .load::<Credentials>("credentials.json")?
        .remove(&provider)
    {
        Some(Credential::ApiKey { key }) if !key.trim().is_empty() => Ok(key),
        _ => bail!(
            "{} is not connected. Set {} or connect it in the provider picker.",
            provider.label(),
            provider.env()
        ),
    }
}
