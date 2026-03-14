//! Telemetry initialization — tracing subscriber + optional OTLP export.
//!
//! # Environment variables
//!
//! - `SMELT_LOG` — controls console log verbosity (default: `smelt=info`)
//!   Examples: `SMELT_LOG=debug`, `SMELT_LOG=smelt=trace,hyper=off`
//!
//! - `SMELT_OTEL_ENDPOINT` — enables OTLP span export (requires `otel` feature)
//!   Example: `SMELT_OTEL_ENDPOINT=http://localhost:4317`

use tracing_subscriber::{EnvFilter, layer::SubscriberExt, util::SubscriberInitExt};

/// Guard that flushes the OTel provider on drop. Must live until process exit.
pub struct TelemetryGuard {
    #[cfg(feature = "otel")]
    provider: Option<opentelemetry_sdk::trace::SdkTracerProvider>,
}

impl Drop for TelemetryGuard {
    fn drop(&mut self) {
        #[cfg(feature = "otel")]
        if let Some(ref provider) = self.provider {
            if let Err(e) = provider.shutdown() {
                eprintln!("warning: OTel shutdown failed: {e}");
            }
        }
    }
}

fn build_filter() -> EnvFilter {
    EnvFilter::try_from_env("SMELT_LOG").unwrap_or_else(|_| {
        EnvFilter::new("smelt=info")
            .add_directive("hyper=off".parse().unwrap())
            .add_directive("tonic=off".parse().unwrap())
            .add_directive("h2=off".parse().unwrap())
            .add_directive("reqwest=off".parse().unwrap())
    })
}

/// Initialize the tracing subscriber with console output and optional OTLP export.
///
/// Returns a guard that must be held until the process exits to ensure
/// all spans are flushed.
pub fn init() -> TelemetryGuard {
    #[cfg(feature = "otel")]
    {
        if let Ok(endpoint) = std::env::var("SMELT_OTEL_ENDPOINT") {
            return init_with_otel(&endpoint);
        }
    }

    // Default: console only
    tracing_subscriber::registry()
        .with(build_filter())
        .with(
            tracing_subscriber::fmt::layer()
                .with_target(true)
                .with_timer(tracing_subscriber::fmt::time::uptime())
                .compact()
                .with_writer(std::io::stderr),
        )
        .init();

    TelemetryGuard {
        #[cfg(feature = "otel")]
        provider: None,
    }
}

#[cfg(feature = "otel")]
fn init_with_otel(endpoint: &str) -> TelemetryGuard {
    use opentelemetry::KeyValue;
    use opentelemetry::trace::TracerProvider;
    use opentelemetry_otlp::{SpanExporter, WithExportConfig};
    use opentelemetry_sdk::{Resource, trace::SdkTracerProvider};

    let exporter = SpanExporter::builder()
        .with_tonic()
        .with_endpoint(endpoint)
        .build()
        .expect("Failed to create OTLP span exporter");

    let provider = SdkTracerProvider::builder()
        .with_batch_exporter(exporter)
        .with_resource(
            Resource::builder()
                .with_service_name("smelt")
                .with_attributes([KeyValue::new("service.version", env!("CARGO_PKG_VERSION"))])
                .build(),
        )
        .build();

    let otel_layer = tracing_opentelemetry::layer().with_tracer(provider.tracer("smelt"));

    tracing_subscriber::registry()
        .with(build_filter())
        .with(
            tracing_subscriber::fmt::layer()
                .with_target(true)
                .with_timer(tracing_subscriber::fmt::time::uptime())
                .compact()
                .with_writer(std::io::stderr),
        )
        .with(otel_layer)
        .init();

    TelemetryGuard {
        provider: Some(provider),
    }
}
