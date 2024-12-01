//! HTTP client module for making web requests with configurable proxy support.
//!
//! This module provides functionality for building and configuring HTTP clients with
//! features such as:
//! - Configurable proxy support with port range expansion
//! - Timeout settings
//! - User agent customization
//! - Retry mechanisms with exponential backoff
//!
//! # Example
//! ```no_run
//! use capp_config::http::{HttpClientParams, build_http_client};
//! use serde_yaml::Value;
//!
//! let config: Value = serde_yaml::from_str(r#"
//! http:
//!     proxy:
//!         use: true
//!         uri: http://proxy.example.com:{8080-8082}
//!     timeout: 30
//!     connect_timeout: 10
//! "#).unwrap();
//!
//! let params = HttpClientParams::from_config(&config["http"], "my-crawler/1.0");
//! let client = build_http_client(params).unwrap();
//! ```
use crate::proxy::{ProxyProvider, RandomProxyProvider};
use backoff::ExponentialBackoffBuilder;

/// Parameters for configuring an HTTP client.
///
/// This struct holds all configuration needed to build a customized HTTP client,
/// including timeout settings, proxy configuration, and user agent string.
#[derive(Debug)]
pub struct HttpClientParams<'a> {
    pub timeout: u64,
    pub connect_timeout: u64,
    pub proxy_provider: Option<Box<dyn ProxyProvider + Send + Sync>>,
    pub user_agent: &'a str,
}

impl<'a> HttpClientParams<'a> {
    /// Creates an HttpClientParams instance from a YAML configuration.
    ///
    /// The configuration should follow this structure:
    /// ```yaml
    /// http:
    ///     proxy:
    ///         use: true
    ///         uri: http://user:pass@proxy.example.com:8080
    ///     timeout: 30
    ///     connect_timeout: 10
    /// ```
    ///
    /// The proxy URI can include a port range using the format:
    /// `http://proxy.example.com:{8080-8090}`
    ///
    /// # Arguments
    /// * `http_config` - YAML configuration containing HTTP settings
    /// * `user_agent` - User agent string to be used in requests
    ///
    /// # Panics
    /// Panics if required configuration fields are missing (timeout, connect_timeout)
    pub fn from_config(
        http_config: &serde_yaml::Value,
        user_agent: &'a str,
    ) -> Self {
        let timeout = http_config["timeout"]
            .as_u64()
            .expect("No timeout field in config");
        let connect_timeout = http_config["connect_timeout"]
            .as_u64()
            .expect("No connect_timeout field in config");

        let proxy_provider =
            if http_config["proxy"]["use"].as_bool().unwrap_or(false) {
                Some(Box::new(
                    RandomProxyProvider::from_config(&http_config["proxy"])
                        .expect("Failed to create proxy provider"),
                ) as Box<dyn ProxyProvider + Send + Sync>)
            } else {
                None
            };

        Self {
            timeout,
            connect_timeout,
            proxy_provider,
            user_agent,
        }
    }
}

/// Builds an HTTP client with the specified parameters.
///
/// Creates a reqwest::Client configured with:
/// - TLS settings
/// - Timeout configurations
/// - User agent
/// - Optional proxy support
///
/// # Arguments
/// * `params` - Configuration parameters for the client
///
/// # Returns
/// A Result containing either the configured client or an error
pub fn build_http_client(
    params: HttpClientParams,
) -> Result<reqwest::Client, reqwest::Error> {
    let mut client_builder = reqwest::ClientBuilder::new()
        .use_rustls_tls()
        .danger_accept_invalid_certs(true)
        .timeout(std::time::Duration::from_secs(params.timeout))
        .connect_timeout(std::time::Duration::from_secs(params.connect_timeout))
        .user_agent(params.user_agent);

    if let Some(proxy_provider) = params.proxy_provider {
        if let Some(proxy_uri) = proxy_provider.get_proxy() {
            client_builder = client_builder.proxy(
                reqwest::Proxy::all(&proxy_uri).expect("Failed to create proxy"),
            );
        }
    }

    client_builder.build()
}

/// Fetches a URL with automatic retries.
///
/// Makes a GET request to the specified URL, automatically retrying on failure
/// using exponential backoff. This method only retrieves headers and status,
/// not the response body.
///
/// # Arguments
/// * `client` - The HTTP client to use for the request
/// * `url` - The URL to fetch
///
/// # Returns
/// A Result containing either the response or an error
pub async fn fetch_url(
    client: reqwest::Client,
    url: &str,
) -> Result<reqwest::Response, reqwest::Error> {
    let backoff = ExponentialBackoffBuilder::new()
        .with_max_interval(std::time::Duration::from_secs(10))
        .build();
    backoff::future::retry(backoff, || async { Ok(client.get(url).send().await?) })
        .await
}

/// Fetches content from a URL with automatic retries.
///
/// Makes a GET request to the specified URL and retrieves the full response body,
/// automatically retrying on failure using exponential backoff.
///
/// # Arguments
/// * `client` - The HTTP client to use for the request
/// * `url` - The URL to fetch
///
/// # Returns
/// A Result containing either a tuple of (StatusCode, response_body) or an error
pub async fn fetch_url_content(
    client: reqwest::Client,
    url: &str,
) -> Result<(reqwest::StatusCode, String), reqwest::Error> {
    let backoff = ExponentialBackoffBuilder::new()
        .with_max_interval(std::time::Duration::from_secs(10))
        .with_max_elapsed_time(Some(std::time::Duration::from_secs(30)))
        .build();

    let fetch_content = || async {
        let response = client.get(url).send().await?;
        let status = response.status();
        let text = response.text().await?;
        Ok((status, text))
    };

    backoff::future::retry(backoff, fetch_content).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_yaml::Value;
    use std::collections::HashSet;
    use std::time::Duration;

    const YAML_CONF_SINGLE_PORT: &str = r#"
    http:
        proxy:
            use: true
            uri: http://proxy1.example.com:8080
        timeout: 30
        connect_timeout: 10
    "#;

    const YAML_CONF_PORT_RANGE: &str = r#"
    http:
        proxy:
            use: true
            uri: http://proxy1.example.com:{8080-8082}
        timeout: 30
        connect_timeout: 10
    "#;

    const YAML_CONF_MULTIPLE_PORT_RANGES: &str = r#"
    http:
        proxy:
            use: true
            uris:
                - http://proxy1.example.com:{8080-8082}
                - http://proxy2.example.com:9090
                - http://proxy3.example.com:{9000-9001}
        timeout: 30
        connect_timeout: 10
    "#;

    const YAML_CONF_TEXT: &str = r#"
    http:
      proxy:
        use: true
        uri: http://bro:admin@proxygate1.com:42042
      timeout: 30
      connect_timeout: 10
    "#;

    const WRONG_YAML_CONF_TEXT: &str = r#"
    http:
      proxy:
        use: true
        uri: http://bro:admin@proxygate1.com:42042
      connect_timeout: 10
    "#;

    #[test]
    fn test_build_client() {
        let client = build_http_client(HttpClientParams {
            timeout: 10,
            connect_timeout: 5,
            proxy_provider: None,
            user_agent: "hello",
        });

        assert!(client.is_ok());
    }

    #[test]
    fn test_build_client_from_config() {
        let config: serde_yaml::Value =
            serde_yaml::from_str(YAML_CONF_TEXT).unwrap();
        let client = build_http_client(HttpClientParams::from_config(
            config.get("http").unwrap(),
            "hellobot",
        ));
        assert!(client.is_ok());
    }

    #[test]
    #[should_panic(expected = "No timeout field in config")]
    fn test_build_client_bad_config() {
        let config: serde_yaml::Value =
            serde_yaml::from_str(WRONG_YAML_CONF_TEXT).unwrap();
        let _ = build_http_client(HttpClientParams::from_config(
            config.get("http").unwrap(),
            "hellobot",
        ));
    }

    #[test]
    fn test_http_client_single_port() {
        let config: Value = serde_yaml::from_str(YAML_CONF_SINGLE_PORT).unwrap();
        let client_params =
            HttpClientParams::from_config(&config["http"], "test-agent/1.0");

        assert_eq!(client_params.timeout, 30);
        assert_eq!(client_params.connect_timeout, 10);

        if let Some(provider) = client_params.proxy_provider {
            let proxy = provider.get_proxy().unwrap();
            assert_eq!(proxy, "http://proxy1.example.com:8080");
        } else {
            panic!("Expected proxy provider to be configured");
        }
    }

    #[test]
    fn test_http_client_port_range() {
        let config: Value = serde_yaml::from_str(YAML_CONF_PORT_RANGE).unwrap();
        let client_params =
            HttpClientParams::from_config(&config["http"], "test-agent/1.0");

        let mut seen_ports = HashSet::new();
        if let Some(provider) = client_params.proxy_provider {
            // Test multiple calls to verify different ports are used
            for _ in 0..10 {
                let proxy = provider.get_proxy().unwrap();
                let port = extract_port_from_proxy_url(&proxy);
                seen_ports.insert(port);
            }
        }

        // Should see all three ports from range 8080-8082
        assert_eq!(seen_ports.len(), 3);
        assert!(seen_ports.contains(&8080));
        assert!(seen_ports.contains(&8081));
        assert!(seen_ports.contains(&8082));
    }

    #[test]
    fn test_http_client_multiple_port_ranges() {
        let config: Value =
            serde_yaml::from_str(YAML_CONF_MULTIPLE_PORT_RANGES).unwrap();
        let client_params =
            HttpClientParams::from_config(&config["http"], "test-agent/1.0");

        let mut seen_proxies = HashSet::new();
        if let Some(provider) = client_params.proxy_provider {
            // Test multiple calls to verify different proxies are used
            for _ in 0..20 {
                let proxy = provider.get_proxy().unwrap();
                seen_proxies.insert(proxy);
            }
        }

        // Should see:
        // 3 ports from proxy1 (8080-8082)
        // 1 from proxy2 (9090)
        // 2 from proxy3 (9000-9001)
        assert_eq!(seen_proxies.len(), 6);

        // Verify specific proxy patterns
        assert!(seen_proxies
            .iter()
            .any(|p| p == "http://proxy1.example.com:8080"));
        assert!(seen_proxies
            .iter()
            .any(|p| p == "http://proxy1.example.com:8081"));
        assert!(seen_proxies
            .iter()
            .any(|p| p == "http://proxy1.example.com:8082"));
        assert!(seen_proxies
            .iter()
            .any(|p| p == "http://proxy2.example.com:9090"));
        assert!(seen_proxies
            .iter()
            .any(|p| p == "http://proxy3.example.com:9000"));
        assert!(seen_proxies
            .iter()
            .any(|p| p == "http://proxy3.example.com:9001"));
    }

    #[test]
    fn test_http_client_builder_timeouts_are_set() {
        let config: Value = serde_yaml::from_str(YAML_CONF_PORT_RANGE).unwrap();
        let client_params =
            HttpClientParams::from_config(&config["http"], "test-agent/1.0");

        assert_eq!(client_params.timeout, Duration::from_secs(30).as_secs());
        assert_eq!(
            client_params.connect_timeout,
            Duration::from_secs(10).as_secs()
        );
    }

    #[test]
    fn test_proxy_disabled() {
        let yaml = r#"
        http:
            proxy:
                use: false
                uri: http://proxy1.example.com:8080
            timeout: 30
            connect_timeout: 10
        "#;

        let config: Value = serde_yaml::from_str(yaml).unwrap();
        let client_params =
            HttpClientParams::from_config(&config["http"], "test-agent/1.0");

        assert!(
            client_params.proxy_provider.is_none(),
            "Proxy provider should be None when disabled"
        );
    }

    #[test]
    #[should_panic(expected = "No timeout field in config")]
    fn test_missing_timeout() {
        let yaml = r#"
        http:
            proxy:
                use: true
                uri: http://proxy1.example.com:8080
            connect_timeout: 10
        "#;

        let config: Value = serde_yaml::from_str(yaml).unwrap();
        let _ = HttpClientParams::from_config(&config["http"], "test-agent/1.0");
    }

    #[test]
    fn test_invalid_port_range() {
        let yaml = r#"
        http:
            proxy:
                use: true
                uri: http://proxy1.example.com:{8082-8080}
            timeout: 30
            connect_timeout: 10
        "#;

        let config: Value = serde_yaml::from_str(yaml).unwrap();
        let client_params =
            HttpClientParams::from_config(&config["http"], "test-agent/1.0");

        if let Some(provider) = client_params.proxy_provider {
            let proxy = provider.get_proxy().unwrap();
            // Should treat invalid range as literal string
            assert_eq!(proxy, "http://proxy1.example.com:{8082-8080}");
        }
    }

    // Helper function to extract port number from proxy URL
    fn extract_port_from_proxy_url(url: &str) -> u16 {
        let parts: Vec<&str> = url.split(':').collect();
        parts.last().unwrap().parse().unwrap()
    }
}
