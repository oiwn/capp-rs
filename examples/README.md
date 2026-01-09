# Examples

Most examples live in this directory and are run from the `capp` crate:
```sh
cargo run -p capp --example <name>
```

## basic.rs
In-memory queue + `WorkersManager` with a simple task that fails when values
are not divisible by 3 (uses `tests/simple_config.toml`).
```sh
cargo run -p capp --example basic
```

## mailbox.rs
Mailbox runtime with retries, stats logging, and graceful Ctrl+C shutdown.
Optional metrics when compiled with the `observability` feature.
```sh
cargo run -p capp --example mailbox
```
```sh
cargo run -p capp --features observability --example mailbox
```

## mailbox_metrics.rs
Mailbox demo that emits OTLP metrics. Requires `--features observability`.
```sh
OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:9090/api/v1/otlp \
cargo run -p capp --features observability --example mailbox_metrics
```
Use `just prom-otlp` or `examples/otel-local.yaml` to stand up a local receiver.

## mailbox_stats_http.rs
Mailbox demo with a live stats endpoint at `http://127.0.0.1:8080/stats`.
Runs indefinitely; stop with Ctrl+C.
```sh
cargo run -p capp --features stats-http --example mailbox_stats_http
```

## httpbin_tower.rs
Mailbox demo that fetches HTTPBin endpoints with Tower rate limiting + timeouts.
Uses `/delay/*` endpoints to trigger the timeout layer.
```sh
cargo run -p capp --features http --example httpbin_tower
```

## otlp_heartbeat.rs
Emits a heartbeat metric to verify OTLP ingestion. Requires `observability`.
Defaults to `http://127.0.0.1:4318` (override with `OTEL_EXPORTER_OTLP_ENDPOINT`).
```sh
cargo run -p capp --features observability --example otlp_heartbeat
```

## urls.rs
URL middleware demo (dedupe + pattern filter) using `capp-urls`.
```sh
cargo run -p capp --example urls
```

## hackernews/
Hacker News crawler using `examples/hackernews/hn_config.toml` and
`examples/hackernews/hn_uas.txt`. Performs real network requests and writes
pages under `target/tmp`.
To run, re-enable the example entry in `capp/Cargo.toml`, then:
```sh
cargo run -p capp --features http --example hackernews
```

## otel-local.yaml
Local OpenTelemetry Collector config (OTLP -> Prometheus).
```sh
just otelcol-prom
```
