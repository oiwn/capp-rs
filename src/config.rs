use serde_yaml::Value;
use std::{fs, io, path};

pub trait Config {
    fn name(&self) -> &String;
    fn config(&self) -> &Value;

    // read configuration from yaml config
    fn from_config(
        config_file_path: impl AsRef<path::Path>,
    ) -> Result<serde_yaml::Value, io::Error> {
        let content: String = fs::read_to_string(config_file_path)?;
        let config: serde_yaml::Value = serde_yaml::from_str(&content)?;
        Ok(config)
    }

    // support methods
    fn load_user_agents();

    /// Extract Value from config using dot notation i.e. "mongodb.default"
    fn get_config_value(&self, key: &str) -> Option<&Value> {
        let keys: Vec<&str> = key.split('.').collect();
        get_value_recursive(self.config(), &keys)
    }
}

pub fn get_value_recursive<'a>(
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
                    get_value_recursive(value, remaining_keys)
                }
            } else {
                None
            }
        }
        _ => None,
    }
}
