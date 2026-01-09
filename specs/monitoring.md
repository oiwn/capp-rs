% Monitoring Setup (Prom/OTLP)

## Direct to Prometheus (v2.52+)
- Requires Prometheus **>= 2.52.0**; OTLP receiver lives on the web port (9090). No extra ports are exposed.
- Run:
  ```
  docker run --rm --name prom-otlp -p 9090:9090 \
    prom/prometheus:v2.52.0 \
    --config.file=/etc/prometheus/prometheus.yml \
    --enable-feature=otlp-write-receiver
  ```
- App endpoint: set `OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:9090/api/v1/otlp` (base URL; exporter adds `/v1/metrics`).
- Verify Prom logs mention `otlp-write-receiver`; check UI at http://localhost:9090. Query `capp_heartbeat_ticks_total` (from `examples/otlp_heartbeat.rs`) or `capp_tasks_processed_total` (from `examples/mailbox_metrics.rs`).

## If Prom OTLP is flaky: use OTel Collector bridge
- Run a collector that listens on OTLP (4318/4317) and exports Prom text on 9090:
  - Use `examples/otel-local.yaml` and run the Just target:
    - `just otelcol-prom`
  - App endpoint: `http://localhost:4318` (base URL). Prom UI remains at http://localhost:9090.

## Emission sanity checks
- Use `examples/otlp_heartbeat.rs` (continuous ticks) and `examples/mailbox_metrics.rs` (short run) under `--features observability`.
- Ensure exporter shutdown is reached (examples call `metrics.shutdown()`).
- If no series appear, re-check: Prom/collector version, correct base endpoint (no `/v1/metrics` suffix), and logs for OTLP receiver startup/errors.
