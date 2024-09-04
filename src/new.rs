use async_trait::async_trait;
use std::fmt::Debug;

#[async_trait]
pub trait RequestHandler {
    type Req: Debug + Send;
    type Res: Debug + Send;
    type Error: Debug + Send;

    async fn handle(&self, req: &Self::Req) -> Result<Self::Res, Self::Error>;
}

#[async_trait]
#[allow(dead_code)]
pub trait Middleware<T: RequestHandler> {
    async fn process(&self, handler: &T, req: T::Req) -> Result<T::Res, T::Error>;
}

#[async_trait]
pub trait TaskStorage {
    type Task: Clone + Send + Sync + std::fmt::Debug;
    type Error;

    async fn get(&self) -> Result<Self::Task, Self::Error>;
    async fn ack(&self, task: &Self::Task) -> Result<(), Self::Error>;
}

pub struct Worker<T, H>
where
    T: TaskStorage,
    H: RequestHandler<Req = T::Task> + Send + Sync,
{
    pub storage: T,
    pub handler: H,
}

impl<T, H> Worker<T, H>
where
    T: TaskStorage + Send + Sync,
    H: RequestHandler<Req = T::Task> + Send + Sync,
{
    pub async fn run(&self) -> Result<(), T::Error> {
        loop {
            let task = self.storage.get().await?;
            match self.handler.handle(&task).await {
                Ok(result) => {
                    println!("Task processed successfully: {:?}", result);
                    self.storage.ack(&task).await?;
                }
                Err(e) => {
                    println!("Error processing task: {:?}", e);
                }
            }
        }
    }
}
