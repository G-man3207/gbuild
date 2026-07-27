//! Core telemetry mode gate.
//!
//! The product-events/Mixpanel funnel to xAI has been removed: this fork
//! performs no unsolicited analytics uploads. What remains is the
//! process-wide [`TelemetryMode`] gate that local consumers (session
//! metrics capture, feedback gating) key on, plus the opt-in external
//! OTEL stream (see [`crate::external`]).

use std::sync::{Mutex, OnceLock};

use crate::config::TelemetryMode;

#[derive(Clone)]
struct TelemetryClient {
    mode: TelemetryMode,
}

static TELEMETRY_CLIENT: OnceLock<Mutex<Option<TelemetryClient>>> = OnceLock::new();

/// Returns `true` when telemetry mode is `Enabled`.
/// Used by `log_event` — product analytics events only fire in `Enabled` mode.
pub fn is_enabled() -> bool {
    TELEMETRY_CLIENT
        .get()
        .and_then(|m| m.lock().ok())
        .is_some_and(|g| g.as_ref().is_some_and(|c| c.mode.is_enabled()))
}

/// Returns `true` when telemetry mode is `Enabled` or `SessionMetrics`.
/// Used by `session_metrics` — lifecycle events fire in both modes.
pub fn is_session_metrics_enabled() -> bool {
    TELEMETRY_CLIENT
        .get()
        .and_then(|m| m.lock().ok())
        .is_some_and(|g| g.as_ref().is_some_and(|c| c.mode.session_metrics_enabled()))
}

/// Install the process-wide telemetry mode. Safe to call multiple times.
///
/// - `Disabled` → no client
/// - `SessionMetrics` → client active (only `session_metrics::*` events fire)
/// - `Enabled` → client active (all events fire)
pub fn init(mode: TelemetryMode) {
    let lock = TELEMETRY_CLIENT.get_or_init(|| Mutex::new(None));
    let mut guard = lock.lock().unwrap_or_else(|err| err.into_inner());
    *guard = if mode.is_disabled() {
        None
    } else {
        Some(TelemetryClient { mode })
    };
}

/// Re-install the telemetry mode if it was not set at startup (e.g. because
/// auth was not yet available). No-op when the client is already set, so safe
/// to call unconditionally after auth succeeds.
pub fn init_if_needed(mode: TelemetryMode) {
    if mode.is_disabled() {
        return;
    }
    let lock = TELEMETRY_CLIENT.get_or_init(|| Mutex::new(None));
    let mut guard = lock.lock().unwrap_or_else(|err| err.into_inner());
    if guard.is_none() {
        *guard = Some(TelemetryClient { mode });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// SessionMetrics must not flip the product-analytics gate.
    #[test]
    fn session_metrics_mode_gates() {
        // Clear the global client even if an assert below panics.
        struct ClearClient;
        impl Drop for ClearClient {
            fn drop(&mut self) {
                let lock = TELEMETRY_CLIENT.get_or_init(|| Mutex::new(None));
                *lock.lock().unwrap_or_else(|err| err.into_inner()) = None;
            }
        }
        let _clear = ClearClient;

        init(TelemetryMode::SessionMetrics);
        assert!(
            is_session_metrics_enabled(),
            "client must be live for session metrics"
        );
        assert!(!is_enabled(), "product analytics must stay off");
    }
}
