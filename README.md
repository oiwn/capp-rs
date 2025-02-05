![Crates.io Downloads (recent)](https://img.shields.io/crates/dr/capp-rs)
![GitHub License](https://img.shields.io/github/license/oiwn/capp-rs)
[![codecov](https://codecov.io/gh/oiwn/capp-rs/graph/badge.svg?token=36RWDWOO0J)](https://codecov.io/gh/oiwn/capp-rs)
[![dependency status](https://deps.rs/repo/github/oiwn/capp-rs/status.svg)](https://deps.rs/repo/github/oiwn/capp-rs)

# CAPP (Comprehensive Asynchronous Parallel Processing)

Rust library providing a framework for building efficient web crawlers and
asynchronous task processing systems with multiple backend support.

## Features

- **Multi-Backend Task Queues**: Support for Redis, MongoDB, PostgreSQL, and in-memory storage
- **Configurable Workers**: Process tasks concurrently with customizable retry logic and timeouts
- **Dead Letter Queue (DLQ)**: Automatic handling of failed tasks
- **Round-Robin Processing**: Fair task distribution across different domains
- **Health Checks**: Built-in monitoring capabilities
- **Flexible Architecture**: Modular design supporting custom task types and processors

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
capp = "0.4.5"

# Optional features
capp = { version = "0.4.5", features = ["redis", "mongodb", "postgres"] }
```

## Usage

### Basic Example

```rust
use capp::prelude::*;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TaskData {
    value: u32,
}

struct MyComputation;

#[async_trait]
impl Computation<TaskData, Context> for MyComputation {
    async fn call(
        &self,
        worker_id: WorkerId,
        ctx: Arc<Context>,
        queue: AbstractTaskQueue<TaskData>,
        task: &mut Task<TaskData>,
    ) -> Result<(), ComputationError> {
        // Process task here
        Ok(())
    }
}

#[tokio::main]
async fn main() {
    let storage = InMemoryTaskQueue::new();
    let computation = MyComputation {};
    
    let options = WorkersManagerOptionsBuilder::default()
        .concurrency_limit(4)
        .build()
        .unwrap();

    let mut manager = WorkersManager::new(ctx, computation, storage, options);
    manager.run_workers().await;
}
```

### Redis Queue Example

```rust
use capp::prelude::*;

let client = Client::connect("redis://localhost:6379").await?;
let queue = RedisTaskQueue::new(client, "my-queue").await?;
```

## Configuration

Configure via YAML files:

```yaml
app:
  threads: 4
  max_queue: 500

http:
  proxy:
    use: true
    uri: "http://proxy.example.com:8080"
  timeout: 30
  connect_timeout: 10
```

## Features

- **http**: HTTP client functionality with proxy support
- **redis**: Redis backend support
- **mongodb**: MongoDB backend support
- **postgres**: PostgreSQL backend support
- **router**: URL classification and routing
- **healthcheck**: System health monitoring

## License

MIT

## Contributing

Contributions welcome! Please read our contributing guidelines and submit PRs.
