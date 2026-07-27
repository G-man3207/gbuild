//! Agent bootstrap and lifecycle hooks.
//!
//! [`bootstrap`] runs the full init sequence (config resolution, process
//! singletons, model catalog) and returns a resolved config + `ModelsManager`.
//! [`update_telemetry_config`] re-initializes telemetry after auth changes.

use std::sync::Arc;

use indexmap::IndexMap;

use crate::agent::config::{self, Config as AgentConfig, ModelEntry};
use crate::agent::models::ModelsManager;
use crate::auth::AuthManager;
use crate::config::StorageMode;

/// Resolve config, init process singletons, build the model catalog.
///
/// The `ModelsManager` is `Clone + Send`, so callers that need a handle
/// for the config watcher can clone it before passing it to
/// `MvpAgent::with_models`.
pub fn bootstrap(
    cfg: &AgentConfig,
    auth_manager: &Arc<AuthManager>,
    prefetched: Option<IndexMap<String, ModelEntry>>,
) -> Result<(AgentConfig, ModelsManager), String> {
    let mut cfg = cfg.clone();
    ensure_remote_settings_side_effects(&mut cfg);
    crate::managed_config::managed_policy_gate()?;
    let cfg = resolve_config(&cfg, auth_manager);
    cfg.validate_model_filters()?;
    init_process(&cfg, auth_manager);
    let models_manager = ModelsManager::from_config(&cfg, prefetched, auth_manager.clone())?;

    // Refresh on every auth refresh — the FSEvents watcher can silently die after
    // macOS sleep, stranding the catalog on bundled defaults.
    models_manager.start_auth_refresh_watcher(auth_manager.refresh_notifier());

    Ok((cfg, models_manager))
}

/// Print a `bootstrap`/`MvpAgent::new` config error and exit (process boundary).
///
/// Restores native stderr first: a managed-policy refusal on the ACP/server path reaches here
/// while fd 2 may still point at the `/dev/null` the TUI's `redirect_native_stderr()` set, which
/// would swallow the message. No-op when stderr was never redirected (headless).
pub(crate) fn exit_on_config_error<T>(e: String) -> T {
    xai_tty_utils::restore_native_stderr();
    eprintln!("\nConfiguration error:\n\n    {e}\n");
    std::process::exit(1);
}

/// Apply process-global remote side effects (signature kill-switch and
/// caches) for client-supplied remote settings. Safe to call more than once.
/// This fork never fetches remote settings itself, so `cfg.remote_settings`
/// stays `None` unless an embedding client supplied it.
fn ensure_remote_settings_side_effects(cfg: &mut AgentConfig) {
    crate::agent::config::apply_remote_settings_side_effects(cfg.remote_settings.as_ref());
}

/// Config transform: apply managed settings, resolve storage mode.
fn resolve_config(cfg: &AgentConfig, auth_manager: &AuthManager) -> AgentConfig {
    let mut cfg = cfg.clone();

    if let Ok(layers) = crate::config::ConfigLayers::load()
        && layers.has_managed()
    {
        let origins = crate::config::config_origins(&layers);
        let managed_keys: Vec<&str> = origins
            .iter()
            .filter(|(_, s)| matches!(s, config::ConfigSource::ManagedConfig))
            .map(|(k, _)| k.as_str())
            .collect();
        if !managed_keys.is_empty() {
            tracing::info!(keys = ?managed_keys, "managed_config.toml fields");
        }
    }

    let managed_enforced = crate::config::apply_managed_settings_features(&mut cfg);
    let requirements_enforced = crate::config::apply_requirements(&mut cfg);

    for e in managed_enforced.iter().chain(&requirements_enforced) {
        tracing::info!(field = %e.path, value = %e.value, source = %e.source, "policy override");
    }

    crate::util::config::sync_campaign_fields(&mut cfg);

    // env var > remote settings > Local. Skip remote settings for Generic (gbuild -p, subagents).
    if cfg.storage_mode == StorageMode::Local
        && cfg.mode != crate::agent::config::AgentMode::Generic
    {
        cfg.storage_mode = StorageMode::resolve(None, cfg.remote_settings.as_ref());
    }
    // Writeback talks to the code backend; requires grok.com auth.
    if cfg.storage_mode == StorageMode::Writeback
        && !auth_manager.current().is_some_and(|a| a.is_xai_auth())
    {
        tracing::info!("Writeback is disabled: requires auth with grok.com");
        cfg.storage_mode = StorageMode::Local;
    }

    if let Some(rs) = cfg.remote_settings.as_ref()
        && let Some(v) = rs.path_not_found_hints
    {
        cfg.path_not_found_hints = v;
    }

    cfg
}

/// Initialize process-level singletons (deployment sync, built-in metadata,
/// telemetry). `Once`-guarded: only the first call takes effect.
/// Telemetry user ID is updated separately via [`update_telemetry_config`].
fn init_process(cfg: &AgentConfig, auth_manager: &AuthManager) {
    use std::sync::Once;
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        // Every agent mode (stdio/headless/leader and the in-process TUI
        // agent) passes through here, so diagnostic uploads always carry
        // the version stamp and the resource ceilings in effect.
        gbuild_telemetry::unified_log::set_version(gbuild_version::VERSION);
        crate::util::limits::log_effective_limits();

        if !cfg!(test) {
            // Clear a logged-out team's synced files.
            crate::managed_config::clear_orphan();
        }

        let gbuild_home = crate::util::gbuild_home::gbuild_home();
        crate::builtin::extract_builtin_files(&gbuild_home);

        crate::extensions::marketplace::purge_default_skills_installs(&gbuild_home);

        // Auto-register is gated (default off; env/remote settings enables). Kept out
        // of built-in extraction so the gate can read the resolved
        // remote_settings, which resolve_config has populated by now.
        if cfg.resolve_official_marketplace_auto_register().value {
            crate::extensions::marketplace::ensure_official_marketplace_source(&gbuild_home);
        }

        let telemetry_mode = cfg.resolve_telemetry_mode();
        let feedback = cfg.resolve_feedback();
        tracing::info!(
            telemetry = %telemetry_mode,
            feedback = %feedback,
            "telemetry config resolved"
        );
        update_telemetry_config(cfg);
    });
}

/// Apply the current telemetry mode. Tears down the client when telemetry
/// is disabled, so it's safe to call repeatedly.
pub fn update_telemetry_config(config: &AgentConfig) {
    gbuild_telemetry::client::init(config.resolve_telemetry_mode().value);
}
