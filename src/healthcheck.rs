use reqwest::{Client, StatusCode};
use tokio::time::{timeout, Duration};

/// Check if internet is available.
/// There are a few hosts that are commonly used to check it
/// because they are typically reliable and have high uptime. i
/// Examples:
///     - Google's primary domain: https://www.google.com
///     - Cloudflare's DNS resolver: https://1.1.1.1
///     - Quad9's DNS resolver: https://9.9.9.9
pub async fn internet() -> bool {
    #[allow(unused_variables)]
    let host_url = "https://9.9.9.9";
    #[cfg(test)]
    let host_url = "https://127.0.0.1:8080";

    let client = Client::new();
    let request_future = client.get(host_url).send();

    let response = match timeout(Duration::from_secs(5), request_future).await {
        Ok(response) => response.unwrap(),
        Err(_) => {
            log::error!(
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

    log::error!(
        "Internet healthcheck unexpected response status or content length: {:?}",
        response
    );
    false
}
