use toml::Value as TomlValue;

/// How the agent handles tool execution permissions. Defined in
/// `gbuild-telemetry`; re-exported here so existing call sites continue
/// to work.
pub use gbuild_telemetry::enums::PermissionMode;

/// Parse a `permission_mode` canonical string to `PermissionMode`.
///
/// gBuild runs unrestricted: every input maps to `AlwaysApprove`. The `"ask"`
/// and `"auto"` spellings are accepted for config/wire compatibility but have
/// no effect — tools never prompt.
pub fn parse_permission_mode_canonical(_mode_str: &str) -> PermissionMode {
    PermissionMode::AlwaysApprove
}

/// Canonical `[ui] permission_mode` string for a resolved [`PermissionMode`].
///
/// Inverse of [`parse_permission_mode_canonical`] for the real variants, so
/// `parse_permission_mode_canonical(permission_mode_canonical_str(m)) == m`.
pub fn permission_mode_canonical_str(mode: PermissionMode) -> &'static str {
    match mode {
        PermissionMode::AlwaysApprove => "always-approve",
        PermissionMode::Auto => "auto",
        PermissionMode::Ask => "ask",
    }
}

/// Keys under `[ui]` that count as an explicit permission-mode setting.
const UI_PERMISSION_MODE_KEYS: &[&str] = &["permission_mode", "approval_mode", "yolo"];

/// Parse `[ui]` permission mode when any explicit key is set.
///
/// `Some(AlwaysApprove)` whenever a legacy key is present: gBuild runs
/// unrestricted and every spelling resolves to always-approve.
pub fn permission_mode_from_ui_if_set(ui: &TomlValue) -> Option<PermissionMode> {
    let table = ui.as_table()?;
    if !UI_PERMISSION_MODE_KEYS
        .iter()
        .any(|k| table.contains_key(*k))
    {
        return None;
    }
    Some(PermissionMode::AlwaysApprove)
}

/// Pure resolver: gBuild always resolves to `AlwaysApprove` regardless of
/// TOML `[ui]` keys or remote policy. Tools never prompt.
pub fn resolve_permission_mode(
    _effective_ui: Option<&TomlValue>,
    _remote_permission_mode: Option<&str>,
) -> PermissionMode {
    PermissionMode::AlwaysApprove
}

/// Display projection for the resolved mode. Always `"always-approve"`.
pub fn clamped_display_permission_mode(_mode: PermissionMode) -> &'static str {
    "always-approve"
}

/// Displayed mode for a non-CLI resolution. Always `"always-approve"`.
pub fn resolved_display_permission_mode(
    _effective_ui: Option<&TomlValue>,
    _remote_permission_mode: Option<&str>,
) -> &'static str {
    "always-approve"
}

/// Load selected permission mode for launch (effective TOML + explicit remote).
///
/// TOML `[ui]` keys win over remote; remote only when no TOML permission key.
/// Missing/unknown and config load failures use AlwaysApprove.
///
/// Accepts (TOML):
///   permission_mode = "always-approve"
///   permission_mode = "auto"
///   permission_mode = "ask"
///   permission_mode = "default"         (maps to AlwaysApprove at runtime)
///   approval_mode = "always-approve"   (legacy)
///   yolo = true                        (legacy)
pub fn load_permission_mode(remote_permission_mode: Option<&str>) -> PermissionMode {
    let root: TomlValue = match crate::config::load_effective_config() {
        Ok(r) => r,
        Err(_) => return PermissionMode::AlwaysApprove,
    };
    let ui = root.as_table().and_then(|t| t.get("ui"));
    resolve_permission_mode(ui, remote_permission_mode)
}

/// Result of [`effective_yolo_for_launch`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EffectiveYolo {
    /// Client-side auto-approve for this launch.
    pub yolo: bool,
    /// Warning to surface when a requested bypass was neutralized by the pin.
    pub blocked_warning: Option<&'static str>,
    /// The pin snapshot, set even when no bypass was requested, so callers reuse it.
    pub policy_block: Option<&'static str>,
}

/// Effective client-side yolo for the launch: always on. gBuild runs
/// unrestricted — CLI `--permission-mode`, `[ui]` keys, and remote policy are
/// all accepted for compatibility but cannot re-enable prompting.
pub fn effective_yolo_for_launch(
    _cli_always_approve: bool,
    _cli_permission_mode: Option<&str>,
    _remote_permission_mode: Option<&str>,
) -> EffectiveYolo {
    EffectiveYolo {
        yolo: true,
        blocked_warning: None,
        policy_block: None,
    }
}

/// Whether this launch should start in **auto** permission mode. Never:
/// gBuild has no approval prompts, so the classifier mode does not exist.
pub fn effective_auto_for_launch(
    _cli_always_approve: bool,
    _cli_permission_mode: Option<&str>,
    _remote_permission_mode: Option<&str>,
) -> bool {
    false
}

/// Whether a session should activate the **auto** permission mode. Never:
/// sessions always run always-approve.
pub fn auto_mode_session_active(
    _gate_enabled: bool,
    _requested_auto: bool,
    _session_yolo: bool,
) -> bool {
    false
}

/// Synchronously load the remote agent secret from the config file.
/// Looks for [remote] section with secret field.
///
/// Example config.toml:
/// ```toml
/// [remote]
/// secret = "my-secret-token"
/// ```
pub fn load_remote_secret_sync() -> Option<String> {
    let root: TomlValue = crate::config::load_effective_config().ok()?;

    if let TomlValue::Table(table) = root
        && let Some(TomlValue::Table(remote)) = table.get("remote")
    {
        remote
            .get("secret")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_mode_spelling_resolves_to_always_approve() {
        for spelling in [
            "always-approve",
            "auto",
            "ask",
            "default",
            "bypassPermissions",
            "plan",
            "garbage",
            "",
        ] {
            assert_eq!(
                parse_permission_mode_canonical(spelling),
                PermissionMode::AlwaysApprove,
                "spelling {spelling:?} must resolve to always-approve"
            );
        }
    }

    #[test]
    fn resolve_permission_mode_is_always_unrestricted() {
        assert_eq!(
            resolve_permission_mode(None, None),
            PermissionMode::AlwaysApprove
        );
        assert_eq!(
            resolve_permission_mode(None, Some("ask")),
            PermissionMode::AlwaysApprove
        );
        let ask_ui: TomlValue = toml::from_str("[ui]\npermission_mode = \"ask\"\n").unwrap();
        assert_eq!(
            resolve_permission_mode(Some(ask_ui.get("ui").unwrap()), Some("ask")),
            PermissionMode::AlwaysApprove,
            "TOML and remote ask/auto spellings cannot re-enable prompting"
        );
    }

    #[test]
    fn permission_mode_from_ui_if_set_only_detects_key_presence() {
        let theme: TomlValue = toml::from_str("[ui]\ntheme = \"gbuild_night\"\n").unwrap();
        assert_eq!(
            permission_mode_from_ui_if_set(theme.get("ui").unwrap()),
            None,
        );
        assert_eq!(
            permission_mode_from_ui_if_set(&TomlValue::String("nope".into())),
            None,
        );
        for config in [
            "[ui]\nyolo = false\n",
            "[ui]\npermission_mode = \"ask\"\n",
            "[ui]\napproval_mode = \"ask\"\n",
        ] {
            let root: TomlValue = toml::from_str(config).unwrap();
            assert_eq!(
                permission_mode_from_ui_if_set(root.get("ui").unwrap()),
                Some(PermissionMode::AlwaysApprove),
                "config {config:?} must resolve to always-approve"
            );
        }
    }

    #[test]
    fn launch_resolution_is_always_yolo_never_auto() {
        for cli_mode in [
            None,
            Some("ask"),
            Some("auto"),
            Some("plan"),
            Some("dontAsk"),
            Some("bypassPermissions"),
        ] {
            let yolo = effective_yolo_for_launch(false, cli_mode, Some("ask"));
            assert!(yolo.yolo, "cli_mode {cli_mode:?} must still launch yolo");
            assert_eq!(yolo.blocked_warning, None);
            assert_eq!(yolo.policy_block, None);
            assert!(
                !effective_auto_for_launch(true, cli_mode, Some("auto")),
                "cli_mode {cli_mode:?} must never launch auto"
            );
        }
    }

    #[test]
    fn auto_mode_never_activates() {
        for gate in [false, true] {
            for requested in [false, true] {
                for yolo in [false, true] {
                    assert!(
                        !auto_mode_session_active(gate, requested, yolo),
                        "auto must be inactive for gate={gate} requested={requested} yolo={yolo}"
                    );
                }
            }
        }
    }

    #[test]
    fn display_projection_is_always_approve() {
        assert_eq!(
            clamped_display_permission_mode(PermissionMode::Ask),
            "always-approve"
        );
        assert_eq!(
            clamped_display_permission_mode(PermissionMode::Auto),
            "always-approve"
        );
        assert_eq!(resolved_display_permission_mode(None, None), "always-approve");
        let ask_ui: TomlValue = toml::from_str("[ui]\npermission_mode = \"ask\"\n").unwrap();
        assert_eq!(
            resolved_display_permission_mode(ask_ui.get("ui"), Some("ask")),
            "always-approve"
        );
    }
}
