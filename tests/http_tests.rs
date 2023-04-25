#[cfg(test)]
mod tests {
    use capp::http::{
        build_http_client, fetch_url, fetch_url_content, HttpClientParams,
    };
    use hyper::service::{make_service_fn, service_fn};
    use hyper::{Body, Request, Response, Server};
    use std::net::SocketAddr;
    use std::time::Duration;
    use tokio::runtime::Runtime;

    pub async fn start_test_server(addr: SocketAddr) {
        let make_svc = make_service_fn(|_conn| async {
            Ok::<_, hyper::Error>(service_fn(handle_request))
        });

        let server = Server::bind(&addr).serve(make_svc);
        println!("Test server running on http://{}", addr);

        if let Err(e) = server.await {
            eprintln!("Server error: {}", e);
        }
    }

    async fn handle_request(
        _req: Request<Body>,
    ) -> Result<Response<Body>, hyper::Error> {
        let response = Response::builder()
            .status(200)
            .body(Body::from("Hello, World!"))
            .unwrap();
        Ok(response)
    }

    #[test]
    fn test_http_request() {
        let addr = ([127, 0, 0, 1], 8080).into();

        // Start the test server in a separate thread
        let server_handle = std::thread::spawn(move || {
            let rt = Runtime::new().unwrap();
            rt.block_on(start_test_server(addr))
        });

        // Wait for the test server to start
        std::thread::sleep(Duration::from_millis(100));

        let params = HttpClientParams {
            proxy: None,
            timeout: 5,
            connect_timeout: 2,
            user_agent: "test-client",
        };

        let client = build_http_client(params).expect("Failed to build client");

        let rt = Runtime::new().unwrap();
        let resp = rt.block_on(async {
            fetch_url(client.clone(), format!("http://{}", addr).as_str())
                .await
                .ok()
                .unwrap()
        });

        assert_eq!(resp.status(), 200);

        let rt = Runtime::new().unwrap();
        let resp = rt.block_on(async {
            fetch_url_content(client, format!("http://{}", addr).as_str())
                .await
                .ok()
                .unwrap()
        });

        assert_eq!(
            (
                reqwest::StatusCode::from_u16(200).unwrap(),
                "Hello, World!".to_string()
            ),
            resp
        );

        // Stop the test server
        drop(server_handle);
    }
}
