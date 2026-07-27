//! Stored API keys for third-party model providers.
//!
//! `/login <provider>` (or `gbuild login --provider <id> --api-key <key>`)
//! persists a provider key in `~/.gbuild/auth.json` under a `provider::<id>`
//! scope, alongside the xAI session/API-key scopes. A process-wide overlay
//! maps each provider's environment variable names to its stored key, so
//! every `env_key` resolution site (catalog ordering, per-model credential
//! resolution, auxiliary services) sees stored keys exactly like real
//! environment variables — the overlay is checked first, the process env
//! second.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{OnceLock, RwLock};

use super::model::{AuthMode, GrokAuth};
use super::storage;

/// A third-party provider that accepts a stored API key.
pub struct ProviderKeySpec {
    /// Stable identifier (`/login <id>`, `--provider <id>`, scope suffix).
    pub id: &'static str,
    /// Human-readable label for menus and toasts.
    pub display: &'static str,
    /// Environment variable names the stored key answers to, in priority
    /// order (mirrors the built-in catalog's `env_key` fields).
    pub env_vars: &'static [&'static str],
}

/// Providers with stored-key login, in menu order. xAI is listed for
/// completeness but its primary login remains the OAuth flow.
pub const PROVIDER_KEY_SPECS: &[ProviderKeySpec] = &[
    ProviderKeySpec {
        id: "anthropic",
        display: "Anthropic (Claude)",
        env_vars: &["ANTHROPIC_API_KEY"],
    },
    ProviderKeySpec {
        id: "openai",
        display: "OpenAI",
        env_vars: &["OPENAI_API_KEY"],
    },
    ProviderKeySpec {
        id: "google",
        display: "Google Gemini",
        env_vars: &["GEMINI_API_KEY", "GOOGLE_API_KEY"],
    },
    ProviderKeySpec {
        id: "openrouter",
        display: "OpenRouter",
        env_vars: &["OPENROUTER_API_KEY"],
    },
    ProviderKeySpec {
        id: "opencode",
        display: "OpenCode Go",
        env_vars: &["OPENCODE_API_KEY"],
    },
    ProviderKeySpec {
        id: "kimi",
        display: "Kimi (Moonshot)",
        env_vars: &["KIMI_API_KEY"],
    },
    ProviderKeySpec {
        id: "zai",
        display: "GLM (Z.AI)",
        env_vars: &["ZHIPU_API_KEY", "ZAI_API_KEY"],
    },
    ProviderKeySpec {
        id: "xai",
        display: "xAI",
        env_vars: &["XAI_API_KEY"],
    },
];

/// auth.json scope for a provider's stored key. xAI shares the existing
/// `xai::api_key` scope so there is exactly one xAI key on disk.
pub fn scope_for(provider_id: &str) -> String {
    if provider_id == "xai" {
        return super::model::API_KEY_SCOPE.to_owned();
    }
    format!("provider::{provider_id}")
}

/// Look up a provider spec by its id (case-insensitive).
pub fn spec_by_id(id: &str) -> Option<&'static ProviderKeySpec> {
    let needle = id.trim().to_ascii_lowercase();
    PROVIDER_KEY_SPECS.iter().find(|s| s.id == needle)
}

/// The provider a given environment variable belongs to, if any.
pub fn spec_by_env_var(env_var: &str) -> Option<&'static ProviderKeySpec> {
    PROVIDER_KEY_SPECS
        .iter()
        .find(|s| s.env_vars.contains(&env_var))
}

/// Ids of every provider env var the overlay can answer for.
fn all_env_vars() -> impl Iterator<Item = &'static str> {
    PROVIDER_KEY_SPECS
        .iter()
        .flat_map(|s| s.env_vars.iter().copied())
}

// ── Overlay ─────────────────────────────────────────────────────────────

/// Process-wide map of env var name → stored provider key. Loaded lazily
/// from auth.json on first read; updated eagerly on store/clear.
static OVERLAY: OnceLock<RwLock<HashMap<String, String>>> = OnceLock::new();

fn overlay() -> &'static RwLock<HashMap<String, String>> {
    OVERLAY.get_or_init(|| {
        let mut map = HashMap::new();
        let home = crate::util::gbuild_home::gbuild_home();
        if let Ok(store) = storage::read_auth_json(&home.join("auth.json")) {
            for spec in PROVIDER_KEY_SPECS {
                if let Some(auth) = store.get(&scope_for(spec.id)) {
                    for var in spec.env_vars {
                        map.insert((*var).to_string(), auth.key.clone());
                    }
                }
            }
        }
        RwLock::new(map)
    })
}

/// Resolve an env var name against stored provider keys, then process env.
/// Used as the getter for `EnvKeys` resolution everywhere.
pub fn resolve_env_or_stored(name: &str) -> Option<String> {
    if let Ok(map) = overlay().read()
        && let Some(key) = map.get(name)
        && !key.trim().is_empty()
    {
        return Some(key.clone());
    }
    std::env::var(name).ok()
}

/// Whether any provider credential is available (stored or process env).
pub fn any_provider_credentials_available() -> bool {
    all_env_vars().any(|var| resolve_env_or_stored(var).is_some())
}

// ── Store / clear ───────────────────────────────────────────────────────

/// Persist a provider API key in auth.json and update the overlay.
pub fn store_provider_key(gbuild_home: &Path, provider_id: &str, key: &str) -> std::io::Result<()> {
    let spec = spec_by_id(provider_id).ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("unknown provider '{provider_id}'"),
        )
    })?;
    let path = gbuild_home.join("auth.json");
    let mut map = storage::read_auth_json_or_empty_recovering_corrupt(&path)?;
    map.insert(
        scope_for(spec.id),
        GrokAuth {
            key: key.to_owned(),
            auth_mode: AuthMode::ApiKey,
            ..Default::default()
        },
    );
    storage::write_auth_json(&path, &map)?;
    if let Ok(mut overlay) = overlay().write() {
        for var in spec.env_vars {
            overlay.insert((*var).to_string(), key.to_owned());
        }
    }
    Ok(())
}

/// Remove a provider's stored key from auth.json and the overlay.
pub fn clear_provider_key(gbuild_home: &Path, provider_id: &str) -> std::io::Result<()> {
    let spec = spec_by_id(provider_id).ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("unknown provider '{provider_id}'"),
        )
    })?;
    let path = gbuild_home.join("auth.json");
    if let Ok(mut map) = storage::read_auth_json(&path) {
        map.remove(&scope_for(spec.id));
        if map.is_empty() {
            let _ = std::fs::remove_file(&path);
        } else {
            storage::write_auth_json(&path, &map)?;
        }
    }
    if let Ok(mut overlay) = overlay().write() {
        for var in spec.env_vars {
            overlay.remove(*var);
        }
    }
    Ok(())
}

/// The provider ids with a stored key (for status displays).
pub fn stored_provider_ids(gbuild_home: &Path) -> Vec<&'static str> {
    let Ok(store) = storage::read_auth_json(&gbuild_home.join("auth.json")) else {
        return Vec::new();
    };
    PROVIDER_KEY_SPECS
        .iter()
        .filter(|s| store.contains_key(&scope_for(s.id)))
        .map(|s| s.id)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scope_format_and_lookup() {
        assert_eq!(scope_for("anthropic"), "provider::anthropic");
        assert!(spec_by_id("Anthropic").is_some());
        assert!(spec_by_id("ANTHROPIC").is_some());
        assert!(spec_by_id("nope").is_none());
        assert_eq!(
            spec_by_env_var("ANTHROPIC_API_KEY").map(|s| s.id),
            Some("anthropic")
        );
        assert_eq!(spec_by_env_var("ZAI_API_KEY").map(|s| s.id), Some("zai"));
        assert!(spec_by_env_var("UNRELATED_VAR").is_none());
    }

    #[test]
    fn store_read_clear_round_trip() {
        let home = tempfile::tempdir().unwrap();
        store_provider_key(home.path(), "anthropic", "sk-ant-test").unwrap();
        let store = storage::read_auth_json(&home.path().join("auth.json")).unwrap();
        assert_eq!(
            store.get(&scope_for("anthropic")).map(|a| a.key.as_str()),
            Some("sk-ant-test")
        );
        assert_eq!(stored_provider_ids(home.path()), vec!["anthropic"]);
        clear_provider_key(home.path(), "anthropic").unwrap();
        assert!(stored_provider_ids(home.path()).is_empty());
        assert!(std::fs::metadata(home.path().join("auth.json")).is_err());
    }
}
