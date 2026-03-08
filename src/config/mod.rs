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

    /// Discovered skills (populated at load time, not serialized to config file)
    #[serde(skip)]
    pub skills: HashMap<String, SkillDef>,
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

/// A discovered skill definition (from SKILL.md frontmatter)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillDef {
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub license: Option<String>,
    #[serde(default)]
    pub compatibility: Option<String>,
    #[serde(default)]
    pub metadata: HashMap<String, String>,
    /// Full path to SKILL.md (resolved at load time, not serialized from frontmatter)
    #[serde(skip)]
    pub path: PathBuf,
    /// The body content (below frontmatter) — loaded on demand
    #[serde(skip)]
    pub body: String,
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

    /// Session budget limit in dollars (optional)
    #[serde(default)]
    pub budget: Option<f64>,
}

impl Default for LlmDefaults {
    fn default() -> Self {
        Self {
            model: default_model(),
            api_key: String::new(),
            max_tokens: default_max_tokens(),
            system_prompt: default_system_prompt(),
            max_turns: default_max_turns(),
            budget: None,
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
            let mut config: Config = serde_yaml::from_str(&content)?;
            config.load_markdown_agents();
            config.load_agents_md_rules();
            config.load_skills();
            Ok(config)
        } else if toml_path.exists() {
            // Migrate: read the old toml, write new yaml, keep toml as backup
            eprintln!("Migrating config.toml → config.yaml");
            let content = std::fs::read_to_string(&toml_path)?;
            // Parse the old flat toml format into the new Config
            let mut config = Self::from_legacy_toml(&content)?;
            // Write the new yaml
            let yaml_str = serde_yaml::to_string(&config)?;
            std::fs::write(&yaml_path, &yaml_str)?;
            // Rename old file
            let backup = toml_path.with_extension("toml.bak");
            let _ = std::fs::rename(&toml_path, &backup);
            config.load_markdown_agents();
            config.load_agents_md_rules();
            config.load_skills();
            Ok(config)
        } else {
            let mut config = Config {
                data_dir: default_data_dir(),
                model: default_model(),
                provider: HashMap::new(),
                agent: HashMap::new(),
                rules: Vec::new(),
                instructions: Vec::new(),
                llm: LlmDefaults::default(),
                skills: HashMap::new(),
            };
            // Write default config
            std::fs::create_dir_all(yaml_path.parent().unwrap())?;
            let yaml_str = serde_yaml::to_string(&config)?;
            std::fs::write(&yaml_path, &yaml_str)?;
            config.load_markdown_agents();
            config.load_agents_md_rules();
            config.load_skills();
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
                budget: None,
            },
            skills: HashMap::new(),
        })
    }

    /// Load agent definitions from markdown files with YAML frontmatter.
    /// Searches (in order, later overrides earlier):
    ///   - ~/.rustpilot/agents/*.md        (global)
    ///   - ~/.config/opencode/agents/*.md  (opencode global compat)
    ///   - .rustpilot/agents/*.md          (project)
    ///   - .opencode/agents/*.md           (opencode project compat)
    fn load_markdown_agents(&mut self) {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));

        let search_dirs = [
            home.join(".rustpilot").join("agents"),
            home.join(".config/opencode/agents"),
            cwd.join(".rustpilot/agents"),
            cwd.join(".opencode/agents"),
        ];

        for dir in &search_dirs {
            if !dir.is_dir() {
                continue;
            }
            let Ok(entries) = std::fs::read_dir(dir) else { continue };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) != Some("md") {
                    continue;
                }
                let agent_name = path.file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("")
                    .to_string();
                if agent_name.is_empty() {
                    continue;
                }

                if let Ok(content) = std::fs::read_to_string(&path) {
                    if let Some(agent_def) = Self::parse_agent_markdown(&content) {
                        // Only insert if not already defined in config.yaml
                        // (config.yaml takes precedence over markdown files)
                        self.agent.entry(agent_name).or_insert(agent_def);
                    }
                }
            }
        }
    }

    /// Parse a markdown agent file with YAML frontmatter.
    /// Format:
    /// ```
    /// ---
    /// description: Reviews code
    /// mode: subagent
    /// model: anthropic/claude-sonnet-4-20250514
    /// tools:
    ///   write: false
    /// ---
    /// You are a code reviewer. Focus on...
    /// ```
    fn parse_agent_markdown(content: &str) -> Option<AgentDef> {
        let content = content.trim();
        if !content.starts_with("---") {
            return None;
        }

        // Find the closing ---
        let rest = &content[3..];
        let end = rest.find("\n---")?;
        let frontmatter = &rest[..end];
        let body = rest[end + 4..].trim();

        // Parse frontmatter as YAML into AgentDef
        let mut agent_def: AgentDef = serde_yaml::from_str(frontmatter).ok()?;

        // If no prompt was set in frontmatter, use the markdown body
        if agent_def.prompt.is_none() && !body.is_empty() {
            agent_def.prompt = Some(body.to_string());
        }

        Some(agent_def)
    }

    /// Load rules from AGENTS.md / CLAUDE.md files (project + global).
    /// These are appended to `self.rules`.
    fn load_agents_md_rules(&mut self) {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));

        // Global rules (first match wins per location)
        let global_paths = [
            home.join(".rustpilot/AGENTS.md"),
            home.join(".config/opencode/AGENTS.md"),
            home.join(".claude/CLAUDE.md"),
        ];

        // Project rules (first match wins per location)
        let project_paths = [
            cwd.join("AGENTS.md"),
            cwd.join("CLAUDE.md"),
        ];

        // Load global (use first that exists)
        for path in &global_paths {
            if path.is_file() {
                if let Ok(content) = std::fs::read_to_string(path) {
                    self.rules.push(format!("# Global Rules (from {})\n{}", path.display(), content));
                }
                break;
            }
        }

        // Load project (use first that exists)
        for path in &project_paths {
            if path.is_file() {
                if let Ok(content) = std::fs::read_to_string(path) {
                    self.rules.push(format!("# Project Rules (from {})\n{}", path.display(), content));
                }
                break;
            }
        }
    }

    /// Discover skills from SKILL.md files across standard locations.
    /// Compatible with opencode/Claude Code skill paths.
    fn load_skills(&mut self) {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));

        let search_dirs = [
            // Global
            home.join(".rustpilot/skills"),
            home.join(".config/opencode/skills"),
            home.join(".claude/skills"),
            home.join(".agents/skills"),
            // Project
            cwd.join(".rustpilot/skills"),
            cwd.join(".opencode/skills"),
            cwd.join(".claude/skills"),
            cwd.join(".agents/skills"),
        ];

        for dir in &search_dirs {
            if !dir.is_dir() {
                continue;
            }
            let Ok(entries) = std::fs::read_dir(dir) else { continue };
            for entry in entries.flatten() {
                let skill_dir = entry.path();
                if !skill_dir.is_dir() {
                    continue;
                }
                let skill_md = skill_dir.join("SKILL.md");
                if !skill_md.is_file() {
                    continue;
                }
                let Ok(content) = std::fs::read_to_string(&skill_md) else { continue };
                if let Some(mut skill) = Self::parse_skill_md(&content) {
                    skill.path = skill_md;
                    // First discovery wins (project overrides global due to search order,
                    // but we want project to win, so don't overwrite)
                    self.skills.entry(skill.name.clone()).or_insert(skill);
                }
            }
        }
    }

    /// Parse a SKILL.md file: YAML frontmatter + markdown body
    fn parse_skill_md(content: &str) -> Option<SkillDef> {
        let content = content.trim();
        if !content.starts_with("---") {
            return None;
        }
        let rest = &content[3..];
        let end = rest.find("\n---")?;
        let frontmatter = &rest[..end];
        let body = rest[end + 4..].trim().to_string();

        let mut skill: SkillDef = serde_yaml::from_str(frontmatter).ok()?;
        if skill.name.is_empty() || skill.description.is_empty() {
            return None;
        }
        skill.body = body;
        Some(skill)
    }

    /// Build the <available_skills> XML block for injection into agent system prompts
    #[allow(dead_code)]
    pub fn skills_xml(&self) -> String {
        if self.skills.is_empty() {
            return String::new();
        }
        let mut xml = String::from("\n\n<available_skills>\n");
        for skill in self.skills.values() {
            xml.push_str(&format!(
                "  <skill>\n    <name>{}</name>\n    <description>{}</description>\n  </skill>\n",
                skill.name, skill.description
            ));
        }
        xml.push_str("</available_skills>\n");
        xml
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
