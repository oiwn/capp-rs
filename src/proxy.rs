use rand::seq::SliceRandom;
use rand::thread_rng;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::sync::Mutex;

// Add Debug to the trait bounds
pub trait ProxyProvider: Send + Sync + fmt::Debug {
    fn get_proxy(&self) -> Option<String>;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProxyConfig {
    pub use_proxy: bool,
    pub uris: Vec<String>,
}

impl ProxyConfig {
    pub fn from_config(config: &serde_yaml::Value) -> Option<Self> {
        let use_proxy = config["use"].as_bool()?;
        let uris = if use_proxy {
            match config["uris"].as_sequence() {
                Some(seq) => seq
                    .iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect(),
                None => {
                    vec![config["uri"].as_str()?.to_string()]
                }
            }
        } else {
            vec![]
        };

        Some(ProxyConfig { use_proxy, uris })
    }
}

// Use a thread-safe RNG
#[derive(Debug)]
pub struct RandomProxyProvider {
    config: ProxyConfig,
    // Use once_cell to lazily initialize the Mutex
    rng: Mutex<()>, // We'll create a new RNG for each call instead
}

impl RandomProxyProvider {
    pub fn new(config: ProxyConfig) -> Self {
        Self {
            config,
            rng: Mutex::new(()),
        }
    }

    pub fn from_config(config: &serde_yaml::Value) -> Option<Self> {
        let config = ProxyConfig::from_config(config)?;
        Some(Self::new(config))
    }
}

impl ProxyProvider for RandomProxyProvider {
    fn get_proxy(&self) -> Option<String> {
        if !self.config.use_proxy || self.config.uris.is_empty() {
            return None;
        }

        let _lock = self.rng.lock().ok()?;
        // Create a new thread_rng for each call
        let mut rng = thread_rng();
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

    pub fn from_config(config: &serde_yaml::Value) -> Option<Self> {
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
    use serde_yaml::Value;

    const YAML_CONF_SINGLE: &str = r#"
    proxy:
        use: true
        uri: http://proxy1.example.com:8080
        timeout: 30
        connect_timeout: 10
    "#;

    const YAML_CONF_MULTIPLE: &str = r#"
    proxy:
        use: true
        uris:
            - http://proxy1.example.com:8080
            - http://proxy2.example.com:8080
            - http://proxy3.example.com:8080
        timeout: 30
        connect_timeout: 10
    "#;

    #[test]
    fn test_random_proxy_provider() {
        let config: Value = serde_yaml::from_str(YAML_CONF_MULTIPLE).unwrap();
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
        let config: Value = serde_yaml::from_str(YAML_CONF_MULTIPLE).unwrap();
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
        let config: Value = serde_yaml::from_str(
            r#"
        proxy:
            use: false
            uri: http://proxy1.example.com:8080
        "#,
        )
        .unwrap();

        let provider = RandomProxyProvider::from_config(&config["proxy"]).unwrap();
        assert_eq!(provider.get_proxy(), None);
    }
}
