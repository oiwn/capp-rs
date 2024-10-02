use reqwest::{Client, StatusCode};
use serde_json::Value;
use tokio::time::{timeout, Duration};

// const GOOGLE: &str = "http://www.google.com";

/// Check if internet is available.
/// There are a few hosts that are commonly used to check it
/// because they are typically reliable and have high uptime. i
/// Examples:
///     - Google's primary domain: https://www.google.com
///     - Cloudflare's DNS resolver: https://1.1.1.1
///     - Quad9's DNS resolver: https://9.9.9.9
pub async fn internet(http_url: &str) -> bool {
    let client = Client::new();
    let request_future = client.get(http_url).send();

    let response = match timeout(Duration::from_secs(1), request_future).await {
        Ok(response) => response.unwrap(),
        Err(_) => {
            tracing::error!(
                "Internet healthcheck request timed out: {}",
                StatusCode::REQUEST_TIMEOUT
            );
            return false;
        }
    };

    if response.status() == StatusCode::NOT_FOUND
        && response.content_length() == Some(9)
    {
        return true;
    }

    tracing::error!(
        "Internet healthcheck unexpected response status or content length: {:?}",
        response
    );
    false
}

pub async fn test_proxy(proxy_url: &str) -> bool {
    let client = Client::new();
    let proxy_client = Client::builder()
        .proxy(reqwest::Proxy::all(proxy_url).unwrap())
        .build()
        .unwrap();

    let ip_check_url = "https://httpbin.org/ip";

    // Get local IP
    let local_ip = match get_ip(&client, ip_check_url).await {
        Ok(ip) => ip,
        Err(_) => return false,
    };

    // Get IP through proxy
    let proxy_ip = match get_ip(&proxy_client, ip_check_url).await {
        Ok(ip) => ip,
        Err(_) => return false,
    };

    // Compare IPs
    local_ip != proxy_ip
}

async fn get_ip(client: &Client, url: &str) -> Result<String, reqwest::Error> {
    let response = client.get(url).send().await?;
    let body: Value = response.json().await?;
    Ok(body["origin"].as_str().unwrap_or("").to_string())
}
