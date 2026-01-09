use rand::{rng, seq::IndexedRandom};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::{
    fmt,
    sync::{LazyLock, Mutex},
};
use toml::Value;

// Add Debug to the trait bounds
pub trait ProxyProvider: Send + Sync + fmt::Debug {
    fn get_proxy(&self) -> Option<String>;
}

static PORT_RANGE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\{(\d+)-(\d+)\}").expect("Failed to compile port range regex")
});

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProxyConfig {
    pub use_proxy: bool,
    pub uris: Vec<String>,
}

impl ProxyConfig {
    fn expand_uri(uri: &str) -> Vec<String> {
        if let Some(captures) = PORT_RANGE_RE.captures(uri) {
            if let (Some(start), Some(end)) = (
                captures.get(1).and_then(|m| m.as_str().parse::<u16>().ok()),
                captures.get(2).and_then(|m| m.as_str().parse::<u16>().ok()),
            ) {
                if start <= end {
                    return (start..=end)
                        .map(|port| uri.replace(&captures[0], &port.to_string()))
                        .collect();
                }
            }
        }
        vec![uri.to_string()]
    }

    pub fn from_config(config: &Value) -> Option<Self> {
        let use_proxy = config.get("use")?.as_bool()?;
        let uris = if use_proxy {
            if let Some(seq) = config.get("uris").and_then(|v| v.as_array()) {
                seq.iter()
                    .filter_map(|v| v.as_str())
                    .flat_map(Self::expand_uri)
                    .collect()
            } else if let Some(uri) = config.get("uri").and_then(|v| v.as_str()) {
                Self::expand_uri(uri)
            } else {
                vec![]
            }
        } else {
            vec![]
        };

        Some(ProxyConfig { use_proxy, uris })
    }
}

#[derive(Debug)]
pub struct RandomProxyProvider {
    config: ProxyConfig,
}

impl RandomProxyProvider {
    pub fn new(config: ProxyConfig) -> Self {
        Self { config }
    }

    pub fn from_config(config: &Value) -> Option<Self> {
        let config = ProxyConfig::from_config(config)?;
        Some(Self::new(config))
    }
}

impl ProxyProvider for RandomProxyProvider {
    fn get_proxy(&self) -> Option<String> {
        if !self.config.use_proxy || self.config.uris.is_empty() {
            return None;
        }

        // Create a new thread_rng for each call
        let mut rng = rng();
        self.config.uris.choose(&mut rng).cloned()
    }
}

// Round robin implementation
#[derive(Debug)]
pub struct RoundRobinProxyProvider {
    config: ProxyConfig,
    current: Mutex<usize>,
}

impl RoundRobinProxyProvider {
    pub fn new(config: ProxyConfig) -> Self {
        Self {
            config,
            current: Mutex::new(0),
        }
    }

    pub fn from_config(config: &Value) -> Option<Self> {
        let config = ProxyConfig::from_config(config)?;
        Some(Self::new(config))
    }
}

impl ProxyProvider for RoundRobinProxyProvider {
    fn get_proxy(&self) -> Option<String> {
        if !self.config.use_proxy || self.config.uris.is_empty() {
            return None;
        }

        let mut current = self.current.lock().ok()?;
        let proxy = self.config.uris[*current].clone();
        *current = (*current + 1) % self.config.uris.len();
        Some(proxy)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOML_CONF_SINGLE: &str = r#"
    [proxy]
    use = true
    uri = "http://proxy1.example.com:8080"
    timeout = 30
    connect_timeout = 10
    "#;

    const TOML_CONF_MULTIPLE: &str = r#"
    [proxy]
    use = true
    uris = [
        "http://proxy1.example.com:8080",
        "http://proxy2.example.com:8080",
        "http://proxy3.example.com:8080",
    ]
    timeout = 30
    connect_timeout = 10
    "#;

    #[test]
    fn test_proxy_provider() {
        let config: Value = toml::from_str(TOML_CONF_SINGLE).unwrap();
        let provider = RandomProxyProvider::from_config(&config["proxy"]).unwrap();

        let proxy_url = provider.get_proxy().unwrap();
        assert!(proxy_url.as_str() == "http://proxy1.example.com:8080");
    }

    #[test]
    fn test_random_proxy_provider() {
        let config: Value = toml::from_str(TOML_CONF_MULTIPLE).unwrap();
        let provider = RandomProxyProvider::from_config(&config["proxy"]).unwrap();

        // Test multiple calls return different proxies
        let mut seen = std::collections::HashSet::new();
        for _ in 0..10 {
            if let Some(proxy) = provider.get_proxy() {
                seen.insert(proxy);
            }
        }
        assert!(seen.len() > 1); // Should see multiple different proxies
    }

    #[test]
    fn test_round_robin_proxy_provider() {
        let config: Value = toml::from_str(TOML_CONF_MULTIPLE).unwrap();
        let provider =
            RoundRobinProxyProvider::from_config(&config["proxy"]).unwrap();

        // Test round robin behavior
        let first = provider.get_proxy();
        let second = provider.get_proxy();
        let third = provider.get_proxy();
        let fourth = provider.get_proxy(); // Should wrap around to first

        assert_ne!(first, second);
        assert_ne!(second, third);
        assert_eq!(first, fourth);
    }

    #[test]
    fn test_proxy_provider_disabled() {
        let config: Value = toml::from_str(
            r#"
        [proxy]
        use = false
        uri = "http://proxy1.example.com:8080"
        "#,
        )
        .unwrap();

        let provider = RandomProxyProvider::from_config(&config["proxy"]).unwrap();
        assert_eq!(provider.get_proxy(), None);
    }

    #[test]
    fn test_proxy_config_port_range() {
        let toml = r#"
        [proxy]
        use = true
        uri = "http://proxy1.example.com:{8080-8082}"
        "#;

        let config: Value = toml::from_str(toml).unwrap();
        let proxy_config = ProxyConfig::from_config(&config["proxy"]).unwrap();

        assert_eq!(proxy_config.uris.len(), 3);
        assert!(
            proxy_config
                .uris
                .contains(&"http://proxy1.example.com:8080".to_string())
        );
        assert!(
            proxy_config
                .uris
                .contains(&"http://proxy1.example.com:8081".to_string())
        );
        assert!(
            proxy_config
                .uris
                .contains(&"http://proxy1.example.com:8082".to_string())
        );
    }

    #[test]
    fn test_proxy_config_multiple_uris_with_ranges() {
        let toml = r#"
        [proxy]
        use = true
        uris = [
            "http://proxy1.example.com:{8080-8082}",
            "http://proxy2.example.com:8090",
            "http://proxy3.example.com:{9000-9001}",
        ]
        "#;

        let config: Value = toml::from_str(toml).unwrap();
        let proxy_config = ProxyConfig::from_config(&config["proxy"]).unwrap();

        assert_eq!(proxy_config.uris.len(), 6); // 3 from first range + 1 single + 2 from last range
        assert!(
            proxy_config
                .uris
                .contains(&"http://proxy2.example.com:8090".to_string())
        );
    }

    #[test]
    fn test_invalid_port_range() {
        let toml = r#"
        [proxy]
        use = true
        uri = "http://proxy1.example.com:{8082-8080}"
        "#;

        let config: Value = toml::from_str(toml).unwrap();
        let proxy_config = ProxyConfig::from_config(&config["proxy"]).unwrap();

        // Should treat invalid range as literal string
        assert_eq!(proxy_config.uris.len(), 1);
        assert_eq!(
            proxy_config.uris[0],
            "http://proxy1.example.com:{8082-8080}".to_string()
        );
    }

    #[test]
    fn test_no_port_range() {
        let toml = r#"
        [proxy]
        use = true
        uri = "http://proxy1.example.com:8080"
        "#;

        let config: Value = toml::from_str(toml).unwrap();
        let proxy_config = ProxyConfig::from_config(&config["proxy"]).unwrap();

        assert_eq!(proxy_config.uris.len(), 1);
        assert_eq!(
            proxy_config.uris[0],
            "http://proxy1.example.com:8080".to_string()
        );
    }
}
