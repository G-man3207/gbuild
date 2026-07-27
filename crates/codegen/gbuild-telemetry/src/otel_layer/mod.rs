//! Shared OpenTelemetry tracing layer.
//!
//! `gbuild-pager` uses this module to install the tracing→OpenTelemetry
//! bridge so session-level spans carry trace/span ids (used for `traceparent`
//! propagation on user-initiated inference requests). The xAI-internal OTLP
//! span firehose to cli-chat-proxy has been removed from this fork: no spans
//! are exported from here. The opt-in external OTEL stream (`crate::external`)
//! is a separate, user-configured pipeline.
use opentelemetry::global;
use opentelemetry::trace::TracerProvider as _;
use opentelemetry_sdk::trace::SdkTracerProvider;
use std::sync::OnceLock;
use tracing_opentelemetry::OpenTelemetryLayer;
use tracing_subscriber::Layer as _;
use tracing_subscriber::registry::LookupSpan;
static TRACER_PROVIDER: OnceLock<SdkTracerProvider> = OnceLock::new();
const ENV_OTEL_FILTER: &str = "GBUILD_OTEL_FILTER";
const DEFAULT_OTEL_FILTER: &str = "info";
/// Static identity of the client emitting telemetry. Becomes resource
/// attributes (`client.name`, `client.version`, `service.version`,
/// `app.entrypoint`) on every span.
#[derive(Debug, Clone, Copy)]
pub struct OtelClientInfo {
    /// Binary name (`gbuild-pager`) -> `client.name`.
    pub client_name: &'static str,
    /// Front-end client version -> `client.version`.
    pub client_version: &'static str,
    /// Engine build (version + commit) -> `service.version`.
    pub service_version: &'static str,
    /// How the session was launched (`cli`/`headless`/`agent`) -> `app.entrypoint`.
    pub app_entrypoint: &'static str,
}
/// Creates an OpenTelemetry layer that bridges tracing spans to OpenTelemetry.
/// This enables trace context propagation; spans are kept in-process only.
///
/// - `client_name`: binary name (e.g. `"gbuild-tui"`, `"gbuild-pager"`) -- stored as
///   `client.name` resource attribute for dashboards to distinguish client types.
/// - `client_version`: `CARGO_PKG_VERSION` -- stored as `client.version`.
/// - `service_version`: `VERSION_WITH_COMMIT` -- stored as `service.version` resource attribute.
pub fn build_otel_layer<S>(
    client: OtelClientInfo,
) -> impl tracing_subscriber::layer::Layer<S>
where
    S: tracing::Subscriber + for<'span> LookupSpan<'span>,
{
    let provider = TRACER_PROVIDER.get_or_init(|| build_tracer_provider(client));
    let tracer = provider.tracer("grok-cli");
    global::set_tracer_provider(provider.clone());
    global::set_text_map_propagator(opentelemetry_sdk::propagation::TraceContextPropagator::new());
    let otel_filter =
        std::env::var(ENV_OTEL_FILTER).unwrap_or_else(|_| DEFAULT_OTEL_FILTER.to_string());
    let otel_filter = tracing_subscriber::filter::EnvFilter::try_new(&otel_filter)
        .unwrap_or_else(|e| {
            eprintln!(
                "[otel] Invalid GBUILD_OTEL_FILTER '{}': {}. Using default '{}'.",
                otel_filter, e, DEFAULT_OTEL_FILTER
            );
            tracing_subscriber::filter::EnvFilter::try_new(DEFAULT_OTEL_FILTER)
                .expect("default otel filter must parse")
        })
        .add_directive(
            "sampling_log=off"
                .parse()
                .expect("static directive must parse"),
        );
    OpenTelemetryLayer::new(tracer)
        .with_context_activation(false)
        .with_filter(otel_filter)
}
fn build_tracer_provider(client: OtelClientInfo) -> SdkTracerProvider {
    let OtelClientInfo {
        client_name,
        client_version,
        service_version,
        app_entrypoint,
    } = client;
    let mut resource_attrs = vec![
        opentelemetry::KeyValue::new("service.version", service_version.to_string()),
        opentelemetry::KeyValue::new("client.name", client_name.to_string()),
        opentelemetry::KeyValue::new("client.version", client_version.to_string()),
        opentelemetry::KeyValue::new("app.entrypoint", app_entrypoint.to_string()),
    ];
    if let Some(terminal_type) = std::env::var("TERM_PROGRAM")
        .ok()
        .or_else(|| std::env::var("TERM").ok())
        .filter(|v| !v.is_empty())
    {
        resource_attrs.push(opentelemetry::KeyValue::new("terminal.type", terminal_type));
    }
    SdkTracerProvider::builder()
        .with_resource(
            opentelemetry_sdk::Resource::builder_empty()
                .with_service_name("grok-cli")
                .with_attributes(resource_attrs)
                .build(),
        )
        .build()
}
/// Flush and shut down the global tracer provider (and the external OTEL
/// stream — both ride the same exit chokepoints).
///
/// Prefer [`OtelGuard`] for normal code paths. Use this directly only in
/// signal handlers or `process::exit` paths where destructors won't run.
/// Safe to call multiple times (second call logs a warning but does not panic;
/// the external shutdown is idempotent).
pub fn shutdown_otel() {
    crate::external::shutdown();
    if let Some(provider) = TRACER_PROVIDER.get()
        && let Err(e) = provider.shutdown()
    {
        tracing::debug!("[otel] Failed to shutdown tracer provider: {}", e);
    }
}
/// RAII guard that calls [`shutdown_otel`] on drop.
pub struct OtelGuard;
impl Drop for OtelGuard {
    fn drop(&mut self) {
        shutdown_otel();
    }
}
/// Create an [`OtelGuard`] that flushes traces on drop.
pub fn otel_guard() -> OtelGuard {
    OtelGuard
}
