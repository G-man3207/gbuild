//! GitHub Copilot sign-in: GitHub OAuth device flow, exchanging the GitHub
//! token for short-lived Copilot API tokens on demand.
//!
//! Two scopes live in `~/.gbuild/auth.json`:
//! - `provider::copilot` — the long-lived GitHub OAuth token (from the device
//!   flow). Never sent to the inference endpoint.
//! - `provider::copilot::session` — the derived Copilot API token (~30 min
//!   TTL), refreshed transparently at turn start and on demand.
//!
//! Inference uses `https://api.githubcopilot.com` (OpenAI Chat Completions)
//! with the derived token as bearer.

use std::io::IsTerminal;
use std::path::Path;

use chrono::{DateTime, Duration, Utc};

use super::model::{AuthMode, GrokAuth};
use super::provider_keys;

pub const COPILOT_PROVIDER_ID: &str = "copilot";
const SESSION_SCOPE: &str = "provider::copilot::session";
/// Public OAuth client id used by VS Code's Copilot Chat integration.
const CLIENT_ID: &str = "Iv1.b507a08c87ecfe98";
const DEVICE_CODE_URL: &str = "https://github.com/login/device/code";
const ACCESS_TOKEN_URL: &str = "https://github.com/login/oauth/access_token";
const COPILOT_TOKEN_URL: &str = "https://api.github.com/copilot_internal/v2/token";
/// Inference endpoint for Copilot tokens.
pub const COPILOT_BACKEND_BASE_URL: &str = "https://api.githubcopilot.com";
/// Refresh a derived token expiring within this window.
const REFRESH_SKEW_SECS: i64 = 60;
/// Give up waiting for the user to authorize after this long.
const DEVICE_FLOW_TIMEOUT_SECS: i64 = 15 * 60;

fn scope_path(home: &Path) -> std::path::PathBuf {
    home.join("auth.json")
}

fn load_scope(home: &Path, scope: &str) -> Option<GrokAuth> {
    let store = super::storage::read_auth_json(&scope_path(home)).ok()?;
    store.get(scope).cloned()
}

fn store_scope(home: &Path, scope: &str, auth: &GrokAuth) -> std::io::Result<()> {
    let path = scope_path(home);
    let mut map = super::storage::read_auth_json_or_empty_recovering_corrupt(&path)?;
    map.insert(scope.to_string(), auth.clone());
    super::storage::write_auth_json(&path, &map)
}

fn fresh(auth: &GrokAuth) -> bool {
    match auth.expires_at {
        Some(exp) => Utc::now() < exp - Duration::seconds(REFRESH_SKEW_SECS),
        None => false,
    }
}

/// The derived Copilot API token when stored and still fresh (sync read for
/// credential resolution).
pub fn load_copilot_token(home: &Path) -> Option<String> {
    let auth = load_scope(home, SESSION_SCOPE)?;
    fresh(&auth).then(|| auth.key.clone())
}

/// Whether a GitHub token (the device-flow credential) is stored.
pub fn has_copilot_login(home: &Path) -> bool {
    load_scope(home, &provider_keys::scope_for(COPILOT_PROVIDER_ID)).is_some()
}

/// A fresh Copilot API token, re-exchanging the stored GitHub token when the
/// derived token is stale. Returns `None` when signed out or the exchange
/// fails (the stored GitHub token may be revoked — sign in again).
pub async fn ensure_fresh_copilot(home: &Path) -> Option<String> {
    if let Some(token) = load_copilot_token(home) {
        return Some(token);
    }
    let gh = load_scope(home, &provider_keys::scope_for(COPILOT_PROVIDER_ID))?;
    match exchange_for_copilot_token(&gh.key).await {
        Ok((token, expires_at)) => {
            let session = GrokAuth {
                key: token.clone(),
                auth_mode: AuthMode::ApiKey,
                expires_at: Some(expires_at),
                ..Default::default()
            };
            if let Err(e) = store_scope(home, SESSION_SCOPE, &session) {
                tracing::warn!(error = %e, "copilot: failed to persist derived token");
            }
            Some(token)
        }
        Err(e) => {
            tracing::warn!(error = %e, "copilot: token exchange failed");
            None
        }
    }
}

#[derive(serde::Deserialize)]
struct CopilotTokenResponse {
    token: String,
    expires_at: Option<i64>,
}

async fn exchange_for_copilot_token(gh_token: &str) -> anyhow::Result<(String, DateTime<Utc>)> {
    let client = gbuild_http::shared_client();
    let resp = client
        .get(COPILOT_TOKEN_URL)
        .header("Authorization", format!("token {gh_token}"))
        .header("Accept", "application/json")
        .header("Editor-Version", "gbuild/0.2")
        .send()
        .await?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!("Copilot token exchange failed ({status}): {body}");
    }
    let parsed: CopilotTokenResponse = resp.json().await?;
    if parsed.token.trim().is_empty() {
        anyhow::bail!("Copilot token exchange returned an empty token");
    }
    let expires_at = parsed
        .expires_at
        .and_then(|ts| DateTime::from_timestamp(ts, 0))
        .unwrap_or_else(|| Utc::now() + Duration::minutes(25));
    Ok((parsed.token, expires_at))
}

#[derive(serde::Deserialize)]
struct DeviceCodeResponse {
    device_code: String,
    user_code: String,
    verification_uri: String,
    interval: Option<u64>,
}

#[derive(serde::Deserialize)]
struct AccessTokenResponse {
    access_token: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
    interval: Option<u64>,
}

/// Run the GitHub device-flow sign-in and store the GitHub OAuth token under
/// `provider::copilot`.
pub async fn run_copilot_login() -> anyhow::Result<()> {
    let client = gbuild_http::shared_client();
    let device: DeviceCodeResponse = client
        .post(DEVICE_CODE_URL)
        .header("Accept", "application/json")
        .form(&[("client_id", CLIENT_ID), ("scope", "read:user")])
        .send()
        .await?
        .json()
        .await?;

    eprintln!();
    eprintln!("Signing in with GitHub Copilot...");
    eprintln!();
    eprintln!("  1. Open:  {}", device.verification_uri);
    eprintln!("  2. Enter: {}", device.user_code);
    eprintln!();
    if let Err(e) = webbrowser::open(&device.verification_uri) {
        tracing::debug!(error = %e, "copilot: failed to open browser");
    }

    let gh_token = poll_for_authorization(&client, &device).await?;

    let home = crate::util::gbuild_home::gbuild_home();
    let auth = GrokAuth {
        key: gh_token,
        auth_mode: AuthMode::ApiKey,
        ..Default::default()
    };
    store_scope(&home, &provider_keys::scope_for(COPILOT_PROVIDER_ID), &auth)?;
    // Prime the derived token so the first turn doesn't wait on the exchange.
    let _ = ensure_fresh_copilot(&home).await;
    eprintln!("GitHub Copilot sign-in complete (stored in {}/auth.json)", home.display());
    eprintln!("Select a Copilot model with /model (e.g. copilot gpt-5.3-codex).");
    Ok(())
}

async fn poll_for_authorization(
    client: &reqwest::Client,
    device: &DeviceCodeResponse,
) -> anyhow::Result<String> {
    let mut interval = Duration::seconds(device.interval.unwrap_or(5).max(1) as i64);
    let deadline = Utc::now() + Duration::seconds(DEVICE_FLOW_TIMEOUT_SECS);
    loop {
        if Utc::now() > deadline {
            anyhow::bail!("timed out waiting for GitHub authorization");
        }
        tokio::time::sleep(interval.to_std().unwrap_or(std::time::Duration::from_secs(5))).await;
        let resp: AccessTokenResponse = client
            .post(ACCESS_TOKEN_URL)
            .header("Accept", "application/json")
            .form(&[
                ("client_id", CLIENT_ID),
                ("device_code", device.device_code.as_str()),
                ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
            ])
            .send()
            .await?
            .json()
            .await?;
        if let Some(token) = resp.access_token {
            return Ok(token);
        }
        match resp.error.as_deref() {
            Some("authorization_pending") => {}
            Some("slow_down") => {
                interval = interval + Duration::seconds(resp.interval.unwrap_or(5) as i64);
            }
            Some("expired_token") => anyhow::bail!("device code expired; run login again"),
            Some("access_denied") => anyhow::bail!("authorization denied"),
            Some(other) => anyhow::bail!(
                "GitHub authorization failed: {other} ({})",
                resp.error_description.unwrap_or_default()
            ),
            None => anyhow::bail!("GitHub authorization failed: empty response"),
        }
    }
}

/// Remove both Copilot scopes (login + derived session token).
pub fn clear_copilot(home: &Path) -> std::io::Result<()> {
    let path = scope_path(home);
    if let Ok(mut map) = super::storage::read_auth_json(&path) {
        map.remove(&provider_keys::scope_for(COPILOT_PROVIDER_ID));
        map.remove(SESSION_SCOPE);
        if map.is_empty() {
            let _ = std::fs::remove_file(&path);
        } else {
            super::storage::write_auth_json(&path, &map)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_window_math() {
        let mut auth = GrokAuth::default();
        auth.expires_at = Some(Utc::now() + Duration::minutes(10));
        assert!(fresh(&auth));
        auth.expires_at = Some(Utc::now() + Duration::seconds(30));
        assert!(!fresh(&auth));
        auth.expires_at = None;
        assert!(!fresh(&auth));
    }

    #[test]
    fn scope_round_trip_and_clear() {
        let home = tempfile::tempdir().unwrap();
        let gh = GrokAuth {
            key: "gho_test".into(),
            auth_mode: AuthMode::ApiKey,
            ..Default::default()
        };
        store_scope(
            home.path(),
            &provider_keys::scope_for(COPILOT_PROVIDER_ID),
            &gh,
        )
        .unwrap();
        assert!(has_copilot_login(home.path()));
        let session = GrokAuth {
            key: "tid_test".into(),
            auth_mode: AuthMode::ApiKey,
            expires_at: Some(Utc::now() + Duration::minutes(10)),
            ..Default::default()
        };
        store_scope(home.path(), SESSION_SCOPE, &session).unwrap();
        assert_eq!(load_copilot_token(home.path()).as_deref(), Some("tid_test"));
        clear_copilot(home.path()).unwrap();
        assert!(!has_copilot_login(home.path()));
        assert!(load_copilot_token(home.path()).is_none());
    }
}
