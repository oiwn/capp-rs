//! # CAPP - "Comprehensive Asynchronous Parallel Processing" or just "Crawler APP"
//!
//! `capp` is a Rust library designed to provide powerful and flexible tools for building efficient web crawlers and other asynchronous, parallel processing applications. It offers a robust framework for managing concurrent tasks, handling network requests, and processing large amounts of data in a scalable manner.
//!
//! ## Features
//!
//! - **Asynchronous Task Management**: Utilize tokio-based asynchronous processing for efficient, non-blocking execution of tasks.
//! - **Flexible Task Queue**: Implement various backend storage options for task queues, including in-memory and Redis-based solutions.
//! - **Round-Robin Task Distribution**: Ensure fair distribution of tasks across different domains or categories.
//! - **Configurable Workers**: Set up and manage multiple worker instances to process tasks concurrently.
//! - **Error Handling and Retry Mechanisms**: Robust error handling with configurable retry policies for failed tasks.
//! - **Dead Letter Queue (DLQ)**: Automatically move problematic tasks to a separate queue for later analysis or reprocessing.
//! - **Health Checks**: Built-in health check functionality to ensure the stability of your crawling or processing system.
//! - **Extensible Architecture**: Easily extend the library with custom task types, processing logic, and storage backends.
//!
//! ## Use Cases
//!
//! While `capp` is primarily designed for building web crawlers, its architecture makes it suitable for a variety of parallel processing tasks, including:
//!
//! - Web scraping and data extraction
//! - Distributed task processing
//! - Batch job management
//! - Asynchronous API clients
//! - Large-scale data processing pipelines
//!
//! ## Getting Started
//!
//! To use `capp` in your project, add it to your `Cargo.toml`:
//!
//! ```toml
//! [dependencies]
//! capp = "0.5"
//! ```
//!
//! Check examples!
//!
//! ## Modules
//!
//! - `config`: Configuration management for your application.
//! - `healthcheck`: Functions for performing health checks on your system.
//! - `http`: Utilities for making HTTP requests and handling responses.
//! - `manager`: Task and worker management structures.
//! - `queue`: Task queue implementations and traits.
//! - `task`: Definitions and utilities for working with tasks.
pub mod manager;
pub mod prelude;
#[cfg(feature = "cache")]
pub use capp_cache as cache;
pub use capp_config as config;
#[cfg(feature = "http")]
pub use capp_config::backoff;
pub use capp_queue as queue;
#[cfg(feature = "router")]
pub use capp_router as router;
#[cfg(feature = "http")]
pub use config::http;
#[cfg(feature = "observability")]
pub mod observability;
#[cfg(feature = "mongodb")]
pub use mongodb;
#[cfg(feature = "http")]
pub use reqwest;
#[cfg(feature = "redis")]
pub use rustis;
pub use tower;
// re-export
pub use async_trait;
pub use rand;
pub use serde;
pub use serde_json;
pub use thiserror;
pub use toml;
pub use tracing;
pub use tracing_subscriber;
pub use uuid;
