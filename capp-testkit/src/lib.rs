mod support;

use std::{collections::HashMap, net::SocketAddr, sync::Arc, time::Duration};

use anyhow::Context;
use bytes::Bytes;
use http_body_util::Full;
use hyper::{
    Method, Request, Response, StatusCode, body::Incoming, server::conn::http1,
    service::service_fn,
};
use serde::{Deserialize, Serialize};
use tokio::{
    net::TcpListener,
    sync::{Mutex, oneshot},
    task::JoinHandle,
};

use crate::support::TokioIo;

type ResponseBody = Full<Bytes>;

#[derive(Clone, Copy, Debug)]
pub enum Scenario {
    Blog { posts: u32 },
    Catalog,
    Infinite,
}

impl Scenario {
    pub fn blog() -> Self {
        Self::Blog { posts: 4 }
    }

    pub fn blog_with_posts(posts: u32) -> Self {
        Self::Blog {
            posts: posts.max(1),
        }
    }

    pub fn catalog() -> Self {
        Self::Catalog
    }

    pub fn infinite() -> Self {
        Self::Infinite
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RequestLogEntry {
    pub method: String,
    pub path: String,
    pub query: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SiteStats {
    pub scenario: String,
    pub total_requests: u64,
    pub per_path: HashMap<String, u64>,
    pub request_log: Vec<RequestLogEntry>,
}

#[derive(Debug)]
pub struct TestServerHandle {
    addr: SocketAddr,
    state: Arc<State>,
    shutdown_tx: Option<oneshot::Sender<()>>,
    server_task: JoinHandle<anyhow::Result<()>>,
}

impl TestServerHandle {
    pub async fn spawn(scenario: Scenario) -> anyhow::Result<Self> {
        let listener = TcpListener::bind(("127.0.0.1", 0))
            .await
            .context("bind test server listener")?;
        let addr = listener.local_addr().context("read local test address")?;
        let state = Arc::new(State::new(scenario));
        let (shutdown_tx, mut shutdown_rx) = oneshot::channel::<()>();
        let server_state = state.clone();

        let server_task = tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = &mut shutdown_rx => {
                        break;
                    }
                    accepted = listener.accept() => {
                        let (stream, _) = accepted.context("accept test server connection")?;
                        let io = TokioIo::new(stream);
                        let connection_state = server_state.clone();

                        tokio::spawn(async move {
                            let service = service_fn(move |req| {
                                let state = connection_state.clone();
                                async move { Ok::<_, hyper::Error>(handle_request(req, state).await) }
                            });

                            if let Err(err) = http1::Builder::new()
                                .serve_connection(io, service)
                                .await
                            {
                                eprintln!("test server connection error: {err:?}");
                            }
                        });
                    }
                }
            }

            Ok(())
        });

        Ok(Self {
            addr,
            state,
            shutdown_tx: Some(shutdown_tx),
            server_task,
        })
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    pub fn base_url(&self) -> String {
        format!("http://{}", self.addr)
    }

    pub fn stats_url(&self) -> String {
        format!("{}/stats", self.base_url())
    }

    pub async fn stats(&self) -> SiteStats {
        self.state.snapshot().await
    }

    pub async fn shutdown(mut self) -> anyhow::Result<()> {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }

        self.server_task.await.context("join test server task")?
    }
}

#[derive(Debug)]
struct State {
    scenario: Scenario,
    request_log: Mutex<Vec<RequestLogEntry>>,
    per_path: Mutex<HashMap<String, u64>>,
}

impl State {
    fn new(scenario: Scenario) -> Self {
        Self {
            scenario,
            request_log: Mutex::new(Vec::new()),
            per_path: Mutex::new(HashMap::new()),
        }
    }

    async fn record(&self, req: &Request<Incoming>) {
        let path = req.uri().path().to_string();
        let query = req.uri().query().map(ToOwned::to_owned);
        let method = req.method().as_str().to_string();

        {
            let mut per_path = self.per_path.lock().await;
            *per_path.entry(path.clone()).or_insert(0) += 1;
        }

        let mut request_log = self.request_log.lock().await;
        request_log.push(RequestLogEntry {
            method,
            path,
            query,
        });
    }

    async fn snapshot(&self) -> SiteStats {
        let request_log = self.request_log.lock().await.clone();
        let per_path = self.per_path.lock().await.clone();

        SiteStats {
            scenario: self.scenario.name().to_string(),
            total_requests: request_log.len() as u64,
            per_path,
            request_log,
        }
    }
}

impl Scenario {
    fn name(self) -> &'static str {
        match self {
            Self::Blog { .. } => "blog",
            Self::Catalog => "catalog",
            Self::Infinite => "infinite",
        }
    }
}

async fn handle_request(
    req: Request<Incoming>,
    state: Arc<State>,
) -> Response<ResponseBody> {
    state.record(&req).await;

    if req.method() != Method::GET {
        return response(StatusCode::METHOD_NOT_ALLOWED, "method not allowed")
            .with_content_type("text/plain; charset=utf-8");
    }

    if req.uri().path() == "/stats" {
        let body = match serde_json::to_vec(&state.snapshot().await) {
            Ok(body) => body,
            Err(_) => b"{\"error\":\"stats serialization failed\"}".to_vec(),
        };

        return Response::builder()
            .status(StatusCode::OK)
            .header("content-type", "application/json")
            .body(Full::new(Bytes::from(body)))
            .unwrap_or_else(|_| {
                response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "stats serialization failed",
                )
                .with_content_type("text/plain; charset=utf-8")
            });
    }

    match state.scenario {
        Scenario::Blog { posts } => blog_response(req.uri().path(), posts).await,
        Scenario::Catalog => {
            catalog_response(req.uri().path(), req.uri().query()).await
        }
        Scenario::Infinite => infinite_response(req.uri().path()).await,
    }
}

async fn blog_response(path: &str, posts: u32) -> Response<ResponseBody> {
    let page = if path == "/" {
        Some(blog_home_page(posts))
    } else if path == "/archive" {
        Some(blog_archive_page(posts))
    } else if let Some(post_id) = path.strip_prefix("/post/") {
        post_id
            .parse::<u32>()
            .ok()
            .filter(|id| (1..=posts).contains(id))
            .map(|id| blog_post_page(id, posts))
    } else {
        None
    };

    match page {
        Some(body) => html_ok(body),
        None => not_found(path),
    }
}

fn blog_home_page(posts: u32) -> String {
    let mut links = Vec::with_capacity(6);
    links.push(("/archive".to_string(), "Archive".to_string()));

    for post_id in 1..=posts.min(5) {
        links.push((format!("/post/{post_id}"), format!("Post {post_id:03}")));
    }

    if posts > 5 {
        links.push((format!("/post/{posts}"), format!("Latest post {posts:03}")));
    }

    html_page_owned("Blog Home", links)
}

fn blog_archive_page(posts: u32) -> String {
    let mut links = Vec::with_capacity((posts as usize) + 1);
    links.push(("/".to_string(), "Home".to_string()));

    for post_id in 1..=posts {
        links.push((format!("/post/{post_id}"), format!("Post {post_id:03}")));
    }

    html_page_owned("Archive", links)
}

fn blog_post_page(post_id: u32, posts: u32) -> String {
    let mut links = Vec::with_capacity(6);
    links.push(("/".to_string(), "Home".to_string()));
    links.push(("/archive".to_string(), "Archive".to_string()));

    if post_id > 1 {
        links.push((
            format!("/post/{}", post_id - 1),
            format!("Previous {}", post_id - 1),
        ));
    }

    if post_id < posts {
        links.push((
            format!("/post/{}", post_id + 1),
            format!("Next {}", post_id + 1),
        ));
    }

    if post_id + 2 <= posts {
        links.push((
            format!("/post/{}", post_id + 2),
            format!("Related {}", post_id + 2),
        ));
    }

    if post_id > 2 {
        links.push((
            format!("/post/{}", post_id - 2),
            format!("Earlier {}", post_id - 2),
        ));
    }

    html_page_owned(&format!("Blog Post {post_id:03}"), links)
}

async fn catalog_response(
    path: &str,
    query: Option<&str>,
) -> Response<ResponseBody> {
    let page = match path {
        "/" => Some(html_page(
            "Catalog Home",
            &[
                ("/category/widgets?page=1", "Widgets"),
                ("/category/gadgets?page=1", "Gadgets"),
            ],
        )),
        "/category/widgets" => Some(catalog_category_page("widgets", query)),
        "/category/gadgets" => Some(catalog_category_page("gadgets", query)),
        "/item/widgets-1" => Some(catalog_item_page("widgets-1")),
        "/item/widgets-2" => Some(catalog_item_page("widgets-2")),
        "/item/widgets-3" => Some(catalog_item_page("widgets-3")),
        "/item/gadgets-1" => Some(catalog_item_page("gadgets-1")),
        "/item/gadgets-2" => Some(catalog_item_page("gadgets-2")),
        "/item/gadgets-3" => Some(catalog_item_page("gadgets-3")),
        _ => None,
    };

    match page {
        Some(body) => html_ok(body),
        None => not_found(path),
    }
}

fn catalog_category_page(category: &str, query: Option<&str>) -> String {
    let page_number = match query {
        Some("page=2") => 2,
        _ => 1,
    };

    match (category, page_number) {
        ("widgets", 1) => html_page(
            "Widgets Page 1",
            &[
                ("/", "Home"),
                ("/item/widgets-1", "Item widgets-1"),
                ("/item/widgets-2", "Item widgets-2"),
                ("/category/widgets?page=2", "Next page"),
            ],
        ),
        ("widgets", 2) => html_page(
            "Widgets Page 2",
            &[
                ("/", "Home"),
                ("/category/widgets?page=1", "Previous page"),
                ("/item/widgets-3", "Item widgets-3"),
            ],
        ),
        ("gadgets", 1) => html_page(
            "Gadgets Page 1",
            &[
                ("/", "Home"),
                ("/item/gadgets-1", "Item gadgets-1"),
                ("/item/gadgets-2", "Item gadgets-2"),
                ("/category/gadgets?page=2", "Next page"),
            ],
        ),
        ("gadgets", 2) => html_page(
            "Gadgets Page 2",
            &[
                ("/", "Home"),
                ("/category/gadgets?page=1", "Previous page"),
                ("/item/gadgets-3", "Item gadgets-3"),
            ],
        ),
        _ => html_page("Missing", &[]),
    }
}

fn catalog_item_page(item: &str) -> String {
    let category = if item.starts_with("widgets") {
        "/category/widgets?page=1"
    } else {
        "/category/gadgets?page=1"
    };

    html_page(
        &format!("Item {item}"),
        &[("/", "Home"), (category, "Category")],
    )
}

async fn infinite_response(path: &str) -> Response<ResponseBody> {
    let normalized = if path == "/" { "/page/0" } else { path };
    let depth = normalized
        .trim_start_matches("/page/")
        .parse::<u64>()
        .unwrap_or(0);
    let next = depth + 1;
    let sibling = depth + 2;

    tokio::time::sleep(Duration::from_millis(10)).await;

    html_ok(html_page(
        &format!("Infinite Page {depth}"),
        &[
            (&format!("/page/{next}"), "Next"),
            (&format!("/page/{sibling}"), "Skip"),
        ],
    ))
}

fn html_page(title: &str, links: &[(&str, &str)]) -> String {
    let links = links
        .iter()
        .map(|(href, label)| format!("<li><a href=\"{href}\">{label}</a></li>"))
        .collect::<Vec<_>>()
        .join("");

    format!(
        "<!doctype html><html><head><title>{title}</title></head><body><h1>{title}</h1><ul>{links}</ul></body></html>"
    )
}

fn html_page_owned(title: &str, links: Vec<(String, String)>) -> String {
    let links = links
        .into_iter()
        .map(|(href, label)| format!("<li><a href=\"{href}\">{label}</a></li>"))
        .collect::<Vec<_>>()
        .join("");

    format!(
        "<!doctype html><html><head><title>{title}</title></head><body><h1>{title}</h1><ul>{links}</ul></body></html>"
    )
}

fn html_ok(body: String) -> Response<ResponseBody> {
    response(StatusCode::OK, body).with_content_type("text/html; charset=utf-8")
}

fn not_found(path: &str) -> Response<ResponseBody> {
    response(StatusCode::NOT_FOUND, format!("not found: {path}"))
        .with_content_type("text/plain; charset=utf-8")
}

fn response(status: StatusCode, body: impl Into<Bytes>) -> ResponseBuilder {
    ResponseBuilder {
        status,
        body: body.into(),
        content_type: None,
    }
}

struct ResponseBuilder {
    status: StatusCode,
    body: Bytes,
    content_type: Option<&'static str>,
}

impl ResponseBuilder {
    fn with_content_type(
        mut self,
        content_type: &'static str,
    ) -> Response<ResponseBody> {
        self.content_type = Some(content_type);
        self.build()
    }

    fn build(self) -> Response<ResponseBody> {
        let mut builder = Response::builder().status(self.status);
        if let Some(content_type) = self.content_type {
            builder = builder.header("content-type", content_type);
        }

        builder.body(Full::new(self.body)).unwrap_or_else(|_| {
            Response::new(Full::new(Bytes::from_static(b"response build failed")))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{Scenario, TestServerHandle};
    use reqwest::StatusCode;

    #[tokio::test]
    async fn blog_stats_endpoint_tracks_requests() {
        let server = TestServerHandle::spawn(Scenario::blog())
            .await
            .expect("server starts");
        let client = reqwest::Client::new();

        let root = client
            .get(server.base_url())
            .send()
            .await
            .expect("root request");
        assert!(root.status().is_success());

        let post = client
            .get(format!("{}/post/1", server.base_url()))
            .send()
            .await
            .expect("post request");
        assert!(post.status().is_success());

        let stats = client
            .get(server.stats_url())
            .send()
            .await
            .expect("stats request")
            .json::<super::SiteStats>()
            .await
            .expect("stats json");

        assert_eq!(stats.scenario, "blog");
        assert_eq!(stats.total_requests, 3);
        assert_eq!(stats.per_path.get("/"), Some(&1));
        assert_eq!(stats.per_path.get("/post/1"), Some(&1));
        assert_eq!(stats.per_path.get("/stats"), Some(&1));

        server.shutdown().await.expect("server shutdown");
    }

    #[tokio::test]
    async fn catalog_scenario_tracks_paginated_requests() {
        let server = TestServerHandle::spawn(Scenario::catalog())
            .await
            .expect("server starts");
        let client = reqwest::Client::new();

        for path in [
            "/",
            "/category/widgets?page=1",
            "/category/widgets?page=2",
            "/item/widgets-3",
            "/category/gadgets?page=1",
            "/category/gadgets?page=2",
            "/item/gadgets-3",
        ] {
            let response = client
                .get(format!("{}{}", server.base_url(), path))
                .send()
                .await
                .expect("catalog request");
            assert_eq!(response.status(), StatusCode::OK);
        }

        let stats = server.stats().await;
        assert_eq!(stats.scenario, "catalog");
        assert_eq!(stats.total_requests, 7);
        assert_eq!(stats.per_path.get("/category/widgets"), Some(&2));
        assert_eq!(stats.per_path.get("/category/gadgets"), Some(&2));

        server.shutdown().await.expect("server shutdown");
    }

    #[tokio::test]
    async fn infinite_scenario_handles_generated_pages() {
        let server = TestServerHandle::spawn(Scenario::infinite())
            .await
            .expect("server starts");
        let client = reqwest::Client::new();

        for path in ["/", "/page/1", "/page/2", "/page/not-a-number"] {
            let response = client
                .get(format!("{}{}", server.base_url(), path))
                .send()
                .await
                .expect("infinite request");
            assert_eq!(response.status(), StatusCode::OK);
            let body = response.text().await.expect("infinite body");
            assert!(body.contains("Infinite Page"));
        }

        let stats = server.stats().await;
        assert_eq!(stats.scenario, "infinite");
        assert_eq!(stats.total_requests, 4);
        assert_eq!(stats.per_path.get("/page/1"), Some(&1));
        assert_eq!(stats.per_path.get("/page/not-a-number"), Some(&1));

        server.shutdown().await.expect("server shutdown");
    }

    #[tokio::test]
    async fn non_get_requests_return_method_not_allowed() {
        let server = TestServerHandle::spawn(Scenario::blog())
            .await
            .expect("server starts");
        let client = reqwest::Client::new();

        let response = client
            .post(format!("{}/post/1", server.base_url()))
            .send()
            .await
            .expect("post request");

        assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
        assert_eq!(response.text().await.unwrap(), "method not allowed");

        let stats = server.stats().await;
        assert_eq!(stats.total_requests, 1);
        assert_eq!(stats.per_path.get("/post/1"), Some(&1));

        server.shutdown().await.expect("server shutdown");
    }

    #[tokio::test]
    async fn unknown_blog_path_returns_not_found() {
        let server = TestServerHandle::spawn(Scenario::blog_with_posts(8))
            .await
            .expect("server starts");
        let client = reqwest::Client::new();

        let response = client
            .get(format!("{}/missing", server.base_url()))
            .send()
            .await
            .expect("missing request");

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert_eq!(response.text().await.unwrap(), "not found: /missing");

        server.shutdown().await.expect("server shutdown");
    }

    #[tokio::test]
    async fn generated_blog_supports_large_post_counts() {
        let server = TestServerHandle::spawn(Scenario::blog_with_posts(100))
            .await
            .expect("server starts");
        let client = reqwest::Client::new();

        for path in ["/", "/archive", "/post/1", "/post/57", "/post/100"] {
            let response = client
                .get(format!("{}{}", server.base_url(), path))
                .send()
                .await
                .expect("blog request");
            assert_eq!(response.status(), StatusCode::OK);
        }

        let stats = server.stats().await;
        assert_eq!(stats.total_requests, 5);
        assert_eq!(stats.per_path.get("/post/100"), Some(&1));

        server.shutdown().await.expect("server shutdown");
    }
}
