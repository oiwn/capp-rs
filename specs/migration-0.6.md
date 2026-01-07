% Migration to 0.6

## Who should read this
- Users upgrading from 0.5.x to 0.6.x.
- Anyone moving from the legacy `WorkersManager` flow to the new mailbox runtime.

## Headline changes
- Config is TOML-only (YAML is no longer supported).
- New mailbox runtime uses Tower middleware for rate limiting, timeouts, retries,
  and buffering.
- Fjall is the default queue backend in `capp-queue` (disable if you only want
  in-memory queues).

## Migration checklist
1) **Update your dependency version**
   ```toml
   [dependencies]
   capp = "0.6"
   ```

2) **Move config files to TOML**
   - Rename any `.yaml`/`.yml` configs to `.toml`.
   - Update syntax to TOML (tables with `[section]`, arrays with `[a, b]`).
   - Re-check optional HTTP/proxy keys, which are now TOML-driven.

3) **Adopt the mailbox runtime (recommended)**
   - Create a Tower `Service` that accepts `ServiceRequest<T, Ctx>`.
   - Use `ServiceBuilder` (or `build_service_stack`) to add timeout and
     concurrency controls.
   - Start workers via `spawn_mailbox_runtime`.

   Minimal skeleton:
   ```rust
   use std::{sync::Arc, time::Duration};
   use capp::{
       manager::{MailboxConfig, ServiceRequest, spawn_mailbox_runtime},
       queue::{InMemoryTaskQueue, JsonSerializer, Task},
   };
   use tower::{BoxError, ServiceBuilder, service_fn, util::BoxCloneService};

   let queue = Arc::new(InMemoryTaskQueue::<MyTask, JsonSerializer>::new());
   let ctx = Arc::new(());

   let base = service_fn(|req: ServiceRequest<MyTask, ()>| async move {
       // handle req.task
       Ok::<(), BoxError>(())
   });

   let service = ServiceBuilder::new()
       .timeout(Duration::from_secs(5))
       .service(base);
   let service = BoxCloneService::new(service);

   let runtime = spawn_mailbox_runtime(
       queue,
       ctx,
       service,
       MailboxConfig::default(),
   );
   ```

4) **Keep `WorkersManager` only if you need it**
   - The legacy `Computation` flow remains available for now.
   - Prefer mailbox + Tower for new work to take advantage of rate limits and
     timeouts.

5) **Optional: disable Fjall if you don't need it**
   - For a custom setup, depend on `capp-queue` directly with
     `default-features = false`, then enable the backend you want.

## Example updates
- New HTTPBin mailbox demo with Tower rate limiting and timeouts:
  `cargo run -p capp --features http --example httpbin_tower`.
- Existing mailbox examples continue to work, now with explicit Tower stacks.
