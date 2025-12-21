% Updating to 0.6

# Boss thoughs for this session:


## User should be able to define middlewares for tower service

We need to check and make sure user of library would be able to define own middlewares for tower, and assign new, like timeout, retries (if needed), all tower could provide. Do research about this subject and report back in section below.

### Report (fill this section with your findings)
- Current mailbox runtime already accepts any Tower stack: `spawn_mailbox_runtime` takes `MailboxService = BoxCloneService<ServiceRequest<D, Ctx>, (), BoxError>`, so callers can compose their own `ServiceBuilder`/`Layer` chain (retry, timeout, tracing, tower-http, etc.) and pass it directly; `build_service_stack` is just a convenience.
- The helper `build_service_stack` hardcodes `ConcurrencyLimitLayer -> load_shed -> buffer -> timeout` with only `ServiceStackOptions` knobs, so attaching extra middleware requires wrapping the base service before handing it to the helper or skipping it and building/boxing the stack manually.
- Follow-ups to make this obvious: add docs/example showing a custom stack with `Retry`/`Trace` layers boxed into `MailboxService`, and consider an overload that applies the default layers then lets the caller inject additional `Layer`s (or a `Fn(ServiceBuilder) -> ServiceBuilder`) to keep ergonomics high while preserving our defaults.

## Need to figure out deps for observability

I would like to use Prometheus for metrics in my cluster and Graphana. Need something compatible, is it possible to have few sources with Graphana? k3s will use prometheus which i could observe using Graphana. How we could organize metrics for capp-rs? I remember one can not push events into Prometheus there is thing like Gateway to pypass it. Research options and return back with report.

### Observability todo
- Approach chosen: OpenTelemetry OTLP metrics (HTTP) behind `observability` feature; Prometheus/Grafana ingest via OTLP endpoint or collector.
- Deps (capp, optional): `opentelemetry`, `opentelemetry_sdk`, `opentelemetry-otlp`.
- Wiring: `observability::init_metrics(service_name, endpoint?)` installs OTLP exporter (default `http://127.0.0.1:4318/v1/metrics`), sets global meter; returns shutdown handle.
- Emitted metrics via mailbox stats: counters `capp_tasks_processed_total`, `capp_tasks_succeeded_total`, `capp_tasks_failed_total`, `capp_tasks_terminal_failures_total`; histogram `capp_task_latency_ms`; gauge `capp_queue_depth`.
- Example hook: `examples/mailbox.rs` calls `init_metrics`, reads `OTEL_EXPORTER_OTLP_ENDPOINT` if set; smoke test under feature flag.

### Need to write real example.

Which could use httpbin service as target to fetch different pages and demonstrate how it could act in real world example, after it done it should provide report with data.

Make detailed plan about how this example should work (if should fit 1 file btw and just fjall as queue backend).

### Plan (write plan here)
- Single-file example `examples/httpbin_mailbox.rs` using fjall queue (`FjallTaskQueue<_, JsonSerializer>::open(path)`) and the mailbox runtime.
- Task payload `FetchJob { path: String }` with base URL configurable via env (default `https://httpbin.org`). Seed tasks like `/get?i=n`, `/uuid`, `/delay/2`, `/status/500` to exercise success, latency, and errors.
- Build one `reqwest::Client` in context; base `service_fn` clones it, builds URL, times the request, and sends `FetchResult { url, status, latency_ms, body_len, error }` over a channel for reporting.
- Compose Tower stack with `ServiceBuilder`: load-shed + buffer (e.g., 32) + concurrency limit (e.g., 8) + timeout (e.g., 5s) plus a retry layer (`tower::retry::Retry` with exponential/backoff policy from `capp_config::backoff`), then box into `MailboxService`.
- Configure `MailboxConfig` (worker_count ~4, max_retries 2–3, short dequeue_backoff), enqueue the tasks, wait for all results/stat snapshots, then `shutdown`.
- Aggregate results into a summary printed to stdout (counts by status, failures with errors/attempts, average latency; optionally emit a tiny CSV/JSON snippet to mimic a “report with data”).
