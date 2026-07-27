//! Ambient session context for telemetry. `session_id` and `turn_number` are
//! injected from the task-local [`TelemetryCtx`] active for the duration of a
//! session.
//!
//! Extracted from `gbuild-shell::agent::telemetry`.

use std::sync::Arc;

use crate::events::TelemetryEvent;

/// Ambient session context for telemetry. Snapshotted synchronously by
/// `log_event` at call time to avoid racing with turn increments.
#[derive(Clone)]
pub struct TelemetryCtx {
    pub session_id: String,
    pub prompt_index: Arc<tokio::sync::Mutex<usize>>,
    /// Per-prompt correlation UUID for the external OTEL stream (`prompt.id`,
    /// events only — never metrics). Set at turn start where `prompt_index`
    /// increments; `None` outside a prompt.
    pub prompt_id: Arc<parking_lot::Mutex<Option<String>>>,
}

impl TelemetryCtx {
    pub fn new(session_id: String, prompt_index: Arc<tokio::sync::Mutex<usize>>) -> Self {
        Self {
            session_id,
            prompt_index,
            prompt_id: Arc::new(parking_lot::Mutex::new(None)),
        }
    }
}

/// Snapshot of the ambient ctx for the external OTEL stream.
pub(crate) struct ExternalCtxSnapshot {
    pub session_id: String,
    pub turn_number: Option<u32>,
    pub prompt_id: Option<String>,
}

/// Rotate the per-prompt correlation UUID at turn start (where
/// `prompt_index` increments). No-op outside a session ctx scope. The id is
/// attached as `prompt.id` to external OTEL events only.
pub fn begin_prompt_id() {
    let _ = TELEMETRY_CTX.try_with(|c| {
        *c.prompt_id.lock() = Some(uuid::Uuid::new_v4().to_string());
    });
}

/// Snapshot the task-local ctx (if any) for external emission. Non-blocking:
/// a contended `prompt_index` lock yields `turn_number = None` rather than
/// stalling the emitting task.
pub(crate) fn external_ctx_snapshot() -> Option<ExternalCtxSnapshot> {
    TELEMETRY_CTX
        .try_with(|c| ExternalCtxSnapshot {
            session_id: c.session_id.clone(),
            turn_number: c.prompt_index.try_lock().map(|g| *g as u32).ok(),
            prompt_id: c.prompt_id.lock().clone(),
        })
        .ok()
}

tokio::task_local! {
    static TELEMETRY_CTX: Arc<TelemetryCtx>;
}

/// The `session_id` field name the debug-log firehose router keys on:
/// `debug_log::SessionIdVisitor` stashes a `SessionId` extension on any span
/// carrying this field — the span *name* is not load-bearing for routing. Shared
/// so the `info_span!` here and the router in `debug_log` can't silently drift; a
/// rename trips `session_span_exposes_router_field` below.
pub(crate) const SESSION_ID_FIELD: &str = "session_id";

/// Build the per-session tracing span the firehose router routes by. The field
/// name MUST be the literal `session_id` (tracing field names can't come from a
/// const); the test below pins it against [`SESSION_ID_FIELD`].
fn session_span(session_id: &str) -> tracing::Span {
    tracing::info_span!("session", session_id = %session_id)
}

/// Run `fut` with telemetry context active. Also sets a `tracing` span.
pub async fn with_session_ctx<F: std::future::Future>(ctx: TelemetryCtx, fut: F) -> F::Output {
    use tracing::Instrument;
    let span = session_span(&ctx.session_id);
    TELEMETRY_CTX
        .scope(Arc::new(ctx), fut.instrument(span))
        .await
}

/// Product analytics event (type-safe). Events route to the external OTEL
/// stream only ("one call site, one sink"): the gate is
/// `external::is_active()`, and the legacy xAI product-events/Mixpanel
/// funnel has been removed from this fork.
pub fn log_event<T: TelemetryEvent>(data: T) {
    crate::external::emit(&data);
}

/// Session lifecycle event (type-safe). Routes to the external OTEL stream
/// only (see [`log_event`]).
pub fn log_session_event<T: TelemetryEvent>(data: T) {
    crate::external::emit(&data);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The debug-log firehose router (`debug_log`) finds the session span by its
    /// `session_id` field (not by name). That field name is a literal in
    /// `session_span` (tracing field names can't be a const), so pin it against the
    /// shared const here — a rename of either breaks this test instead of silently
    /// degrading routing to the per-pid fallback.
    #[test]
    fn session_span_exposes_router_field() {
        // A bare registry enables every callsite, so the span has live metadata.
        let subscriber = tracing_subscriber::registry();
        tracing::subscriber::with_default(subscriber, || {
            let span = session_span("test-id");
            let meta = span
                .metadata()
                .expect("session span must have metadata under an enabling subscriber");
            assert!(
                meta.fields().field(SESSION_ID_FIELD).is_some(),
                "session span must expose `{SESSION_ID_FIELD}` for debug-log routing",
            );
        });
    }
}
