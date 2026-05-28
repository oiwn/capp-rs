use reqwest::Client;
use serde_json::Value;
use tokio::time::{Duration, timeout};

/// Returns `true` iff any HTTP response is received within 1 second.
/// Network errors (DNS, refused, TLS) and timeouts return `false`.
/// Choose a URL your environment can normally reach. Examples:
///     - Cloudflare's DNS resolver: <https://1.1.1.1>
///     - Quad9's DNS resolver: <https://9.9.9.9>
///     - Google's primary domain: <https://www.google.com>
pub async fn internet(http_url: &str) -> bool {
    let client = Client::new();
    let request_future = client.get(http_url).send();

    match timeout(Duration::from_secs(1), request_future).await {
        Ok(Ok(_response)) => true,
        Ok(Err(err)) => {
            tracing::warn!(%err, url = http_url, "internet healthcheck failed");
            false
        }
        Err(_) => {
            tracing::warn!(url = http_url, "internet healthcheck timed out");
            false
        }
    }
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
