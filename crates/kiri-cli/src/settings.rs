use crate::args::{ProviderCommand, WorkspaceCommand};
use anyhow::{Result, bail};
use kiri_ai::{
    config::{
        Credential, Credentials, Provider, ProviderSettings, Settings, api_key, save_credential,
    },
    oauth,
};
use kiri_core::{model::terminal_text, repo::Repository, storage::Store, workspace::Workspaces};
use std::io::Read;

pub async fn provider(store: &Store, command: ProviderCommand) -> Result<()> {
    match command {
        ProviderCommand::List { json } => {
            let settings = Settings::load(store)?;
            let credentials: Credentials = store.load("credentials.json")?;
            let rows: Vec<_> = Provider::ALL.iter().map(|p| {
                let connected = if *p == Provider::Codex { credentials.contains_key(p) }
                    else { api_key(store, *p).is_ok() || settings.providers.get(p).is_some_and(|s| s.aws_profile.is_some()) };
                serde_json::json!({"id":p.id(), "name":p.label(), "active":settings.active==Some(*p), "connected":connected,
                    "model":settings.providers.get(p).map(|s|s.model.as_str()).unwrap_or(p.default_model()), "environment":p.env()})
            }).collect();
            if json {
                println!("{}", serde_json::to_string_pretty(&rows)?);
            } else {
                for row in rows {
                    println!(
                        "{} {:12} {:24} {}",
                        if row["active"] == true { ">" } else { " " },
                        row["id"].as_str().unwrap_or_default(),
                        if row["connected"] == true {
                            "connected"
                        } else {
                            "not connected"
                        },
                        terminal_text(row["model"].as_str().unwrap_or_default())
                    );
                }
                println!(
                    "\nConnect: kiri provider connect google\nSubscription: kiri provider login codex"
                );
            }
        }
        ProviderCommand::Connect {
            provider,
            model,
            endpoint,
            region,
            aws_profile,
            key_stdin,
        } => {
            if provider == Provider::Codex {
                bail!("Use `kiri provider login codex` for ChatGPT subscription authentication");
            }
            let mut config = ProviderSettings::defaults(provider);
            if let Some(model) = model {
                config.model = model;
            }
            config.endpoint = endpoint;
            if region.is_some() {
                config.region = region;
            }
            config.aws_profile = aws_profile;
            config.validate(provider)?;
            if key_stdin {
                let mut key = String::new();
                std::io::stdin().take(16385).read_to_string(&mut key)?;
                let key = key.trim();
                if key.is_empty() || key.len() > 16384 || key.chars().any(char::is_control) {
                    bail!("Expected one API key on stdin, at most 16 KiB");
                }
                save_credential(
                    store,
                    provider,
                    Credential::ApiKey {
                        key: key.to_owned(),
                    },
                )?;
            }
            if config.aws_profile.is_none() {
                api_key(store, provider)?;
            }
            Settings::select(store, provider, config)?;
            println!(
                "Connected {}. Credentials stay local; AI only runs when requested.",
                provider.label()
            );
        }
        ProviderCommand::Use { provider, model } => {
            let mut settings = Settings::load(store)?;
            let mut config = settings
                .providers
                .remove(&provider)
                .unwrap_or_else(|| ProviderSettings::defaults(provider));
            if let Some(model) = model {
                config.model = model;
            }
            Settings::select(store, provider, config)?;
            println!("Selected {}", provider.label());
        }
        ProviderCommand::Login { provider } => {
            if provider != Provider::Codex {
                bail!(
                    "Browser sign-in is available for codex. Use `provider connect` for API-key providers."
                );
            }
            let login = oauth::begin().await?;
            println!(
                "Open {}\nEnter code: {}\nWaiting for sign-in. Ctrl+C cancels.",
                oauth::DEVICE_URL,
                terminal_text(&login.user_code)
            );
            tokio::select! {
                result = oauth::finish(store, login) => result?,
                _ = tokio::signal::ctrl_c() => bail!("Sign-in cancelled"),
            }
            Settings::select(
                store,
                Provider::Codex,
                ProviderSettings::defaults(Provider::Codex),
            )?;
            println!("Connected ChatGPT subscription.");
        }
    }
    Ok(())
}

pub async fn workspace(store: &Store, command: WorkspaceCommand) -> Result<()> {
    match command {
        WorkspaceCommand::List { json } => {
            let registry = Workspaces::load(store)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&registry)?);
            } else {
                for (index, entry) in registry.entries.iter().enumerate() {
                    println!(
                        "{}  {}  {}",
                        index + 1,
                        entry.name,
                        terminal_text(&entry.root.display().to_string())
                    );
                }
            }
        }
        WorkspaceCommand::Add { path, name } => {
            let workspace = Workspaces::add(store, &path, name).await?;
            println!("Saved {}", workspace.name);
        }
        WorkspaceCommand::Remove { path } => {
            let root = match Repository::open(&path).await {
                Ok(repo) => repo.root().to_path_buf(),
                Err(_) => path,
            };
            Workspaces::remove(store, &root)?;
            println!("Removed the saved workspace. Repository files were not changed.");
        }
    }
    Ok(())
}
