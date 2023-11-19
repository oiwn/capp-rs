use bytes::Bytes;
use http_body_util::Full;
use hyper::server::conn::http1;
use hyper::service::Service;
use hyper::{body::Incoming as IncomingBody, Request, Response};
use tokio::net::TcpListener;

use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;

use crate::support::TokioIo;

pub trait ServiceFactory {
    type ServiceType: Service<
            Request<IncomingBody>,
            Response = Response<Full<Bytes>>,
            Error = hyper::Error,
        > + Send
        + 'static;

    fn create_service(&self) -> Self::ServiceType;
}

pub struct TestService;
pub struct TestServiceFactory;

impl ServiceFactory for TestServiceFactory {
    type ServiceType = TestService;

    fn create_service(&self) -> Self::ServiceType {
        TestService::new()
    }
}

impl Service<Request<IncomingBody>> for TestService {
    type Response = Response<Full<Bytes>>;
    type Error = hyper::Error;
    type Future =
        Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn call(&self, req: Request<IncomingBody>) -> Self::Future {
        fn ok_response(s: String) -> Result<Response<Full<Bytes>>, hyper::Error> {
            Ok(Response::builder().body(Full::new(Bytes::from(s))).unwrap())
        }

        fn fail_response(s: String) -> Result<Response<Full<Bytes>>, hyper::Error> {
            Ok(Response::builder()
                .status(hyper::StatusCode::NOT_FOUND)
                .body(Full::new(Bytes::from(s)))
                .unwrap())
        }

        let res = match req.uri().path() {
            "/" => ok_response(format!("here")),
            // Return the 404 Not Found for other routes.
            _ => return Box::pin(async { fail_response("not found".into()) }),
        };

        Box::pin(async { res })
    }
}

impl TestService {
    fn new() -> Self {
        Self {}
    }
}

pub async fn run_service<F>(port: u16, service_factory: F) -> anyhow::Result<()>
where
    F: ServiceFactory + Send + 'static,
    <<F as ServiceFactory>::ServiceType as Service<
        hyper::Request<hyper::body::Incoming>,
    >>::Future: Send,
{
    let addr: SocketAddr = ([127, 0, 0, 1], port).into();

    let listener = TcpListener::bind(&addr).await?;
    println!("Listening on http://{}", addr);

    loop {
        let (stream, _) = listener.accept().await?;
        let io = TokioIo::new(stream);
        let service = service_factory.create_service();
        tokio::task::spawn(async move {
            if let Err(err) =
                http1::Builder::new().serve_connection(io, service).await
            {
                println!("Failed to serve connection: {:?}", err);
            }
        });
    }
}
