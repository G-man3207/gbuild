//! ChatGPT (Codex) subscription sign-in: OpenAI OAuth2 + PKCE against
//! auth.openai.com, used against the ChatGPT backend Responses endpoint.
//!
//! The resulting credential is an OpenAI access token (bearer at
//! `https://chatgpt.com/backend-api/codex`) plus a `chatgpt-account-id`
//! header value, stored in `~/.gbuild/auth.json` under the `provider::codex`
//! scope with a refresh token. Tokens are refreshed eagerly at turn start
//! and on demand.

use std::io::{BufRead, BufReader, IsTerminal, Write};
use std::net::TcpListener;
use std::path::Path;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::{Duration, Utc};
use sha2::{Digest, Sha256};

use super::model::{AuthMode, GrokAuth};
use super::provider_keys;

pub const CODEX_PROVIDER_ID: &str = "codex";
/// Public client id shared by OpenAI's own Codex CLI.
const CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const AUTHORIZE_URL: &str = "https://auth.openai.com/oauth/authorize";
const TOKEN_URL: &str = "https://auth.openai.com/oauth/token";
const SCOPES: &str = "openid profile email offline_access";
/// The registered loopback redirect for the Codex client id; the port is
/// fixed by the registration and cannot be randomized.
const CALLBACK_PORT: u16 = 1455;
/// Inference endpoint for Codex-subscription tokens.
pub const CODEX_BACKEND_BASE_URL: &str = "https://chatgpt.com/backend-api/codex";
/// Refresh a token expiring within this window.
const REFRESH_SKEW_SECS: i64 = 120;

fn generate_pkce() -> (String, String) {
    let random_bytes: [u8; 32] = rand::random();
    let verifier = URL_SAFE_NO_PAD.encode(random_bytes);
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    (verifier, challenge)
}

fn scope_path(home: &Path) -> std::path::PathBuf {
    home.join("auth.json")
}

/// Read the stored Codex credential, if any.
pub fn load_codex_auth(home: &Path) -> Option<GrokAuth> {
    let store = super::storage::read_auth_json(&scope_path(home)).ok()?;
    store.get(&provider_keys::scope_for(CODEX_PROVIDER_ID)).cloned()
}

fn store_codex_auth(home: &Path, auth: &GrokAuth) -> std::io::Result<()> {
    let path = scope_path(home);
    let mut map = super::storage::read_auth_json_or_empty_recovering_corrupt(&path)?;
    map.insert(provider_keys::scope_for(CODEX_PROVIDER_ID), auth.clone());
    super::storage::write_auth_json(&path, &map)
}

/// Whether the stored credential is inside the refresh window (or missing).
fn needs_refresh(auth: &GrokAuth) -> bool {
    match auth.expires_at {
        Some(exp) => Utc::now() >= exp - Duration::seconds(REFRESH_SKEW_SECS),
        None => auth.refresh_token.is_some(),
    }
}

/// The current access token + account id, refreshing and persisting when the
/// token is inside the refresh window. Returns `None` when signed out.
pub async fn ensure_fresh_codex(home: &Path) -> Option<(String, Option<String>)> {
    let auth = load_codex_auth(home)?;
    if !needs_refresh(&auth) {
        return Some((auth.key.clone(), auth.organization_id.clone()));
    }
    let refresh_token = auth.refresh_token.clone()?;
    match refresh_codex(home, &auth, &refresh_token).await {
        Ok(fresh) => Some((fresh.key.clone(), fresh.organization_id.clone())),
        Err(e) => {
            // Fall back to the stored token: it may still be accepted, and a
            // definitive answer comes from the API, not the local clock.
            tracing::warn!(error = %e, "codex: token refresh failed; using stored token");
            Some((auth.key.clone(), auth.organization_id.clone()))
        }
    }
}

#[derive(serde::Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: Option<String>,
    id_token: Option<String>,
    expires_in: Option<i64>,
}

fn grok_auth_from_tokens(tokens: &TokenResponse, prev: Option<&GrokAuth>) -> GrokAuth {
    let account_id = tokens
        .id_token
        .as_deref()
        .and_then(account_id_from_id_token)
        .or_else(|| prev.and_then(|p| p.organization_id.clone()));
    GrokAuth {
        key: tokens.access_token.clone(),
        auth_mode: AuthMode::Oidc,
        create_time: Utc::now(),
        user_id: prev.map(|p| p.user_id.clone()).unwrap_or_default(),
        organization_id: account_id,
        refresh_token: tokens
            .refresh_token
            .clone()
            .or_else(|| prev.and_then(|p| p.refresh_token.clone())),
        expires_at: tokens
            .expires_in
            .map(|secs| Utc::now() + Duration::seconds(secs)),
        oidc_issuer: Some("https://auth.openai.com".to_string()),
        oidc_client_id: Some(CLIENT_ID.to_string()),
        ..Default::default()
    }
}

/// Extract the ChatGPT account id from an id_token JWT's claims (no signature
/// verification — the token just arrived over TLS from the token endpoint).
fn account_id_from_id_token(id_token: &str) -> Option<String> {
    let payload = id_token.split('.').nth(1)?;
    let decoded = URL_SAFE_NO_PAD.decode(payload).ok()?;
    let claims: serde_json::Value = serde_json::from_slice(&decoded).ok()?;
    // Primary shape: { "https://api.openai.com/auth": { "chatgpt_account_id": "..." } }
    if let Some(auth) = claims.get("https://api.openai.com/auth") {
        for key in ["chatgpt_account_id", "organization_id", "project_id"] {
            if let Some(v) = auth.get(key).and_then(|v| v.as_str())
                && !v.is_empty()
            {
                return Some(v.to_string());
            }
        }
    }
    // Fallback: { "organizations": [{ "id": "..." }] }
    claims
        .get("organizations")
        .and_then(|o| o.as_array())
        .and_then(|arr| arr.first())
        .and_then(|org| org.get("id"))
        .and_then(|v| v.as_str())
        .map(str::to_string)
}

async fn token_request(form: &[(&str, &str)]) -> anyhow::Result<TokenResponse> {
    let client = gbuild_http::shared_client();
    let resp = client.post(TOKEN_URL).form(form).send().await?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!("OpenAI token endpoint error ({status}): {body}");
    }
    Ok(resp.json().await?)
}

async fn refresh_codex(home: &Path, prev: &GrokAuth, refresh_token: &str) -> anyhow::Result<GrokAuth> {
    let tokens = token_request(&[
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh_token),
        ("client_id", CLIENT_ID),
    ])
    .await?;
    let auth = grok_auth_from_tokens(&tokens, Some(prev));
    store_codex_auth(home, &auth)?;
    Ok(auth)
}

/// Run the full browser sign-in: loopback callback on port 1455 (with a
/// stdin-paste fallback for remote machines), PKCE exchange, account-id
/// capture, and storage under `provider::openai-codex`.
pub async fn run_codex_login() -> anyhow::Result<()> {
    let (verifier, challenge) = generate_pkce();
    let state = uuid::Uuid::now_v7().to_string();
    let listener = TcpListener::bind(("127.0.0.1", CALLBACK_PORT)).map_err(|e| {
        anyhow::anyhow!(
            "cannot bind 127.0.0.1:{CALLBACK_PORT} ({e}); the Codex sign-in needs that port free \
             (another Codex login may be running)"
        )
    })?;
    listener.set_nonblocking(true)?;
    let redirect_uri = format!("http://localhost:{CALLBACK_PORT}/auth/callback");
    let auth_url = format!(
        "{}?response_type=code&client_id={}&redirect_uri={}&scope={}\
         &code_challenge={}&code_challenge_method=S256&state={}&originator=gbuild\
         &id_token_add_organizations=true",
        AUTHORIZE_URL,
        urlencoding::encode(CLIENT_ID),
        urlencoding::encode(&redirect_uri),
        urlencoding::encode(SCOPES),
        urlencoding::encode(&challenge),
        urlencoding::encode(&state),
    );

    eprintln!();
    eprintln!("Signing in with ChatGPT (Codex subscription)...");
    if let Err(e) = webbrowser::open(&auth_url) {
        tracing::debug!(error = %e, "codex: failed to open browser");
    }
    eprintln!("Open this URL to sign in:");
    eprintln!("  {auth_url}");

    let code = wait_for_code(listener, &state).await?;
    let tokens = token_request(&[
        ("grant_type", "authorization_code"),
        ("code", &code),
        ("redirect_uri", &redirect_uri),
        ("client_id", CLIENT_ID),
        ("code_verifier", &verifier),
    ])
    .await?;

    let home = crate::util::gbuild_home::gbuild_home();
    let auth = grok_auth_from_tokens(&tokens, None);
    if auth.organization_id.is_none() {
        tracing::warn!("codex: no chatgpt-account-id claim in id_token; requests may lack the account header");
    }
    store_codex_auth(&home, &auth)?;
    eprintln!("ChatGPT Codex sign-in complete (stored in {}/auth.json)", home.display());
    eprintln!("Select a Codex model with /model (gpt-5.3-codex or gpt-5.2-codex).");
    Ok(())
}

async fn wait_for_code(listener: TcpListener, expected_state: &str) -> anyhow::Result<String> {
    let use_stdin = std::io::stdin().is_terminal();
    if use_stdin {
        eprintln!();
        eprintln!("Paste the redirected URL here if the browser can't connect:");
    }
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    let loopback_tx = tx.clone();
    let state_owned = expected_state.to_string();
    tokio::task::spawn_blocking(move || loopback_listener(listener, loopback_tx, &state_owned));
    if use_stdin {
        let stdin_tx = tx;
        let state_stdin = expected_state.to_string();
        tokio::task::spawn_blocking(move || {
            let stdin = std::io::stdin();
            let mut line = String::new();
            if stdin.lock().read_line(&mut line).is_ok() {
                let pasted = line.trim();
                if let Some(code) = extract_code_and_check_state(pasted, &state_stdin) {
                    let _ = stdin_tx.send(code);
                } else if !pasted.is_empty() && !pasted.contains("://") {
                    let _ = stdin_tx.send(pasted.to_string());
                }
            }
        });
    }
    rx.recv()
        .await
        .ok_or_else(|| anyhow::anyhow!("sign-in cancelled"))
}

fn extract_code_and_check_state(url: &str, expected_state: &str) -> Option<String> {
    let parsed = reqwest::Url::parse(url).ok()?;
    let mut code = None;
    let mut state = None;
    for (k, v) in parsed.query_pairs() {
        match k.as_ref() {
            "code" => code = Some(v.into_owned()),
            "state" => state = Some(v.into_owned()),
            _ => {}
        }
    }
    if state.as_deref() == Some(expected_state) { code } else { None }
}

fn loopback_listener(
    listener: TcpListener,
    tx: tokio::sync::mpsc::UnboundedSender<String>,
    expected_state: &str,
) {
    for stream in listener.incoming() {
        let Ok(mut stream) = stream else { continue };
        let mut reader = BufReader::new(stream.try_clone().expect("clone stream"));
        let mut request_line = String::new();
        if reader.read_line(&mut request_line).is_err() {
            continue;
        }
        let target = request_line.split_whitespace().nth(1).unwrap_or_default();
        let url = format!("http://localhost{target}");
        let code = extract_code_and_check_state(&url, expected_state);
        let body = if code.is_some() {
            "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: 74\r\nConnection: close\r\n\r\n<html><body><h3>Signed in — you can close this tab.</h3></body></html>"
        } else {
            "HTTP/1.1 400 Bad Request\r\nContent-Length: 11\r\nConnection: close\r\n\r\nbad request"
        };
        let _ = stream.write_all(body.as_bytes());
        let _ = stream.flush();
        if let Some(code) = code {
            let _ = tx.send(code);
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id_token_with_claims(claims: serde_json::Value) -> String {
        let payload = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&claims).unwrap());
        format!("header.{payload}.signature")
    }

    #[test]
    fn account_id_from_primary_claim_shape() {
        let token = id_token_with_claims(serde_json::json!({
            "sub": "user-1",
            "https://api.openai.com/auth": { "chatgpt_account_id": "acct-123" }
        }));
        assert_eq!(account_id_from_id_token(&token).as_deref(), Some("acct-123"));
    }

    #[test]
    fn account_id_from_organizations_fallback() {
        let token = id_token_with_claims(serde_json::json!({
            "organizations": [{ "id": "org-9" }]
        }));
        assert_eq!(account_id_from_id_token(&token).as_deref(), Some("org-9"));
    }

    #[test]
    fn account_id_absent_is_none() {
        let token = id_token_with_claims(serde_json::json!({ "sub": "user-1" }));
        assert!(account_id_from_id_token(&token).is_none());
        assert!(account_id_from_id_token("not-a-jwt").is_none());
    }

    #[test]
    fn extract_code_checks_state() {
        let url = "http://localhost:1455/auth/callback?code=abc&state=s1";
        assert_eq!(extract_code_and_check_state(url, "s1").as_deref(), Some("abc"));
        assert!(extract_code_and_check_state(url, "other").is_none());
        assert!(extract_code_and_check_state("not a url", "s1").is_none());
    }

    #[test]
    fn needs_refresh_matrix() {
        let mut auth = GrokAuth::default();
        auth.expires_at = Some(Utc::now() + Duration::hours(1));
        assert!(!needs_refresh(&auth));
        auth.expires_at = Some(Utc::now() + Duration::seconds(30));
        assert!(needs_refresh(&auth));
        auth.expires_at = None;
        auth.refresh_token = Some("r".into());
        assert!(needs_refresh(&auth));
        auth.refresh_token = None;
        assert!(!needs_refresh(&auth));
    }
}
