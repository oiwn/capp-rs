#[cfg(test)]
mod tests {
    use capp::healthcheck::internet;

    use hyper::{
        body::Body,
        service::{make_service_fn, service_fn},
        Request, Response, Server,
    };
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
            .status(404)
            .body(Body::from("not found"))
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

        let rt = Runtime::new().unwrap();
        let result = rt.block_on(async { internet().await });

        assert_eq!(result, true);

        // Stop the test server
        drop(server_handle);
    }
}
