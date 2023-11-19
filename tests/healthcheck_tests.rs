#[cfg(test)]
mod tests {
    use capp::healthcheck::internet;
    use capp::test_utils::{run_service, TestServiceFactory};

    #[test]
    fn ping_healthcheck_service() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _server_handle = rt.spawn(run_service(3000, TestServiceFactory));

        // Wait for the test server to start
        std::thread::sleep(std::time::Duration::from_millis(100));

        let rt = tokio::runtime::Runtime::new().unwrap();
        let result =
            rt.block_on(async { internet("http://127.0.0.1:3000/fail").await });

        assert_eq!(result, true);
    }
}
