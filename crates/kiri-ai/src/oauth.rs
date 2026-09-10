use crate::{
    config::{Credential, Credentials, Provider, save_credential},
    http,
};
use anyhow::{Context, Result, bail};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use kiri_core::storage::{FileLock, Store};
use serde::Deserialize;
use serde_json::{Value, json};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const ISSUER: &str = "https://auth.openai.com";
pub const DEVICE_URL: &str = "https://auth.openai.com/codex/device";

#[derive(Deserialize)]
pub struct DeviceLogin {
    device_auth_id: String,
    pub user_code: String,
    interval: Value,
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: Option<String>,
    id_token: Option<String>,
    expires_in: Option<u64>,
}

pub async fn begin() -> Result<DeviceLogin> {
    http::json(
        http::send(
            http::client()?
                .post(format!("{ISSUER}/api/accounts/deviceauth/usercode"))
                .json(&json!({"client_id": CLIENT_ID})),
        )
        .await?,
    )
    .await
}

pub async fn finish(store: &Store, login: DeviceLogin) -> Result<()> {
    let client = http::client()?;
    let interval = login
        .interval
        .as_u64()
        .or_else(|| login.interval.as_str().and_then(|s| s.parse().ok()))
        .unwrap_or(5)
        .clamp(1, 60);
    let started = tokio::time::Instant::now();
    while started.elapsed() < Duration::from_secs(900) {
        let response = client
            .post(format!("{ISSUER}/api/accounts/deviceauth/token"))
            .json(&json!({"device_auth_id": login.device_auth_id, "user_code": login.user_code}))
            .send()
            .await?;
        if response.status().is_success() {
            #[derive(Deserialize)]
            struct Authorization {
                authorization_code: String,
                code_verifier: String,
            }
            let auth: Authorization = http::json(response).await?;
            let response = http::send(client.post(format!("{ISSUER}/oauth/token")).form(&[
                ("grant_type", "authorization_code"),
                ("client_id", CLIENT_ID),
                ("code", auth.authorization_code.as_str()),
                ("code_verifier", auth.code_verifier.as_str()),
                (
                    "redirect_uri",
                    "https://auth.openai.com/deviceauth/callback",
                ),
            ]))
            .await?;
            let tokens: TokenResponse = http::json(response).await?;
            save_credential(store, Provider::Codex, credential(tokens, None)?)?;
            return Ok(());
        }
        if !matches!(response.status().as_u16(), 403 | 404) {
            http::require_success(response)?;
        }
        tokio::time::sleep(Duration::from_secs(interval + 3)).await;
    }
    bail!("Login expired. Run `kiri provider login codex` to start again.")
}

pub async fn access(store: &Store) -> Result<(String, Option<String>)> {
    if let Some(access) = cached_access(store)? {
        return Ok(access);
    }
    static REFRESH: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    let _refresh = REFRESH.lock().await;
    if let Some(access) = cached_access(store)? {
        return Ok(access);
    }
    store.prepare()?;
    let _lock = FileLock::acquire(store.path("codex-refresh.lock"))?;
    let stored = store
        .load::<Credentials>("credentials.json")?
        .remove(&Provider::Codex)
        .context("Sign in first with `kiri provider login codex`.")?;
    let Credential::OAuth {
        access,
        refresh,
        expires_at,
        account_id,
    } = stored
    else {
        bail!("Reconnect your ChatGPT subscription");
    };
    if expires_at > now()? + 60 {
        return Ok((access, account_id));
    }
    let response = http::send(
        http::client()?
            .post(format!("{ISSUER}/oauth/token"))
            .form(&[
                ("grant_type", "refresh_token"),
                ("client_id", CLIENT_ID),
                ("refresh_token", refresh.as_str()),
            ]),
    )
    .await?;
    let tokens: TokenResponse = http::json(response).await?;
    let mut updated = credential(tokens, Some(refresh))?;
    let Credential::OAuth {
        access,
        account_id: new_account,
        ..
    } = &mut updated
    else {
        bail!("Refreshed subscription credentials have an invalid type");
    };
    *new_account = new_account.take().or(account_id);
    let result = (access.clone(), new_account.clone());
    save_credential(store, Provider::Codex, updated)?;
    Ok(result)
}

fn cached_access(store: &Store) -> Result<Option<(String, Option<String>)>> {
    let stored = store
        .load::<Credentials>("credentials.json")?
        .remove(&Provider::Codex)
        .context("Sign in first with `kiri provider login codex`.")?;
    match stored {
        Credential::OAuth {
            access,
            account_id,
            expires_at,
            ..
        } if expires_at > now()? + 60 => Ok(Some((access, account_id))),
        Credential::OAuth { .. } => Ok(None),
        _ => bail!("Reconnect your ChatGPT subscription"),
    }
}

fn credential(tokens: TokenResponse, previous_refresh: Option<String>) -> Result<Credential> {
    let account_id = tokens
        .id_token
        .as_deref()
        .and_then(account_id)
        .or_else(|| account_id(&tokens.access_token));
    Ok(Credential::OAuth {
        access: tokens.access_token,
        refresh: tokens
            .refresh_token
            .or(previous_refresh)
            .context("Login did not return a refresh token")?,
        expires_at: now()? + tokens.expires_in.unwrap_or(3600),
        account_id,
    })
}

fn account_id(token: &str) -> Option<String> {
    let body = URL_SAFE_NO_PAD.decode(token.split('.').nth(1)?).ok()?;
    let claims: Value = serde_json::from_slice(&body).ok()?;
    claims
        .get("chatgpt_account_id")
        .or_else(|| {
            claims
                .get("https://api.openai.com/auth")?
                .get("chatgpt_account_id")
        })?
        .as_str()
        .map(str::to_owned)
}

fn now() -> Result<u64> {
    Ok(SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn parallel_valid_token_reads_do_not_take_the_refresh_lock() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let store = Store::at(temp.path());
        save_credential(
            &store,
            Provider::Codex,
            Credential::OAuth {
                access: "test-access".into(),
                refresh: "test-refresh".into(),
                expires_at: u64::MAX,
                account_id: Some("test-account".into()),
            },
        )?;
        let _lock = FileLock::acquire(store.path("codex-refresh.lock"))?;
        let mut jobs = tokio::task::JoinSet::new();
        for _ in 0..8 {
            let store = store.clone();
            jobs.spawn(async move { access(&store).await });
        }
        while let Some(result) = jobs.join_next().await {
            assert_eq!(
                result??,
                ("test-access".into(), Some("test-account".into()))
            );
        }
        Ok(())
    }
}
