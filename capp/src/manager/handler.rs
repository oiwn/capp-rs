//! This module defines the `TaskHandler` trait, which provides a generic interface
//! for asynchronous task processing. It allows for flexible implementation of
//! request handling with associated types for requests, responses, and errors.

use crate::prelude::WorkerId;
use async_trait::async_trait;
use capp_queue::queue::AbstractTaskQueue;
use capp_queue::task::Task;
use std::sync::Arc;

/// A trait for handling asynchronous tasks or requests.
///
/// This trait uses associated types to allow for flexible implementations
/// with different request, response, and error types.
///
/// # Type Parameters
///
/// * `Req`: The type of the request or task to be handled.
/// * `Res`: The type of the response returned after handling the request.
/// * `Error`: The type of error that can occur during request handling.
///
/// # Examples
///
/// ```no_run
/// use async_trait::async_trait;
///
/// struct MyHandler;
///
/// #[async_trait]
/// impl TaskHandler for MyHandler {
///     type Req = String;
///     type Res = usize;
///     type Error = std::io::Error;
///
///     async fn handle(&self, req: &Self::Req) -> Result<Self::Res, Self::Error> {
///         Ok(req.len())
///     }
/// }
/// ```
#[async_trait]
pub trait TaskHandler {
    type Req;
    type Res;
    type Error;

    /// Handles a request asynchronously.
    ///
    /// This method takes a reference to a request of type `Req` and returns
    /// a `Result` containing either a response of type `Res` or an error of type `Error`.
    ///
    /// # Arguments
    ///
    /// * `req`: A reference to the request to be handled.
    ///
    /// # Returns
    ///
    /// Returns a `Result<Self::Res, Self::Error>` which is:
    /// - `Ok(res)` containing the response if the request was handled successfully.
    /// - `Err(error)` if an error occurred during request handling.
    async fn handle(&self, req: &Self::Req) -> Result<Self::Res, Self::Error>;
}

#[async_trait::async_trait]
pub trait RequestBuilder<D, Ctx, Req>
where
    D: std::fmt::Debug + Clone,
{
    async fn build_request(
        worker_id: WorkerId,
        ctx: Arc<Ctx>,
        queue: AbstractTaskQueue<D>,
        task: &Task<D>,
    ) -> Req;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;

    struct MyTaskHandler;

    #[derive(Debug, PartialEq)]
    struct MyRequest {
        content: String,
    }

    #[async_trait]
    impl TaskHandler for MyTaskHandler {
        type Req = MyRequest;
        type Res = String;
        type Error = io::Error;

        async fn handle(&self, req: &Self::Req) -> Result<Self::Res, Self::Error> {
            if req.content == "error" {
                Err(io::Error::new(io::ErrorKind::Other, "Error triggered"))
            } else {
                Ok(format!("Processed: {}", req.content))
            }
        }
    }

    #[tokio::test]
    async fn test_successful_handle() {
        let handler = MyTaskHandler;
        let request = MyRequest {
            content: "test content".to_string(),
        };

        let result = handler.handle(&request).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "Processed: test content");
    }

    #[tokio::test]
    async fn test_error_handle() {
        let handler = MyTaskHandler;
        let request = MyRequest {
            content: "error".to_string(),
        };

        let result = handler.handle(&request).await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::Other);
    }

    #[tokio::test]
    async fn test_multiple_requests() {
        let handler = MyTaskHandler;
        let requests = vec![
            MyRequest {
                content: "first".to_string(),
            },
            MyRequest {
                content: "second".to_string(),
            },
            MyRequest {
                content: "third".to_string(),
            },
        ];

        for request in requests {
            let result = handler.handle(&request).await;
            assert!(result.is_ok());
            assert_eq!(result.unwrap(), format!("Processed: {}", request.content));
        }
    }
}
