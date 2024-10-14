/* use async_trait::async_trait;
use new::{Middleware, RequestHandler, TaskStorage, Worker};
use std::{collections::VecDeque, io, sync::Arc};
use tokio::{
    self,
    sync::Mutex as AsyncMutex,
    time::{self, Duration},
}; */

mod new;

#[tokio::main]
async fn main() {
    println!("Such main.rs much of dead code. Wow.");
}

/*
struct TaskHandler;

#[async_trait]
impl RequestHandler for TaskHandler {
    type Req = Task;
    type Res = String;
    type Error = io::Error;

    async fn handle(&self, req: &Self::Req) -> Result<Self::Res, Self::Error> {
        time::sleep(Duration::from_secs(1)).await;
        if req.content == "error" {
            Err(io::Error::new(io::ErrorKind::Other, "Error triggered"))
        } else {
            Ok(format!("Processed: {}", req.content))
        }
    }
}

struct LoggingMiddleware;

#[async_trait]
impl<T> Middleware<T> for LoggingMiddleware
where
    T: RequestHandler + Sync + Send,
{
    async fn process(&self, handler: &T, req: T::Req) -> Result<T::Res, T::Error> {
        println!("Request started: {:?}", req);
        let result = handler.handle(&req).await;
        println!("Request completed: {:?}", result);
        result
    }
}

// Define a generic Task that can be serialized and deserialized
#[derive(Clone, Debug)]
struct Task {
    id: u32,
    content: String,
}

struct InMemoryTaskStorage {
    tasks: Arc<AsyncMutex<VecDeque<Task>>>,
}

impl InMemoryTaskStorage {
    fn new() -> Self {
        InMemoryTaskStorage {
            tasks: Arc::new(AsyncMutex::new(VecDeque::new())),
        }
    }

    async fn add_task(&self, task: Task) {
        let mut lock = self.tasks.lock().await;
        lock.push_back(task);
    }
}

#[async_trait]
impl TaskStorage for InMemoryTaskStorage {
    type Task = Task;
    type Error = &'static str;

    async fn get(&self) -> Result<Self::Task, Self::Error> {
        let mut tasks = self.tasks.lock().await;
        tasks.pop_front().ok_or("No tasks available")
    }

    async fn ack(&self, task: &Self::Task) -> Result<(), Self::Error> {
        println!("Acknowledging task: {:?}", task);
        Ok(())
    }
}

#[tokio::main]
async fn main() {
    let storage = InMemoryTaskStorage::new();
    storage
        .add_task(Task {
            id: 1,
            content: "Handle important request".to_string(),
        })
        .await;
    storage
        .add_task(Task {
            id: 2,
            content: "Handle another important request".to_string(),
        })
        .await;

    let handler = TaskHandler;
    let worker = Worker { storage, handler };

    match worker.run().await {
        Ok(_) => println!("Worker terminated successfully."),
        Err(e) => println!("Worker terminated with an error: {:?}", e),
    }
}
*/
