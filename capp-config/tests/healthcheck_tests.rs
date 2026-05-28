#![cfg(feature = "http")]

mod common;

#[cfg(test)]
mod tests {
    use crate::common::http_server::{TestServiceFactory, run_service};
    use capp_config::healthcheck::internet;
    use tokio::{
        io::AsyncWriteExt,
        net::TcpListener,
        time::{Duration, sleep},
    };

    #[test]
    fn ping_healthcheck_service() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _server_handle = rt.spawn(run_service(3000, TestServiceFactory));

        // Wait for the test server to start
        std::thread::sleep(std::time::Duration::from_millis(100));

        let rt = tokio::runtime::Runtime::new().unwrap();
        let result =
            rt.block_on(async { internet("http://127.0.0.1:3000/fail").await });

        assert!(result);
    }

    #[tokio::test]
    async fn internet_returns_true_for_any_response() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let addr = listener.local_addr().unwrap();

        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let response = concat!(
                "HTTP/1.1 200 OK\r\n",
                "Content-Length: 2\r\n",
                "Content-Type: text/plain\r\n",
                "Connection: close\r\n",
                "\r\n",
                "ok"
            );
            stream.write_all(response.as_bytes()).await.unwrap();
        });

        let result = internet(&format!("http://{addr}/")).await;
        assert!(result);
        server.await.unwrap();
    }

    #[tokio::test]
    async fn internet_returns_false_on_connection_refused() {
        // Bind, capture the address, then drop the listener so the port
        // is closed when the request hits it.
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let addr = listener.local_addr().unwrap();
        drop(listener);

        let result = internet(&format!("http://{addr}/")).await;
        assert!(!result);
    }

    #[tokio::test]
    async fn internet_returns_false_on_timeout() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let addr = listener.local_addr().unwrap();

        let server = tokio::spawn(async move {
            let (_stream, _) = listener.accept().await.unwrap();
            sleep(Duration::from_millis(1_100)).await;
        });

        let result = internet(&format!("http://{addr}/")).await;
        assert!(!result);
        server.await.unwrap();
    }
}
