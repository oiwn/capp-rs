% Updating to 0.6

# Session Context

## Status
- [x] Tower middleware extensibility: mailbox runtime accepts any boxed Tower stack; `build_service_stack` is optional sugar.
- [x] Observability: OTLP metrics via `observability` feature and Prometheus/Grafana ingest.
- [x] Real httpbin example using Tower rate limits + timeouts.

## Tower Middleware Notes (done)
- `spawn_mailbox_runtime` takes a `MailboxService` (`BoxCloneService<ServiceRequest<_, _>, (), BoxError>`), so callers can build their own `ServiceBuilder` chain (retry, timeout, tracing, tower-http).
- `build_service_stack` is a convenience wrapper; custom middleware can be layered before it or by skipping it entirely.
- Doc follow-up: show a custom stack example with extra `Layer`s.

## Observability Notes (done)
- Approach: OpenTelemetry OTLP metrics (HTTP) under `observability` feature; Prometheus/Grafana via OTLP receiver or collector.
- Deps: `opentelemetry`, `opentelemetry_sdk`, `opentelemetry-otlp`.
- `observability::init_metrics(service_name, endpoint?)` installs exporter and returns shutdown handle.
- Emitted metrics: `capp_tasks_processed_total`, `capp_tasks_succeeded_total`, `capp_tasks_failed_total`, `capp_tasks_terminal_failures_total`, `capp_task_latency_ms`, `capp_queue_depth`.
- Example hook exists in `examples/mailbox.rs` with `OTEL_EXPORTER_OTLP_ENDPOINT`.

## Httpbin Example Notes (done)
- `examples/httpbin_tower.rs` uses mailbox runtime + Tower stack (rate limit,
  concurrency limit, buffer, timeout).
- Includes `/delay/*` endpoints to trigger the timeout layer and `/status/500`
  to exercise retries/DLQ behavior.

## Benchmarks
- Microbenchmarks for hot paths using `criterion`: queue push/pop and serializer encode/decode.
- Macrobench for throughput/latency: spawn mailbox runtime, enqueue N tasks, measure RPS plus p50/p95 via histogram.
- Profiling for bottlenecks: `cargo flamegraph`/`perf` and `tokio-console` for async contention.
- Memory: run macrobench under `/usr/bin/time -l` (macOS) or `time -v` (Linux) to record max RSS; optional jemalloc stats later.
