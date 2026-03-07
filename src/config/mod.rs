use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Serialize, Deserialize)]
pub struct Config {
    #[serde(default = "default_data_dir")]
    pub data_dir: PathBuf,

    #[serde(default)]
    pub llm: LlmConfig,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct LlmConfig {
    #[allow(dead_code)]
    #[serde(default = "default_provider")]
    pub provider: String,
    #[serde(default = "default_model")]
    pub model: String,
    #[serde(default)]
    pub api_key: String,
    #[serde(default = "default_max_tokens")]
    pub max_tokens: u32,
}

fn default_data_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".rustpilot")
}

fn default_provider() -> String {
    "anthropic".to_string()
}

fn default_model() -> String {
    "claude-sonnet-4-20250514".to_string()
}

fn default_max_tokens() -> u32 {
    8192
}

impl Config {
    pub fn config_path() -> PathBuf {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".rustpilot")
            .join("config.toml")
    }

    pub fn load_or_default() -> Result<Self> {
        let path = Self::config_path();
        if path.exists() {
            let content = std::fs::read_to_string(&path)?;
            let config: Config = toml::from_str(&content)?;
            Ok(config)
        } else {
            let config = Config {
                data_dir: default_data_dir(),
                llm: LlmConfig::default(),
            };
            // Ensure directory exists and write default config
            std::fs::create_dir_all(path.parent().unwrap())?;
            let toml_str = toml::to_string_pretty(&config)?;
            std::fs::write(&path, toml_str)?;
            Ok(config)
        }
    }
}
