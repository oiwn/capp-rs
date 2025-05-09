mod common;

#[cfg(test)]
mod tests {
    use crate::common::http_server::{run_service, TestServiceFactory};
    use capp::http::{
        build_http_client, fetch_url, fetch_url_content, HttpClientParams,
    };

    #[test]
    fn test_http_request() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _server_handle = rt.spawn(run_service(3000, TestServiceFactory));

        // Wait for the test server to start
        std::thread::sleep(std::time::Duration::from_millis(100));

        let params = HttpClientParams {
            proxy_provider: None,
            timeout: 5,
            connect_timeout: 2,
            user_agent: "test-client",
        };

        let client = build_http_client(params).expect("Failed to build client");

        let rt = tokio::runtime::Runtime::new().unwrap();
        let resp = rt.block_on(async {
            fetch_url(client.clone(), "http://127.0.0.1:3000")
                .await
                .ok()
                .unwrap()
        });

        assert_eq!(resp.status(), 200);

        let rt = tokio::runtime::Runtime::new().unwrap();
        let resp = rt.block_on(async {
            fetch_url_content(client, "http://127.0.0.1:3000/fail")
                .await
                .ok()
                .unwrap()
        });

        assert_eq!(
            (reqwest::StatusCode::NOT_FOUND, "not found".to_string()),
            resp
        );
    }
}
