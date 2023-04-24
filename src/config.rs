use serde_yaml::Value;
use std::{
    fs,
    io::{self, BufRead},
    path,
};

pub trait Configurable {
    fn name(&self) -> &str;
    fn config(&self) -> &Value;

    // read configuration from yaml config
    fn load_config(
        config_file_path: impl AsRef<path::Path>,
    ) -> Result<serde_yaml::Value, io::Error> {
        let content: String = fs::read_to_string(config_file_path)?;
        let config: serde_yaml::Value = serde_yaml::from_str(&content).unwrap();
        Ok(config)
    }

    // support methods
    fn load_text_file_lines(
        file_path: impl AsRef<path::Path>,
    ) -> Result<Vec<String>, io::Error> {
        let file = fs::File::open(file_path)?;
        let lines: Vec<String> = io::BufReader::new(file)
            .lines()
            .map(|l| l.expect("Could not parse line"))
            .collect();

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
            serde_yaml::Value::Mapping(map) => {
                let key = keys[0];
                let remaining_keys = &keys[1..];

                if let Some(value) =
                    map.get(&serde_yaml::Value::String(key.to_string()))
                {
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

    pub struct Application {
        name: String,
        config: Value,
        user_agents: Option<Vec<String>>,
    }

    impl Configurable for Application {
        fn name(&self) -> &str {
            self.name.as_str()
        }
        fn config(&self) -> &Value {
            &self.config
        }
    }

    impl Application {
        fn from_config(config_file_path: impl AsRef<path::Path>) -> Self {
            let config = Self::load_config(config_file_path);
            Self {
                name: "test-app".to_string(),
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
        let config_path = "tests/simple_config.yml";
        let app = Application::from_config(config_path);

        assert_eq!(app.config["app"]["threads"].as_u64(), Some(4));
        assert_eq!(app.config()["app"]["max_queue"].as_u64(), Some(500));
        assert_eq!(app.user_agents, None);
    }

    #[test]
    fn test_get_config_value() {
        let config_path = "tests/simple_config.yml";
        let app = Application::from_config(config_path);

        assert_eq!(
            app.get_config_value("logging.log_to_redis")
                .unwrap()
                .as_bool(),
            Some(true)
        )
    }

    #[test]
    fn test_load_lines() {
        let config_path = "tests/simple_config.yml";
        let mut app = Application::from_config(config_path);
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
