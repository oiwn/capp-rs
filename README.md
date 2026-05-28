![Crates.io Downloads (recent)](https://img.shields.io/crates/dr/capp)
![GitHub License](https://img.shields.io/github/license/oiwn/capp-rs)
[![codecov](https://codecov.io/gh/oiwn/capp-rs/graph/badge.svg?token=36RWDWOO0J)](https://codecov.io/gh/oiwn/capp-rs)
[![dependency status](https://deps.rs/repo/github/oiwn/capp-rs/status.svg)](https://deps.rs/repo/github/oiwn/capp-rs)

# CAPP (Comprehensive Asynchronous Parallel Processing)

Rust library providing a framework for building efficient web crawlers and
asynchronous task processing systems with multiple backend support.

## Features

- **Task Queues**: Support for Fjall (default persistent backend) and in-memory storage
- **Mailbox Runtime**: Tower-native dispatcher/workers with retries, backpressure, and control signals
- **Dead Letter Queue (DLQ)**: Automatic handling of failed tasks
- **Round-Robin Processing**: Fair task distribution across different domains
- **Health Checks**: Built-in monitoring capabilities
- **Flexible Architecture**: Modular design supporting custom task types and processors

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
capp = "0.7"

# Optional features
capp = { version = "0.7", features = ["router"] }
```

## Usage

### Basic Example

```rust
use std::{sync::Arc, time::Duration};
use capp::{
    manager::{MailboxConfig, ServiceRequest, build_service_stack, spawn_mailbox_runtime},
    queue::{InMemoryTaskQueue, JsonSerializer, Task},
};
use serde::{Deserialize, Serialize};
use tower::{BoxError, ServiceBuilder, service_fn};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TaskData {
    value: u32,
}

#[tokio::main]
async fn main() -> Result<(), BoxError> {
    let queue = Arc::new(InMemoryTaskQueue::<TaskData, JsonSerializer>::new());
    let ctx = Arc::new(());

    let inner = ServiceBuilder::new().concurrency_limit(4).service(service_fn(
        |req: ServiceRequest<TaskData, ()>| async move {
            println!("processing {}", req.task.payload.value);
            Ok::<(), BoxError>(())
        },
    ));
    let service = build_service_stack(inner, Default::default());

    let runtime = spawn_mailbox_runtime(
        queue,
        ctx,
        service,
        MailboxConfig {
            dequeue_backoff: Duration::from_millis(25),
            stop_when_idle: true,
            ..Default::default()
        },
    );

    runtime
        .producer
        .enqueue(Task::new(TaskData { value: 42 }))
        .await?;

    runtime.join().await;
    Ok(())
}
```

## Configuration

Configure via TOML files:

```toml
[app]
threads = 4
max_queue = 500

[http]
timeout = 30
connect_timeout = 10

[http.proxy]
use = true
uri = "http://proxy.example.com:8080"
```

## Features

- **http**: HTTP client functionality with proxy support
- **router**: URL classification and routing
- **healthcheck**: System health monitoring
- **cache**: Cache helpers
- **urls**: URL parsing helpers
- **stats-http**: HTTP stats endpoint for mailbox runtime
- **observability**: OTLP metrics export

## License

MIT

## Contributing

Contributions welcome! Please read non-existent guidelines and submit PRs.
