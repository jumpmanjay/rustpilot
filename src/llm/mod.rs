use tokio::sync::mpsc;

use crate::config::Config;
use crate::panels::llm::{LlmChunk, LlmPanel};

pub struct LlmManager {
    provider: String,
    model: String,
    api_key: String,
    max_tokens: u32,
    /// Receiver for chunks from async streaming tasks
    rx: mpsc::UnboundedReceiver<LlmChunk>,
    /// Sender cloned into each streaming task
    tx: mpsc::UnboundedSender<LlmChunk>,
}

impl LlmManager {
    pub fn new(config: &Config) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        Self {
            provider: config.llm.provider.clone(),
            model: config.llm.model.clone(),
            api_key: config.llm.api_key.clone(),
            max_tokens: config.llm.max_tokens,
            rx,
            tx,
        }
    }

    /// Send a prompt to the LLM — spawns an async task that streams chunks back
    pub fn send_prompt(&self, prompt: &str) {
        let api_key = self.api_key.clone();
        let model = self.model.clone();
        let max_tokens = self.max_tokens;
        let prompt = prompt.to_string();
        let tx = self.tx.clone();

        if api_key.is_empty() {
            let _ = tx.send(LlmChunk::Error(
                "No API key configured. Set llm.api_key in ~/.rustpilot/config.toml".into(),
            ));
            return;
        }

        tokio::spawn(async move {
            let result = stream_anthropic(&api_key, &model, max_tokens, &prompt, &tx).await;
            if let Err(e) = result {
                let _ = tx.send(LlmChunk::Error(format!("{}", e)));
            }
        });
    }

    /// Poll for any pending LLM stream updates (non-blocking)
    pub fn poll_updates(&mut self, panel: &mut LlmPanel) {
        while let Ok(chunk) = self.rx.try_recv() {
            panel.push_chunk(chunk);
        }
    }
}

async fn stream_anthropic(
    api_key: &str,
    model: &str,
    max_tokens: u32,
    prompt: &str,
    tx: &mpsc::UnboundedSender<LlmChunk>,
) -> anyhow::Result<()> {
    let client = reqwest::Client::new();

    let body = serde_json::json!({
        "model": model,
        "max_tokens": max_tokens,
        "stream": true,
        "messages": [
            {"role": "user", "content": prompt}
        ]
    });

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
        let _ = tx.send(LlmChunk::Error(format!("API {}: {}", status, text)));
        return Ok(());
    }

    use futures_util::StreamExt;

    let mut stream = resp.bytes_stream();
    let mut buffer = String::new();
    let mut tokens_in = 0u64;
    let mut tokens_out = 0u64;

    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        buffer.push_str(&String::from_utf8_lossy(&chunk));

        // Parse SSE events from buffer
        while let Some(event_end) = buffer.find("\n\n") {
            let event_str = buffer[..event_end].to_string();
            buffer = buffer[event_end + 2..].to_string();

            for line in event_str.lines() {
                if let Some(data) = line.strip_prefix("data: ") {
                    if data == "[DONE]" {
                        let _ = tx.send(LlmChunk::Done {
                            tokens_in,
                            tokens_out,
                        });
                        return Ok(());
                    }
                    if let Ok(json) = serde_json::from_str::<serde_json::Value>(data) {
                        let event_type = json["type"].as_str().unwrap_or("");
                        match event_type {
                            "content_block_delta" => {
                                if let Some(text) =
                                    json["delta"]["text"].as_str()
                                {
                                    let _ = tx.send(LlmChunk::Text(text.to_string()));
                                }
                            }
                            "message_delta" => {
                                if let Some(usage) = json["usage"].as_object() {
                                    tokens_out =
                                        usage.get("output_tokens")
                                            .and_then(|v| v.as_u64())
                                            .unwrap_or(0);
                                }
                            }
                            "message_start" => {
                                if let Some(usage) =
                                    json["message"]["usage"].as_object()
                                {
                                    tokens_in =
                                        usage.get("input_tokens")
                                            .and_then(|v| v.as_u64())
                                            .unwrap_or(0);
                                }
                            }
                            "message_stop" => {
                                let _ = tx.send(LlmChunk::Done {
                                    tokens_in,
                                    tokens_out,
                                });
                                return Ok(());
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
    }

    // Stream ended without explicit stop
    let _ = tx.send(LlmChunk::Done {
        tokens_in,
        tokens_out,
    });
    Ok(())
}
