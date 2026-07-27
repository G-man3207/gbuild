//! Telemetry-engine configuration.
//!
//! Extracted from `gbuild-shell::agent::config` so the data-collector
//! engine can construct a [`TelemetryClient`](crate::client::TelemetryClient)
//! without a build-time dependency on the shell.
//!
//! Shell still re-exports these types from their original paths so existing
//! call sites (and `Config` derive impls) compile unchanged.
use serde::{Deserialize, Serialize};
/// Telemetry mode: `true`/`false` (legacy bool) or `"session_metrics"` (string).
///
/// - `Disabled` -- nothing sent (enterprise default)
/// - `SessionMetrics` -- metadata-only lifecycle events, no content
/// - `Enabled` -- full product telemetry (events + Mixpanel)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TelemetryMode {
    #[default]
    Disabled,
    SessionMetrics,
    Enabled,
}
impl TelemetryMode {
    pub fn is_disabled(&self) -> bool {
        matches!(self, Self::Disabled)
    }
    pub fn is_enabled(&self) -> bool {
        matches!(self, Self::Enabled)
    }
    /// True for both `SessionMetrics` and `Enabled`.
    pub fn session_metrics_enabled(&self) -> bool {
        matches!(self, Self::SessionMetrics | Self::Enabled)
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" | "on" | "enabled" | "full" => Some(Self::Enabled),
            "0" | "false" | "no" | "off" | "disabled" => Some(Self::Disabled),
            "session-metrics" | "session_metrics" => Some(Self::SessionMetrics),
            _ => None,
        }
    }
}
impl std::fmt::Display for TelemetryMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Disabled => write!(f, "false"),
            Self::SessionMetrics => write!(f, "session_metrics"),
            Self::Enabled => write!(f, "true"),
        }
    }
}
impl From<bool> for TelemetryMode {
    fn from(b: bool) -> Self {
        if b { Self::Enabled } else { Self::Disabled }
    }
}
impl serde::Serialize for TelemetryMode {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Disabled => serializer.serialize_bool(false),
            Self::Enabled => serializer.serialize_bool(true),
            Self::SessionMetrics => serializer.serialize_str("session_metrics"),
        }
    }
}
/// Wire format for `[features] telemetry`: accepts `true`, `false`, or `"session_metrics"`.
#[derive(serde::Deserialize)]
#[serde(untagged)]
enum TelemetryModeValue {
    Bool(bool),
    Str(String),
}
impl<'de> serde::Deserialize<'de> for TelemetryMode {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        match TelemetryModeValue::deserialize(deserializer)? {
            TelemetryModeValue::Bool(b) => Ok(Self::from(b)),
            TelemetryModeValue::Str(s) => Ok(Self::parse(&s).unwrap_or_else(|| {
                tracing::warn!(
                    value = %s,
                    "TELEMETRY_MODE_UNKNOWN: unrecognized telemetry mode; treating as disabled",
                );
                Self::Disabled
            })),
        }
    }
}
/// Parse an env var as a `TelemetryMode`. Returns `None` if unset or empty.
pub fn env_telemetry_mode(name: &str) -> Option<TelemetryMode> {
    let value = std::env::var(name).ok()?;
    TelemetryMode::parse(&value)
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct TelemetryConfig {
    /// Declared for `serde_ignored`. Actual toggle is `[features] telemetry`.
    #[serde(default)]
    pub enabled: Option<bool>,
    /// External OTEL master switch (`= GBUILD_EXTERNAL_OTEL`, env wins).
    pub otel_enabled: Option<bool>,
    /// External OTEL metrics exporter: `otlp` | `console` | `none`.
    pub otel_metrics_exporter: Option<String>,
    /// External OTEL logs/events exporter: `otlp` | `console` | `none`.
    pub otel_logs_exporter: Option<String>,
    /// External OTLP base endpoint (`/v1/logs`, `/v1/metrics` appended for HTTP).
    pub otel_endpoint: Option<String>,
    /// External OTLP transport: `http/protobuf` | `grpc`.
    #[serde(alias = "otel_transport")]
    pub otel_protocol: Option<String>,
    /// External OTEL content gate (admins can pin to `false` via requirements).
    pub otel_log_user_prompts: Option<bool>,
    /// External OTEL content gate (admins can pin to `false` via requirements).
    pub otel_log_tool_details: Option<bool>,
}
impl TelemetryConfig {
    pub fn apply_env_overrides(&mut self) {}
}
/// Derive a stable deployment ID (UUIDv5) from the deployment key.
pub fn deployment_id_from_key(key: &str) -> String {
    uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_OID, key.as_bytes()).to_string()
}
