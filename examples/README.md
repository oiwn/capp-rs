# Examples

Most examples live in this directory and are run from the `capp` crate:
```sh
cargo run -p capp --example <name>
```

## basic.rs
Minimal mailbox runtime example using an in-memory queue and a simple division
task flow (uses `tests/simple_config.toml`).
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

## local_blog_crawl.rs
Mailbox crawler demo against a generated local blog fixture with 100 posts.
Uses `capp-testkit` and verifies crawl coverage via the fixture stats endpoint.
```sh
cargo run -p capp --features http --example local_blog_crawl
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
Example assets remain in `examples/hackernews/`, but the old crawler entrypoint
was removed with the legacy worker runtime and needs a mailbox-based rewrite
before it can return as a runnable example.

## otel-local.yaml
Local OpenTelemetry Collector config (OTLP -> Prometheus).
```sh
just otelcol-prom
```
