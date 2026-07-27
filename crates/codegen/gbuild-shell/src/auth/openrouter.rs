//! OpenRouter OAuth sign-in (PKCE → API key).
//!
//! OpenRouter's flow is OAuth-flavored but not OIDC: the browser dance
//! produces a `code` that is exchanged for a user API key at
//! `/api/v1/auth/keys`. The resulting key is stored like any other provider
//! key (`provider::openrouter` scope) and never expires on our side.

use std::io::{BufRead, BufReader, IsTerminal, Write};
use std::net::TcpListener;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use sha2::{Digest, Sha256};

const AUTHORIZE_URL: &str = "https://openrouter.ai/auth";
const EXCHANGE_URL: &str = "https://openrouter.ai/api/v1/auth/keys";

fn generate_pkce() -> (String, String) {
    let random_bytes: [u8; 32] = rand::random();
    let verifier = URL_SAFE_NO_PAD.encode(random_bytes);
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    (verifier, challenge)
}

/// Run the OpenRouter sign-in flow: open the browser, wait for the loopback
/// callback (or a pasted code on the CLI), exchange the code for an API key,
/// and store it under the `openrouter` provider. Returns the key.
pub async fn run_openrouter_login() -> anyhow::Result<String> {
    let (verifier, challenge) = generate_pkce();
    let listener = TcpListener::bind(("127.0.0.1", 0)).ok();
    let callback_url = match &listener {
        Some(l) => format!("http://127.0.0.1:{}", l.local_addr()?.port()),
        None => {
            eprintln!("(loopback unavailable — paste the code after signing in)");
            "http://127.0.0.1".to_string()
        }
    };
    let auth_url = format!(
        "{}?callback_url={}&code_challenge={}&code_challenge_method=S256",
        AUTHORIZE_URL,
        urlencoding::encode(&callback_url),
        urlencoding::encode(&challenge),
    );

    eprintln!();
    eprintln!("Signing in with OpenRouter...");
    if let Err(e) = webbrowser::open(&auth_url) {
        tracing::debug!(error = %e, "openrouter: failed to open browser");
    }
    eprintln!("Open this URL to sign in:");
    eprintln!("  {auth_url}");

    let code = wait_for_code(listener).await?;
    let key = exchange_code(&code, &verifier).await?;
    let home = crate::util::gbuild_home::gbuild_home();
    crate::auth::provider_keys::store_provider_key(&home, "openrouter", &key)?;
    eprintln!("OpenRouter API key stored in {}/auth.json", home.display());
    Ok(key)
}

/// Wait for the loopback callback, racing a pasted code on stdin. The paste
/// path works on any readable stdin (SSH session, pipe, here-doc), so the
/// flow completes on machines with no browser.
async fn wait_for_code(listener: Option<TcpListener>) -> anyhow::Result<String> {
    if std::io::stdin().is_terminal() {
        eprintln!();
        eprintln!("Paste the code here if the browser can't connect:");
    }
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();

    // Loopback path (when a port could be bound).
    if let Some(listener) = listener {
        listener.set_nonblocking(true)?;
        let loopback_tx = tx.clone();
        tokio::task::spawn_blocking(move || loopback_listener(listener, loopback_tx));
    }

    // Stdin paste path (any readable stdin; a closed/empty stdin never fires).
    {
        let stdin_tx = tx;
        tokio::task::spawn_blocking(move || {
            let stdin = std::io::stdin();
            let mut line = String::new();
            if stdin.lock().read_line(&mut line).is_ok() {
                let pasted = line.trim();
                // Accept a bare code or a full callback URL containing ?code=.
                let code = extract_code(pasted).unwrap_or_else(|| pasted.to_string());
                if !code.is_empty() {
                    let _ = stdin_tx.send(code);
                }
            }
        });
    }

    match tokio::time::timeout(
        std::time::Duration::from_secs(15 * 60),
        rx.recv(),
    )
    .await
    {
        Ok(Some(code)) => Ok(code),
        _ => anyhow::bail!(
            "timed out waiting for sign-in. Re-run and paste the code, \
             or use OPENROUTER_API_KEY directly instead"
        ),
    }
}

/// Extract `code` from a pasted callback URL, if it is one.
fn extract_code(pasted: &str) -> Option<String> {
    let parsed = reqwest::Url::parse(pasted).ok()?;
    parsed
        .query_pairs()
        .find(|(k, _)| k == "code")
        .map(|(_, v)| v.into_owned())
}

fn loopback_listener(listener: TcpListener, tx: tokio::sync::mpsc::UnboundedSender<String>) {
    for stream in listener.incoming() {
        let Ok(mut stream) = stream else { continue };
        let mut reader = BufReader::new(stream.try_clone().expect("clone stream"));
        let mut request_line = String::new();
        if reader.read_line(&mut request_line).is_err() {
            continue;
        }
        // "GET /?code=... HTTP/1.1"
        let code = request_line
            .split_whitespace()
            .nth(1)
            .and_then(extract_code_loose);
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

/// Pull `code=...` out of a raw request target like `/?code=abc&state=x`.
fn extract_code_loose(target: &str) -> Option<String> {
    let query = target.split('?').nth(1)?;
    for pair in query.split('&') {
        let mut kv = pair.splitn(2, '=');
        if kv.next() == Some("code") {
            return kv
                .next()
                .and_then(|v| urlencoding::decode(v).ok().map(|d| d.into_owned()));
        }
    }
    None
}

#[derive(serde::Deserialize)]
struct ExchangeResponse {
    key: String,
}

async fn exchange_code(code: &str, verifier: &str) -> anyhow::Result<String> {
    let client = gbuild_http::shared_client();
    let resp = client
        .post(EXCHANGE_URL)
        .json(&serde_json::json!({
            "code": code,
            "code_verifier": verifier,
        }))
        .send()
        .await?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!("OpenRouter code exchange failed ({status}): {body}");
    }
    let parsed: ExchangeResponse = resp.json().await?;
    if parsed.key.trim().is_empty() {
        anyhow::bail!("OpenRouter code exchange returned an empty key");
    }
    Ok(parsed.key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_code_loose_parses_request_target() {
        assert_eq!(
            extract_code_loose("/?code=abc123&state=x"),
            Some("abc123".to_string())
        );
        assert_eq!(extract_code_loose("/?state=x"), None);
        assert_eq!(extract_code_loose("/"), None);
    }

    #[test]
    fn extract_code_from_pasted_url() {
        assert_eq!(
            extract_code("http://127.0.0.1:8080/?code=xyz"),
            Some("xyz".to_string())
        );
        assert_eq!(extract_code("not a url"), None);
    }
}
