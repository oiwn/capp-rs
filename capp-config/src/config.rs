use std::{
    fs,
    io::{self, BufRead},
    path,
};

use toml::Value;

#[derive(thiserror::Error, Debug)]
pub enum ConfigError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("TOML parsing error: {0}")]
    TomlParse(#[from] toml::de::Error),
    #[error("Line parsing error: {0}")]
    LineParse(String),
}

pub trait Configurable {
    fn config(&self) -> &Value;

    // read configuration from toml config
    fn load_config(
        config_file_path: impl AsRef<path::Path>,
    ) -> Result<Value, ConfigError> {
        let content: String = fs::read_to_string(config_file_path)?;
        let config: Value = toml::from_str(&content)?;
        Ok(config)
    }

    /// Load `Vec<String>`` from file with path `file path`
    fn load_text_file_lines(
        file_path: impl AsRef<path::Path>,
    ) -> Result<Vec<String>, ConfigError> {
        let file = fs::File::open(file_path)?;
        let lines = io::BufReader::new(file)
            .lines()
            .map(|l| l.map_err(|e| ConfigError::LineParse(e.to_string())))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(lines)
    }

    /// Extract Value from config using dot notation i.e. "app.concurrency"
    fn get_config_value(&self, key: &str) -> Option<&Value> {
        let keys: Vec<&str> = key.split('.').collect();
        Self::get_value_recursive(self.config(), &keys)
    }

    fn get_value_recursive<'a>(
        config: &'a Value,
        keys: &[&str],
    ) -> Option<&'a Value> {
        if keys.is_empty() {
            return None;
        };

        match config {
            Value::Table(map) => {
                let key = keys[0];
                let remaining_keys = &keys[1..];

                if let Some(value) = map.get(key) {
                    if remaining_keys.is_empty() {
                        Some(value)
                    } else {
                        Self::get_value_recursive(value, remaining_keys)
                    }
                } else {
                    None
                }
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use std::io::Write;
    use tempfile::tempdir;

    pub struct TestApp {
        config: Value,
        user_agents: Option<Vec<String>>,
    }

    impl Configurable for TestApp {
        fn config(&self) -> &Value {
            &self.config
        }
    }

    impl TestApp {
        fn from_config(config_file_path: impl AsRef<path::Path>) -> Self {
            let config = Self::load_config(config_file_path);
            Self {
                config: config.unwrap(),
                user_agents: None,
            }
        }

        fn load_uas(&mut self, uas_file_path: impl AsRef<path::Path>) {
            self.user_agents = Self::load_text_file_lines(uas_file_path).ok();
        }
    }

    #[test]
    fn test_load_config() {
        let config_path = "../tests/simple_config.toml";
        let app = TestApp::from_config(config_path);

        assert_eq!(app.config["app"]["threads"].as_integer(), Some(4));
        assert_eq!(app.config()["app"]["max_queue"].as_integer(), Some(500));
        assert_eq!(app.user_agents, None);
    }

    #[test]
    fn test_load_config_valid_yaml() {
        let dir = tempdir().unwrap();
        let config_path = dir.path().join("config.toml");
        let mut file = File::create(&config_path).unwrap();
        writeln!(file, "key = \"value\"\n[app]\nsetting = 42").unwrap();

        let config = TestApp::load_config(&config_path);
        assert!(config.is_ok());
        let config = config.unwrap();
        assert_eq!(config["key"].as_str(), Some("value"));
        assert_eq!(config["app"]["setting"].as_integer(), Some(42));
    }

    #[test]
    fn test_load_config_invalid_yaml() {
        let dir = tempdir().unwrap();
        let config_path = dir.path().join("config.toml");
        let mut file = File::create(&config_path).unwrap();
        writeln!(file, "invalid = : toml").unwrap();

        let config = TestApp::load_config(&config_path);
        assert!(matches!(config, Err(ConfigError::TomlParse(_))));
    }

    #[test]
    fn test_load_text_file_lines() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("test.txt");
        let mut file = File::create(&file_path).unwrap();
        writeln!(file, "line1\nline2\nline3").unwrap();

        let lines = TestApp::load_text_file_lines(&file_path);
        assert!(lines.is_ok());
        let lines = lines.unwrap();
        assert_eq!(lines, vec!["line1", "line2", "line3"]);
    }

    #[test]
    fn test_get_config_value_empty_keys() {
        let config_path = "../tests/simple_config.toml";
        let app = TestApp::from_config(config_path);
        assert_eq!(app.get_config_value(""), None);
    }

    #[test]
    fn test_get_config_value() {
        let config_path = "../tests/simple_config.toml";
        let app = TestApp::from_config(config_path);

        assert_eq!(
            app.get_config_value("logging.log_to_redis")
                .unwrap()
                .as_bool(),
            Some(true)
        )
    }

    #[test]
    fn test_get_config_value_recursive() {
        let toml = r#"
        [app.nested]
        value = 42
        "#;
        let config: Value = toml::from_str(toml).unwrap();
        let app = TestApp {
            config,
            user_agents: None,
        };

        assert_eq!(
            app.get_config_value("app.nested.value")
                .and_then(|v| v.as_integer()),
            Some(42)
        );
        assert_eq!(app.get_config_value("app.missing.value"), None);
        assert_eq!(app.get_config_value("missing"), None);
    }

    #[test]
    fn test_load_lines() {
        let config_path = "../tests/simple_config.toml";
        let mut app = TestApp::from_config(config_path);
        let uas_file_path = {
            app.get_config_value("app.user_agents_file")
                .unwrap()
                .as_str()
                .unwrap()
                .to_owned()
        };
        app.load_uas(&uas_file_path);
    }
}
