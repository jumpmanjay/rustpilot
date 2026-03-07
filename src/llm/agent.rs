//! Agent: a stateful LLM session with tools, system prompt, conversation history,
//! and an agentic loop that auto-executes tool calls.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::mpsc;
use uuid::Uuid;

use super::tools::{ToolDef, ToolRegistry};

/// Unique identifier for an agent session
pub type AgentId = String;

/// Events emitted by an agent to the UI
#[derive(Debug, Clone)]
pub enum AgentEvent {
    /// Streaming text from the LLM
    Text(String),
    /// Agent is calling a tool
    ToolCall {
        name: String,
        input: String,
    },
    /// Tool execution result
    ToolOutput {
        name: String,
        output: String,
        is_error: bool,
    },
    /// Agent thinking/reasoning (if extended thinking is enabled)
    #[allow(dead_code)]
    Thinking(String),
    /// Turn complete with usage stats
    TurnDone {
        tokens_in: u64,
        tokens_out: u64,
        turn: usize,
    },
    /// Agent finished (no more tool calls, final response delivered)
    Done {
        total_tokens_in: u64,
        total_tokens_out: u64,
        total_turns: usize,
    },
    /// Error
    Error(String),
}

/// A message in the conversation history
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: String,
    pub content: MessageContent,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MessageContent {
    Text(String),
    Blocks(Vec<ContentBlock>),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ContentBlock {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "tool_use")]
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
    #[serde(rename = "tool_result")]
    ToolResult {
        tool_use_id: String,
        content: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        is_error: Option<bool>,
    },
}

/// Configuration for an agent
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    pub model: String,
    pub max_tokens: u32,
    pub system_prompt: String,
    pub max_turns: usize,
    #[serde(default)]
    pub rules: Vec<String>,
    #[serde(default)]
    pub skills: Vec<SkillConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillConfig {
    pub name: String,
    pub description: String,
    /// Path to skill instructions (loaded as system context)
    pub instructions_path: Option<String>,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            model: "claude-sonnet-4-20250514".into(),
            max_tokens: 16384,
            system_prompt: String::new(),
            max_turns: 20,
            rules: Vec::new(),
            skills: Vec::new(),
        }
    }
}

/// An agent session
pub struct Agent {
    pub id: AgentId,
    pub config: AgentConfig,
    pub history: Vec<Message>,
    pub cwd: String,
    total_tokens_in: u64,
    total_tokens_out: u64,
    total_turns: usize,
}

impl Agent {
    pub fn new(config: AgentConfig, cwd: &str) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            config,
            history: Vec::new(),
            cwd: cwd.to_string(),
            total_tokens_in: 0,
            total_tokens_out: 0,
            total_turns: 0,
        }
    }

    /// Build the full system prompt including rules and skills
    pub fn build_system_prompt(&self) -> String {
        let mut prompt = self.config.system_prompt.clone();

        if !self.config.rules.is_empty() {
            prompt.push_str("\n\n## Rules\n");
            for rule in &self.config.rules {
                prompt.push_str(&format!("- {}\n", rule));
            }
        }

        if !self.config.skills.is_empty() {
            prompt.push_str("\n\n## Available Skills\n");
            for skill in &self.config.skills {
                prompt.push_str(&format!("- **{}**: {}\n", skill.name, skill.description));
                if let Some(ref path) = skill.instructions_path {
                    if let Ok(instructions) = std::fs::read_to_string(path) {
                        prompt.push_str(&format!("\n### {} Instructions\n{}\n", skill.name, instructions));
                    }
                }
            }
        }

        prompt
    }

    /// Run the agentic loop: send message, execute tool calls, repeat until done
    pub async fn run(
        &mut self,
        user_message: &str,
        api_key: &str,
        tools: &ToolRegistry,
        tx: &mpsc::UnboundedSender<(AgentId, AgentEvent)>,
    ) {
        // Add user message to history
        self.history.push(Message {
            role: "user".into(),
            content: MessageContent::Text(user_message.to_string()),
        });

        let system_prompt = self.build_system_prompt();
        let tool_defs = tools.definitions();

        loop {
            self.total_turns += 1;
            if self.total_turns > self.config.max_turns {
                let _ = tx.send((
                    self.id.clone(),
                    AgentEvent::Error(format!("Max turns ({}) exceeded", self.config.max_turns)),
                ));
                break;
            }

            // Call the LLM
            let response = match call_anthropic(
                api_key,
                &self.config.model,
                self.config.max_tokens,
                &system_prompt,
                &self.history,
                &tool_defs,
                &self.id,
                tx,
            )
            .await
            {
                Ok(r) => r,
                Err(e) => {
                    let _ = tx.send((self.id.clone(), AgentEvent::Error(format!("{}", e))));
                    break;
                }
            };

            // Update token counts
            self.total_tokens_in += response.tokens_in;
            self.total_tokens_out += response.tokens_out;

            let _ = tx.send((
                self.id.clone(),
                AgentEvent::TurnDone {
                    tokens_in: response.tokens_in,
                    tokens_out: response.tokens_out,
                    turn: self.total_turns,
                },
            ));

            // Add assistant response to history
            self.history.push(Message {
                role: "assistant".into(),
                content: MessageContent::Blocks(response.content.clone()),
            });

            // Check for tool calls
            let tool_calls: Vec<_> = response
                .content
                .iter()
                .filter_map(|block| {
                    if let ContentBlock::ToolUse { id, name, input } = block {
                        Some((id.clone(), name.clone(), input.clone()))
                    } else {
                        None
                    }
                })
                .collect();

            if tool_calls.is_empty() || response.stop_reason == "end_turn" {
                // No tool calls — agent is done
                let _ = tx.send((
                    self.id.clone(),
                    AgentEvent::Done {
                        total_tokens_in: self.total_tokens_in,
                        total_tokens_out: self.total_tokens_out,
                        total_turns: self.total_turns,
                    },
                ));
                break;
            }

            // Execute tool calls and add results to history
            let mut tool_results = Vec::new();
            for (tool_id, tool_name, tool_input) in &tool_calls {
                let _ = tx.send((
                    self.id.clone(),
                    AgentEvent::ToolCall {
                        name: tool_name.clone(),
                        input: serde_json::to_string_pretty(tool_input).unwrap_or_default(),
                    },
                ));

                let result = tools.execute(tool_name, tool_id, tool_input, &self.cwd);

                let _ = tx.send((
                    self.id.clone(),
                    AgentEvent::ToolOutput {
                        name: tool_name.clone(),
                        output: result.content.clone(),
                        is_error: result.is_error,
                    },
                ));

                tool_results.push(ContentBlock::ToolResult {
                    tool_use_id: result.tool_use_id,
                    content: result.content,
                    is_error: if result.is_error { Some(true) } else { None },
                });
            }

            // Add tool results as user message
            self.history.push(Message {
                role: "user".into(),
                content: MessageContent::Blocks(tool_results),
            });
        }
    }
}

/// Response from a single LLM API call
struct LlmResponse {
    content: Vec<ContentBlock>,
    stop_reason: String,
    tokens_in: u64,
    tokens_out: u64,
}

/// Call the Anthropic API with streaming, emitting text chunks via tx
async fn call_anthropic(
    api_key: &str,
    model: &str,
    max_tokens: u32,
    system_prompt: &str,
    history: &[Message],
    tools: &[ToolDef],
    agent_id: &str,
    tx: &mpsc::UnboundedSender<(AgentId, AgentEvent)>,
) -> anyhow::Result<LlmResponse> {
    let client = reqwest::Client::new();

    let mut body = serde_json::json!({
        "model": model,
        "max_tokens": max_tokens,
        "stream": true,
        "messages": history,
    });

    if !system_prompt.is_empty() {
        body["system"] = serde_json::json!(system_prompt);
    }

    if !tools.is_empty() {
        let tool_defs: Vec<Value> = tools
            .iter()
            .map(|t| {
                serde_json::json!({
                    "name": t.name,
                    "description": t.description,
                    "input_schema": t.input_schema,
                })
            })
            .collect();
        body["tools"] = serde_json::json!(tool_defs);
    }

    let resp = client
        .post("https://api.anthropic.com/v1/messages")
        .header("x-api-key", api_key)
        .header("anthropic-version", "2023-06-01")
        .header("content-type", "application/json")
        .json(&body)
        .send()
        .await?;

    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        anyhow::bail!("API {}: {}", status, text);
    }

    use futures_util::StreamExt;

    let mut stream = resp.bytes_stream();
    let mut buffer = String::new();
    let mut tokens_in = 0u64;
    let mut tokens_out = 0u64;
    let mut stop_reason = String::new();

    // Accumulate content blocks
    let mut content_blocks: Vec<ContentBlock> = Vec::new();
    let mut current_block_type: Option<String> = None;
    let mut current_text = String::new();
    let mut current_tool_name = String::new();
    let mut current_tool_id = String::new();
    let mut current_tool_input = String::new();

    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        buffer.push_str(&String::from_utf8_lossy(&chunk));

        while let Some(event_end) = buffer.find("\n\n") {
            let event_str = buffer[..event_end].to_string();
            buffer = buffer[event_end + 2..].to_string();

            for line in event_str.lines() {
                if let Some(data) = line.strip_prefix("data: ") {
                    if data == "[DONE]" {
                        continue;
                    }
                    if let Ok(json) = serde_json::from_str::<Value>(data) {
                        let event_type = json["type"].as_str().unwrap_or("");
                        match event_type {
                            "message_start" => {
                                if let Some(usage) = json["message"]["usage"].as_object() {
                                    tokens_in = usage
                                        .get("input_tokens")
                                        .and_then(|v| v.as_u64())
                                        .unwrap_or(0);
                                }
                            }
                            "content_block_start" => {
                                let block_type = json["content_block"]["type"]
                                    .as_str()
                                    .unwrap_or("text")
                                    .to_string();
                                current_block_type = Some(block_type.clone());
                                current_text.clear();
                                current_tool_input.clear();

                                if block_type == "tool_use" {
                                    current_tool_name = json["content_block"]["name"]
                                        .as_str()
                                        .unwrap_or("")
                                        .to_string();
                                    current_tool_id = json["content_block"]["id"]
                                        .as_str()
                                        .unwrap_or("")
                                        .to_string();
                                }
                            }
                            "content_block_delta" => {
                                match current_block_type.as_deref() {
                                    Some("text") => {
                                        if let Some(text) = json["delta"]["text"].as_str() {
                                            current_text.push_str(text);
                                            let _ = tx.send((
                                                agent_id.to_string(),
                                                AgentEvent::Text(text.to_string()),
                                            ));
                                        }
                                    }
                                    Some("tool_use") => {
                                        if let Some(json_str) =
                                            json["delta"]["partial_json"].as_str()
                                        {
                                            current_tool_input.push_str(json_str);
                                        }
                                    }
                                    _ => {}
                                }
                            }
                            "content_block_stop" => {
                                match current_block_type.as_deref() {
                                    Some("text") => {
                                        content_blocks.push(ContentBlock::Text {
                                            text: current_text.clone(),
                                        });
                                    }
                                    Some("tool_use") => {
                                        let input: Value =
                                            serde_json::from_str(&current_tool_input)
                                                .unwrap_or(Value::Object(Default::default()));
                                        content_blocks.push(ContentBlock::ToolUse {
                                            id: current_tool_id.clone(),
                                            name: current_tool_name.clone(),
                                            input,
                                        });
                                    }
                                    _ => {}
                                }
                                current_block_type = None;
                            }
                            "message_delta" => {
                                if let Some(reason) = json["delta"]["stop_reason"].as_str() {
                                    stop_reason = reason.to_string();
                                }
                                if let Some(usage) = json["usage"].as_object() {
                                    tokens_out = usage
                                        .get("output_tokens")
                                        .and_then(|v| v.as_u64())
                                        .unwrap_or(0);
                                }
                            }
                            "message_stop" => {}
                            _ => {}
                        }
                    }
                }
            }
        }
    }

    Ok(LlmResponse {
        content: content_blocks,
        stop_reason,
        tokens_in,
        tokens_out,
    })
}
