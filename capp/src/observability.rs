use opentelemetry::{KeyValue, global};
use opentelemetry_otlp::{ExportConfig, WithExportConfig};
use opentelemetry_sdk::{
    Resource,
    metrics::{PeriodicReader, SdkMeterProvider},
    runtime,
};

/// Handle returned by `init_metrics` so callers can flush on shutdown.
pub struct MetricsHandle {
    provider: SdkMeterProvider,
}

impl MetricsHandle {
    /// Flush buffered metrics.
    pub fn shutdown(self) {
        let _ = self.provider.shutdown();
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
            endpoint: endpoint
                .unwrap_or("http://127.0.0.1:4318/v1/metrics")
                .to_string(),
            ..Default::default()
        })
        .build()?;

    let reader = PeriodicReader::builder(exporter, runtime::Tokio).build();
    let resource = Resource::builder()
        .with_service_name(service_name)
        .with_attributes([KeyValue::new("library", "capp")])
        .build();

    let provider = SdkMeterProvider::builder()
        .with_reader(reader)
        .with_resource(resource)
        .build();

    global::set_meter_provider(provider.clone());

    Ok(MetricsHandle { provider })
}
