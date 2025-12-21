//! Emits simple OTLP metrics in a loop so you can verify Prometheus OTLP ingest.
//!
//! Quickstart:
//! ```
//! # Start Prometheus with OTLP write receiver enabled (default ports):
//! just prom-otlp
//!
//! # Run this emitter (pushes to http://127.0.0.1:4318 by default):
//! cargo run -p capp --features observability --example otlp_heartbeat
//!
//! # In Prom UI (http://localhost:9090), query:
//! #   capp_heartbeat_ticks_total
//! #   capp_heartbeat_latency_ms_bucket
//! ```

#![cfg(feature = "observability")]

use std::time::Duration;

use capp::{observability, tracing, tracing_subscriber};
use opentelemetry::{KeyValue, global};
use rand::{Rng, rng};
use tokio::{signal, time};

const OTLP_ENDPOINT: &str = "http://127.0.0.1:4318";

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let metrics =
        observability::init_metrics("capp-otlp-heartbeat", Some(OTLP_ENDPOINT))?;

    let meter = global::meter("capp.heartbeat");
    let counter = meter
        .u64_counter("capp_heartbeat_ticks_total")
        .with_description("Heartbeat ticks emitted for OTLP verification")
        .build();
    let latency = meter
        .f64_histogram("capp_heartbeat_latency_ms")
        .with_unit("ms")
        .with_description("Fake latency samples for OTLP verification")
        .build();

    let mut rng = rng();
    let mut ticker = time::interval(Duration::from_secs(1));

    tracing::info!("emitting heartbeat metrics; press Ctrl+C to stop");
    loop {
        tokio::select! {
            _ = ticker.tick() => {
                let jitter_ms: f64 = rng.random_range(5.0..50.0);
                counter.add(1, &[KeyValue::new("kind", "heartbeat")]);
                latency.record(jitter_ms, &[KeyValue::new("kind", "heartbeat")]);
                tracing::info!(jitter_ms, "tick");
            }
            _ = signal::ctrl_c() => {
                tracing::info!("received Ctrl+C, flushing metrics and exiting");
                break;
            }
        }
    }

    metrics.shutdown();
    Ok(())
}
