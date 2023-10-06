use backoff::ExponentialBackoffBuilder;
use std::time::Duration;

#[derive(Debug)]
pub struct HttpClientParams<'a> {
    pub timeout: u64,
    pub connect_timeout: u64,
    pub proxy: Option<reqwest::Proxy>,
    pub user_agent: &'a str,
}

/// Helper to create typical crawling request with few useful options
/// Assuming there is some kind of settings chunk `http_config` like:
/// ```ignore
/// http:
///     proxy:
///         use: true
///         uri: http://bro:admin@proxygate1.com:420420
///     timeout: 30
///     connect_timeout: 10
/// ```
/// uri could contain range of ports like:
/// http://bro:admin@proxygate1.com:{420420..420440}
impl<'a> HttpClientParams<'a> {
    pub fn from_config(
        http_config: &serde_yaml::Value,
        user_agent: &'a str,
    ) -> Self {
        let timeout = http_config["timeout"]
            .as_u64()
            .expect("No timeout field in config");
        let connect_timeout = http_config["connect_timeout"]
            .as_u64()
            .expect("No connect_timout field in config");
        let need_proxy: bool = http_config["proxy"]["use"]
            .as_bool()
            .expect("No use field for proxy in config");
        let proxy = if need_proxy {
            let proxy_uri = http_config["proxy"]["uri"].as_str().unwrap();
            Some(reqwest::Proxy::all(proxy_uri).expect("Error setting up proxy"))
        } else {
            None
        };
        Self {
            timeout,
            connect_timeout,
            proxy,
            user_agent,
        }
    }
}

/* TODO:
pub fn parse_proxy_uri(uri: &str) -> String {
    match RE.find(uri) {
        Some(matched) => {
            let caps = RE.captures(matched).unwrap();
            let start = caps.get(1).map_or("", |m| m.as_str());
            let end = caps.get(2).map_or("", |m| m.as_str());
        }
        Err(_) => {}
    }
}
*/

/// Helper to create typical crawling request with sane defaultsx
pub fn build_http_client(
    params: HttpClientParams,
) -> Result<reqwest::Client, reqwest::Error> {
    let mut client_builder = reqwest::ClientBuilder::new()
        .use_rustls_tls()
        .danger_accept_invalid_certs(true)
        .timeout(Duration::from_secs(params.timeout))
        .connect_timeout(Duration::from_secs(params.connect_timeout))
        .user_agent(params.user_agent);

    if let Some(proxy) = params.proxy {
        client_builder = client_builder.proxy(proxy);
    }

    let client = client_builder.build()?;
    Ok(client)
}

/// Fetch url with retries (with sane defaults),
/// notice it will not download content
pub async fn fetch_url(
    client: reqwest::Client,
    url: &str,
) -> Result<reqwest::Response, reqwest::Error> {
    let backoff = ExponentialBackoffBuilder::new()
        .with_max_interval(std::time::Duration::from_secs(10))
        .build();
    backoff::future::retry(backoff, || async {
        tracing::info!("[{}] retriving url...", url);
        Ok(client.get(url).send().await?)
    })
    .await
}

/// Fetch content from url retrying
pub async fn fetch_url_content(
    client: reqwest::Client,
    url: &str,
) -> Result<(reqwest::StatusCode, String), reqwest::Error> {
    let backoff = ExponentialBackoffBuilder::new()
        .with_max_interval(std::time::Duration::from_secs(10))
        .with_max_elapsed_time(Some(std::time::Duration::from_secs(30)))
        .build();

    let fetch_content = || async {
        tracing::info!("[{}] retrieving url...", url);
        let response = client.get(url.clone()).send().await?;
        let status = response.status();
        let text = response.text().await?;
        Ok((status, text))
    };

    backoff::future::retry(backoff, fetch_content).await
}

#[cfg(test)]
mod tests {
    use super::*;

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
            proxy: None,
            user_agent: "hello",
        });

        assert!(client.is_ok());
    }

    #[test]
    fn test_build_client_from_config() {
        let config: serde_yaml::Value =
            serde_yaml::from_str(YAML_CONF_TEXT).unwrap();
        let client = build_http_client(HttpClientParams::from_config(
            &config.get("http").unwrap(),
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
            &config.get("http").unwrap(),
            "hellobot",
        ));
    }
}
