// Trying to move to the tower Service
use crate::prelude::{Computation, ComputationError, Task, TaskId, TaskStatus};
// use async_trait::async_trait;
use futures::future::{BoxFuture, FutureExt};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::{
    task::{Context, Poll},
    time::SystemTime,
};
use tower::{Service, ServiceBuilder};

use std::task::{Context, Poll};

struct ComputationService;

struct ServiceRequest = Task<D>

impl Service<Task<TaskData>> for ComputationService {
    type Response = (); // Or a more descriptive type
    type Error = ServiceError;
    type Future = BoxFuture<'static, Result<Self::Response, Self::Error>>;

    fn poll_ready(
        &mut self,
        _cx: &mut Context<'_>,
    ) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, task: Task<TaskData>) -> Self::Future {
        async move {
            // Here, implement your computation logic
            // For example, you could directly call DivisionComputation's logic
            DivisionComputation::compute(&task).await
        }
        .boxed()
    }
}

impl ComputationService {
    async fn compute(task: &Task<TaskData>) -> Result<(), ComputationError> {
        // Your computation logic goes here
    }
}
