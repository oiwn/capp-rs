use std::io::{self, BufRead};
use std::{fs, path};

mod config;

#[derive(thiserror::Error, Debug)]
pub enum ApplicationError {
    #[error("Cannot open/read config file")]
    ReadConfigFile(#[source] io::Error),

    #[error("Cannot parse the configuration file")]
    ParseConfig(#[source] serde_yaml::Error),

    #[error("Can not open user agents file")]
    ReadUserAgentsFile(#[source] io::Error),
}

pub struct Application {
    pub name: String,
    pub config: serde_yaml::Value,
}

impl Application {
    pub fn new(
        name: &str,
        yaml_config_path: impl AsRef<path::Path>,
    ) -> Result<Self, ApplicationError> {
        let config = Application::load_config(yaml_config_path)?;

        Ok(Application {
            name: name.to_string(),
            config,
        })
    }

    pub fn from_env(name: &str) {}

    fn load_config(
        config_file_path: impl AsRef<path::Path>,
    ) -> Result<serde_yaml::Value, ApplicationError> {
        let content: String = fs::read_to_string(config_file_path)
            .map_err(ApplicationError::ReadConfigFile)?;
        let config: serde_yaml::Value = serde_yaml::from_str(&content)
            .map_err(ApplicationError::ParseConfig)?;
        Ok(config)
    }

    fn load_text_file_into_lines(
        ua_file_path: impl AsRef<path::Path>,
    ) -> Result<Vec<String>, ApplicationError> {
        let file =
            fs::File::open(ua_file_path).map_err(ApplicationError::ReadTextFile)?;
        let lines = io::BufReader::new(file).lines();
        let user_agents: Vec<String> =
            lines.map(|l| l.expect("Could not parse line")).collect();

        Ok(user_agents)
    }

    pub fn get_config_value(&self, key: &str) -> Option<&serde_yaml::Value> {
        let keys: Vec<&str> = key.split('.').collect();
        get_value_recursive(&self.config, &keys)
    }
}

pub fn add(left: usize, right: usize) -> usize {
    left + right
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_works() {
        let result = add(2, 2);
        assert_eq!(result, 4);
    }
}
