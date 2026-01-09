//! Minimal hyper-based HTTP server exposing mailbox stats as JSON.
//! Feature-gated behind `stats-http`.
use std::{convert::Infallible, net::SocketAddr};

use bytes::Bytes;
use http_body_util::Full;
use hyper::{Method, Request, Response, body::Incoming, service::service_fn};
use hyper_util::{
    rt::{TokioExecutor, TokioIo},
    server::conn::auto::Builder,
};
use tokio::{net::TcpListener, sync::watch};

use crate::manager::mailbox::StatsSnapshot;

type RespBody = Full<Bytes>;

fn build_response(status: u16, body: impl Into<Bytes>) -> Response<RespBody> {
    Response::builder()
        .status(status)
        .body(Full::new(body.into()))
        .unwrap()
}

fn handle(
    req: Request<Incoming>,
    stats: watch::Receiver<StatsSnapshot>,
) -> Result<Response<RespBody>, Infallible> {
    match (req.method(), req.uri().path()) {
        (&Method::GET, "/stats") => {
            let snapshot = stats.borrow().clone();
            let body =
                serde_json::to_vec(&snapshot).unwrap_or_else(|_| b"{}".to_vec());
            let mut resp = build_response(200, body);
            resp.headers_mut()
                .insert("content-type", "application/json".parse().unwrap());
            Ok(resp)
        }
        (&Method::GET, "/healthz") => Ok(build_response(200, "ok")),
        _ => Ok(build_response(404, "not found")),
    }
}

/// Start a stats HTTP server on the given address.
pub async fn serve_stats(
    addr: SocketAddr,
    stats: watch::Receiver<StatsSnapshot>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let listener = TcpListener::bind(addr).await?;
    loop {
        let (stream, _) = listener.accept().await?;
        let io = TokioIo::new(stream);
        let stats = stats.clone();
        let service = service_fn(move |req| {
            let stats = stats.clone();
            async move { handle(req, stats) }
        });
        tokio::spawn(async move {
            if let Err(err) = Builder::new(TokioExecutor::new())
                .serve_connection(io, service)
                .await
            {
                tracing::error!(?err, "stats server connection error");
            }
        });
    }
}
