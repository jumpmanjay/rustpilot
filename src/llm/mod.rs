pub mod agent;
pub mod tools;

use std::collections::HashMap;
use tokio::sync::mpsc;

use agent::{Agent, AgentConfig, AgentEvent, AgentId};
use tools::ToolRegistry;

use crate::config::Config;
use crate::panels::llm::{LlmChunk, LlmPanel};

/// Manages multiple parallel agent sessions
pub struct LlmManager {
    api_key: String,
    default_model: String,
    default_max_tokens: u32,
    cwd: String,

    /// Channel for agent events from all running agents
    rx: mpsc::UnboundedReceiver<(AgentId, AgentEvent)>,
    tx: mpsc::UnboundedSender<(AgentId, AgentEvent)>,

    /// Active agents (kept for reference, actual work happens in spawned tasks)
    pub agents: HashMap<AgentId, AgentInfo>,

    /// Global rules loaded from config + AGENTS.md
    global_rules: Vec<String>,
}

/// Info about a running agent (metadata kept in main thread)
#[derive(Debug, Clone)]
pub struct AgentInfo {
    pub id: AgentId,
    #[allow(dead_code)]
    pub name: String,
    pub model: String,
    pub status: AgentStatus,
    pub total_tokens_in: u64,
    pub total_tokens_out: u64,
    pub total_turns: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub enum AgentStatus {
    Running,
    Done,
    Error(String),
}

impl LlmManager {
    pub fn new(config: &Config) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();

        let cwd = std::env::current_dir()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| ".".to_string());

        // Extract model name (strip provider/ prefix for API calls)
        let model = config.model.clone();
        let model_name = model.split('/').last().unwrap_or(&model).to_string();

        Self {
            api_key: config.api_key(),
            default_model: model_name,
            default_max_tokens: config.llm.max_tokens,
            cwd,
            rx,
            tx,
            agents: HashMap::new(),
            global_rules: config.rules.clone(),
        }
    }

    /// Spawn a new agent with default config
    pub fn spawn_agent(&mut self, name: &str, system_prompt: &str) -> AgentId {
        let config = AgentConfig {
            model: self.default_model.clone(),
            max_tokens: self.default_max_tokens,
            system_prompt: system_prompt.to_string(),
            ..Default::default()
        };
        self.spawn_agent_with_config(name, config)
    }

    /// Spawn a new agent with custom config
    pub fn spawn_agent_with_config(&mut self, name: &str, config: AgentConfig) -> AgentId {
        let agent = Agent::new(config.clone(), &self.cwd);
        let agent_id = agent.id.clone();

        self.agents.insert(
            agent_id.clone(),
            AgentInfo {
                id: agent_id.clone(),
                name: name.to_string(),
                model: config.model.clone(),
                status: AgentStatus::Running,
                total_tokens_in: 0,
                total_tokens_out: 0,
                total_turns: 0,
            },
        );

        agent_id
    }

    /// Send a message to an existing agent (or create a default one)
    #[allow(dead_code)]
    pub fn send_prompt(&mut self, prompt: &str) -> AgentId {
        self.send_prompt_with_history(prompt, &[])
    }

    /// Send a prompt with conversation history for context
    pub fn send_prompt_with_history(&mut self, prompt: &str, history: &[(String, String)]) -> AgentId {
        self.send_prompt_to_with_history(None, prompt, history)
    }

    /// Send a prompt to a specific agent, or create a new default one
    #[allow(dead_code)]
    pub fn send_prompt_to(&mut self, agent_id: Option<&str>, prompt: &str) -> AgentId {
        self.send_prompt_to_with_history(agent_id, prompt, &[])
    }

    /// Send a prompt to a specific agent with conversation history
    pub fn send_prompt_to_with_history(&mut self, agent_id: Option<&str>, prompt: &str, history: &[(String, String)]) -> AgentId {
        let id = if let Some(id) = agent_id {
            id.to_string()
        } else if let Some(existing) = self.agents.values().find(|a| a.status == AgentStatus::Running || a.status == AgentStatus::Done) {
            existing.id.clone()
        } else {
            self.spawn_agent("default", "You are a helpful coding assistant. You have access to tools for reading, writing, and editing files, running commands, and searching the codebase. Use them to help the user.")
        };

        let api_key = self.api_key.clone();
        let tx = self.tx.clone();
        let prompt = prompt.to_string();
        let cwd = self.cwd.clone();
        let model = self.agents.get(&id).map(|a| a.model.clone()).unwrap_or_else(|| self.default_model.clone());
        let max_tokens = self.default_max_tokens;
        let history = history.to_vec();
        let global_rules = self.global_rules.clone();

        if api_key.is_empty() {
            let _ = tx.send((
                id.clone(),
                AgentEvent::Error("No API key configured. Set llm.api_key in ~/.rustpilot/config.toml".into()),
            ));
            return id;
        }

        // Update status
        if let Some(info) = self.agents.get_mut(&id) {
            info.status = AgentStatus::Running;
        }

        // Spawn the agentic loop in a background task
        let agent_id = id.clone();
        tokio::spawn(async move {
            let mut system_prompt = "You are a helpful coding assistant. You have access to tools for reading, writing, and editing files, running commands, and searching the codebase. Use them to help the user.".to_string();

            // Append global rules from config + AGENTS.md
            if !global_rules.is_empty() {
                system_prompt.push_str("\n\n## Rules\n");
                for rule in &global_rules {
                    system_prompt.push_str(rule);
                    system_prompt.push('\n');
                }
            }

            let config = AgentConfig {
                model,
                max_tokens,
                system_prompt,
                ..Default::default()
            };
            let mut agent = Agent::new(config, &cwd);
            agent.id = agent_id;

            // Pre-populate conversation history from storage
            for (role, content) in &history {
                agent.history.push(agent::Message {
                    role: role.clone(),
                    content: agent::MessageContent::Text(content.clone()),
                });
            }

            let tools = ToolRegistry::with_defaults();
            agent.run(&prompt, &api_key, &tools, &tx).await;
        });

        id
    }

    /// Poll for updates from all running agents
    pub fn poll_updates(&mut self, panel: &mut LlmPanel) {
        while let Ok((agent_id, event)) = self.rx.try_recv() {
            match &event {
                AgentEvent::Text(text) => {
                    panel.push_chunk(LlmChunk::Text(text.clone()));
                }
                AgentEvent::ToolCall { name, input } => {
                    panel.push_chunk(LlmChunk::ToolUse {
                        name: name.clone(),
                        input: input.clone(),
                    });
                }
                AgentEvent::ToolOutput {
                    name,
                    output,
                    is_error,
                } => {
                    let prefix = if *is_error { "✗" } else { "✓" };
                    // Show truncated output in the panel
                    let display = if output.len() > 500 {
                        format!("{}...({} chars)", &output[..500], output.len())
                    } else {
                        output.clone()
                    };
                    panel.push_chunk(LlmChunk::Text(format!(
                        "\n{} {} → {}\n",
                        prefix, name, display
                    )));
                }
                AgentEvent::TurnDone {
                    tokens_in,
                    tokens_out,
                    turn,
                } => {
                    if let Some(info) = self.agents.get_mut(&agent_id) {
                        info.total_tokens_in += tokens_in;
                        info.total_tokens_out += tokens_out;
                        info.total_turns = *turn;
                    }
                }
                AgentEvent::Done {
                    total_tokens_in,
                    total_tokens_out,
                    total_turns,
                } => {
                    if let Some(info) = self.agents.get_mut(&agent_id) {
                        info.status = AgentStatus::Done;
                        info.total_tokens_in = *total_tokens_in;
                        info.total_tokens_out = *total_tokens_out;
                        info.total_turns = *total_turns;
                    }
                    panel.push_chunk(LlmChunk::Done {
                        tokens_in: *total_tokens_in,
                        tokens_out: *total_tokens_out,
                    });
                }
                AgentEvent::Error(msg) => {
                    if let Some(info) = self.agents.get_mut(&agent_id) {
                        info.status = AgentStatus::Error(msg.clone());
                    }
                    panel.push_chunk(LlmChunk::Error(msg.clone()));
                }
                AgentEvent::Thinking(text) => {
                    panel.push_chunk(LlmChunk::Text(format!("💭 {}\n", text)));
                }
            }
        }
    }
}
