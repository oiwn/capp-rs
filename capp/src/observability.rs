use std::time::Duration;

use opentelemetry::{KeyValue, global};
use opentelemetry_otlp::{ExportConfig, WithExportConfig};
use opentelemetry_sdk::{
    Resource,
    metrics::{PeriodicReader, SdkMeterProvider},
};

/// Handle returned by `init_metrics` so callers can flush on shutdown.
pub struct MetricsHandle {
    provider: Option<SdkMeterProvider>,
}

impl MetricsHandle {
    /// Flush buffered metrics.
    pub fn shutdown(mut self) {
        if let Some(provider) = self.provider.take() {
            let _ = provider.shutdown();
        }
    }
}

impl Drop for MetricsHandle {
    fn drop(&mut self) {
        if let Some(provider) = self.provider.take() {
            let _ = provider.shutdown();
        }
    }
}

/// Initialize OTLP HTTP metrics exporter and set the global meter provider.
///
/// `endpoint` defaults to `http://127.0.0.1:4318/v1/metrics` if `None`.
pub fn init_metrics(
    service_name: &str,
    endpoint: Option<&str>,
) -> anyhow::Result<MetricsHandle> {
    let exporter = opentelemetry_otlp::MetricExporter::builder()
        .with_http()
        .with_export_config(ExportConfig {
            endpoint: Some(
                endpoint
                    // Expect base OTLP endpoint (no /v1/metrics suffix); defaults to local OTLP port.
                    .unwrap_or("http://127.0.0.1:4318")
                    .to_string(),
            ),
            ..Default::default()
        })
        .build()?;

    let reader = PeriodicReader::builder(exporter)
        .with_interval(Duration::from_secs(1))
        .build();
    let resource = Resource::builder()
        .with_service_name(service_name.to_string())
        .with_attributes([KeyValue::new("library", "capp")])
        .build();

    let provider = SdkMeterProvider::builder()
        .with_reader(reader)
        .with_resource(resource)
        .build();

    global::set_meter_provider(provider.clone());

    Ok(MetricsHandle {
        provider: Some(provider),
    })
}
