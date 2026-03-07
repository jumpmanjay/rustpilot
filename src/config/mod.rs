use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

/// Top-level config (~/.rustpilot/config.yaml)
#[derive(Debug, Serialize, Deserialize)]
pub struct Config {
    #[serde(default = "default_data_dir")]
    pub data_dir: PathBuf,

    /// Default model for all agents (provider/model format)
    #[serde(default = "default_model")]
    pub model: String,

    /// Provider configs (api keys, base URLs)
    #[serde(default)]
    pub provider: HashMap<String, ProviderConfig>,

    /// Agent definitions (name → config). Compatible with opencode agent format.
    #[serde(default)]
    pub agent: HashMap<String, AgentDef>,

    /// Global rules injected into all agents' system prompts
    #[serde(default)]
    pub rules: Vec<String>,

    /// Instruction files to load (paths or URLs, like opencode's `instructions`)
    #[serde(default)]
    pub instructions: Vec<String>,

    /// LLM defaults (convenience shorthand — overridden by agent-specific settings)
    #[serde(default)]
    pub llm: LlmDefaults,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct ProviderConfig {
    pub api_key: Option<String>,
    #[serde(default)]
    pub base_url: Option<String>,
}

/// Agent definition — aligned with opencode's agent config schema
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentDef {
    /// "primary" or "subagent"
    #[serde(default = "default_agent_mode")]
    pub mode: String,

    /// Model override for this agent (provider/model format)
    #[serde(default)]
    pub model: Option<String>,

    /// System prompt text, or {file:./path} to load from file
    #[serde(default)]
    pub prompt: Option<String>,

    /// Brief description of what this agent does
    #[serde(default)]
    pub description: Option<String>,

    /// Temperature (0.0 - 1.0)
    #[serde(default)]
    pub temperature: Option<f32>,

    /// Max agentic iterations before forced text response
    #[serde(default)]
    pub steps: Option<usize>,

    /// Max output tokens per API call
    #[serde(default)]
    pub max_tokens: Option<u32>,

    /// Tool permissions (tool_name → enabled)
    #[serde(default)]
    pub tools: HashMap<String, bool>,

    /// Agent-specific rules (appended to global rules)
    #[serde(default)]
    pub rules: Vec<String>,

    /// Disable this agent
    #[serde(default)]
    pub disable: bool,
}

/// Convenience LLM defaults (shorthand for users who don't need multi-agent)
#[derive(Debug, Serialize, Deserialize)]
pub struct LlmDefaults {
    #[serde(default = "default_model")]
    pub model: String,
    #[serde(default)]
    pub api_key: String,
    #[serde(default = "default_max_tokens")]
    pub max_tokens: u32,
    #[serde(default = "default_system_prompt")]
    pub system_prompt: String,
    #[serde(default = "default_max_turns")]
    pub max_turns: usize,
}

impl Default for LlmDefaults {
    fn default() -> Self {
        Self {
            model: default_model(),
            api_key: String::new(),
            max_tokens: default_max_tokens(),
            system_prompt: default_system_prompt(),
            max_turns: default_max_turns(),
        }
    }
}

fn default_data_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".rustpilot")
}

fn default_model() -> String {
    "anthropic/claude-sonnet-4-20250514".to_string()
}

fn default_agent_mode() -> String {
    "primary".to_string()
}

fn default_max_tokens() -> u32 {
    16384
}

fn default_system_prompt() -> String {
    "You are a helpful coding assistant. You have access to tools for reading, writing, and editing files, running commands, and searching the codebase. Use them proactively to help the user.".to_string()
}

fn default_max_turns() -> usize {
    20
}

impl Config {
    pub fn config_path() -> PathBuf {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".rustpilot")
            .join("config.yaml")
    }

    pub fn load_or_default() -> Result<Self> {
        let yaml_path = Self::config_path();
        // Also check for legacy config.toml
        let toml_path = yaml_path.with_file_name("config.toml");

        if yaml_path.exists() {
            let content = std::fs::read_to_string(&yaml_path)?;
            let config: Config = serde_yaml::from_str(&content)?;
            Ok(config)
        } else if toml_path.exists() {
            // Migrate: read the old toml, write new yaml, keep toml as backup
            eprintln!("Migrating config.toml → config.yaml");
            let content = std::fs::read_to_string(&toml_path)?;
            // Parse the old flat toml format into the new Config
            let config = Self::from_legacy_toml(&content)?;
            // Write the new yaml
            let yaml_str = serde_yaml::to_string(&config)?;
            std::fs::write(&yaml_path, &yaml_str)?;
            // Rename old file
            let backup = toml_path.with_extension("toml.bak");
            let _ = std::fs::rename(&toml_path, &backup);
            Ok(config)
        } else {
            let config = Config {
                data_dir: default_data_dir(),
                model: default_model(),
                provider: HashMap::new(),
                agent: HashMap::new(),
                rules: Vec::new(),
                instructions: Vec::new(),
                llm: LlmDefaults::default(),
            };
            // Write default config
            std::fs::create_dir_all(yaml_path.parent().unwrap())?;
            let yaml_str = serde_yaml::to_string(&config)?;
            std::fs::write(&yaml_path, &yaml_str)?;
            Ok(config)
        }
    }

    /// Parse the old toml config format into the new structure
    fn from_legacy_toml(content: &str) -> Result<Self> {
        // The old format was:
        // [llm]
        // api_key = "..."
        // model = "..."
        // max_tokens = ...
        // We do a best-effort parse
        // Simple line parsing since toml crate is no longer available
        let mut api_key = String::new();
        let mut model = default_model();
        let mut max_tokens = default_max_tokens();

        for line in content.lines() {
            let line = line.trim();
            if let Some(val) = line.strip_prefix("api_key") {
                if let Some(val) = val.trim().strip_prefix('=') {
                    api_key = val.trim().trim_matches('"').to_string();
                }
            } else if let Some(val) = line.strip_prefix("model") {
                if let Some(val) = val.trim().strip_prefix('=') {
                    model = val.trim().trim_matches('"').to_string();
                }
            } else if let Some(val) = line.strip_prefix("max_tokens") {
                if let Some(val) = val.trim().strip_prefix('=') {
                    max_tokens = val.trim().parse().unwrap_or(default_max_tokens());
                }
            }
        }

        // If model doesn't have provider prefix, add anthropic/
        if !model.contains('/') {
            model = format!("anthropic/{}", model);
        }

        let mut provider = HashMap::new();
        if !api_key.is_empty() {
            provider.insert("anthropic".to_string(), ProviderConfig {
                api_key: Some(api_key.clone()),
                base_url: None,
            });
        }

        Ok(Config {
            data_dir: default_data_dir(),
            model,
            provider,
            agent: HashMap::new(),
            rules: Vec::new(),
            instructions: Vec::new(),
            llm: LlmDefaults {
                model: default_model(),
                api_key,
                max_tokens,
                system_prompt: default_system_prompt(),
                max_turns: default_max_turns(),
            },
        })
    }

    /// Resolve the API key: check provider map first, fall back to llm.api_key
    pub fn api_key(&self) -> String {
        // Extract provider name from model string
        let provider_name = self.model.split('/').next().unwrap_or("anthropic");
        if let Some(prov) = self.provider.get(provider_name) {
            if let Some(ref key) = prov.api_key {
                if !key.is_empty() {
                    return key.clone();
                }
            }
        }
        // Fall back to env var
        if let Ok(key) = std::env::var("ANTHROPIC_API_KEY") {
            if !key.is_empty() {
                return key;
            }
        }
        // Fall back to llm.api_key
        self.llm.api_key.clone()
    }

    /// Resolve the effective model for a given agent (or default)
    #[allow(dead_code)]
    pub fn effective_model(&self, agent_name: Option<&str>) -> String {
        if let Some(name) = agent_name {
            if let Some(agent) = self.agent.get(name) {
                if let Some(ref m) = agent.model {
                    return m.clone();
                }
            }
        }
        self.model.clone()
    }
}
