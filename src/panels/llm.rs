use crossterm::event::{KeyCode, KeyEvent};

/// Content chunk from the LLM stream
#[derive(Debug, Clone)]
pub enum LlmChunk {
    Text(String),
    ToolUse { name: String, input: String },
    Done { tokens_in: u64, tokens_out: u64 },
    Error(String),
}

pub struct LlmPanel {
    /// Accumulated output lines
    pub lines: Vec<String>,
    /// Current partial line being streamed
    pub current_line: String,
    /// Scroll offset from bottom (0 = following latest)
    pub scroll_offset: usize,
    /// Whether we're auto-following new output
    pub following: bool,
    /// Status info
    #[allow(dead_code)]
    pub model: String,
    pub tokens_in: u64,
    pub tokens_out: u64,
    pub streaming: bool,
    /// Raw assistant response text for current turn (saved to storage on Done)
    pub pending_response: String,

    /// Usage tracking
    pub usage: UsageTracker,
    /// Whether to show usage overlay in the LLM panel
    pub show_usage: bool,
}

/// Tracks LLM usage, costs, and session stats
pub struct UsageTracker {
    /// Session start time
    pub session_start: std::time::Instant,
    /// Total API calls made
    pub api_calls: u64,
    /// Per-turn usage history
    pub turns: Vec<TurnUsage>,
    /// Cost per million input tokens (default: Claude Sonnet pricing)
    pub cost_per_m_input: f64,
    /// Cost per million output tokens
    pub cost_per_m_output: f64,
    /// Model name for display
    pub model_name: String,
    /// Budget limit (optional, in dollars)
    pub budget_limit: Option<f64>,
}

#[derive(Debug, Clone)]
pub struct TurnUsage {
    pub tokens_in: u64,
    pub tokens_out: u64,
    pub timestamp: std::time::Instant,
}

impl UsageTracker {
    pub fn new() -> Self {
        Self {
            session_start: std::time::Instant::now(),
            api_calls: 0,
            turns: Vec::new(),
            // Claude Sonnet 4 pricing
            cost_per_m_input: 3.0,
            cost_per_m_output: 15.0,
            model_name: "claude-sonnet-4".to_string(),
            budget_limit: None,
        }
    }

    pub fn record_turn(&mut self, tokens_in: u64, tokens_out: u64) {
        self.api_calls += 1;
        self.turns.push(TurnUsage {
            tokens_in,
            tokens_out,
            timestamp: std::time::Instant::now(),
        });
    }

    pub fn total_tokens_in(&self) -> u64 {
        self.turns.iter().map(|t| t.tokens_in).sum()
    }

    pub fn total_tokens_out(&self) -> u64 {
        self.turns.iter().map(|t| t.tokens_out).sum()
    }

    pub fn total_cost(&self) -> f64 {
        let input_cost = self.total_tokens_in() as f64 / 1_000_000.0 * self.cost_per_m_input;
        let output_cost = self.total_tokens_out() as f64 / 1_000_000.0 * self.cost_per_m_output;
        input_cost + output_cost
    }

    pub fn session_duration(&self) -> std::time::Duration {
        self.session_start.elapsed()
    }

    pub fn cost_per_minute(&self) -> f64 {
        let mins = self.session_duration().as_secs_f64() / 60.0;
        if mins > 0.0 { self.total_cost() / mins } else { 0.0 }
    }

    pub fn budget_remaining(&self) -> Option<f64> {
        self.budget_limit.map(|limit| limit - self.total_cost())
    }

    pub fn budget_percent_used(&self) -> Option<f64> {
        self.budget_limit.map(|limit| {
            if limit > 0.0 { (self.total_cost() / limit * 100.0).min(100.0) } else { 100.0 }
        })
    }

    /// Tokens per minute (last 5 minutes)
    pub fn recent_rate(&self) -> (f64, f64) {
        let cutoff = std::time::Instant::now() - std::time::Duration::from_secs(300);
        let recent: Vec<&TurnUsage> = self.turns.iter().filter(|t| t.timestamp > cutoff).collect();
        let mins = 5.0f64.min(self.session_duration().as_secs_f64() / 60.0).max(0.01);
        let in_rate: f64 = recent.iter().map(|t| t.tokens_in as f64).sum::<f64>() / mins;
        let out_rate: f64 = recent.iter().map(|t| t.tokens_out as f64).sum::<f64>() / mins;
        (in_rate, out_rate)
    }

    /// Set pricing for a model
    pub fn set_model_pricing(&mut self, model: &str) {
        self.model_name = model.to_string();
        // Pricing per million tokens (as of 2025)
        let (input, output) = match model {
            m if m.contains("opus") => (15.0, 75.0),
            m if m.contains("sonnet") => (3.0, 15.0),
            m if m.contains("haiku") => (0.25, 1.25),
            m if m.contains("gpt-4o") => (2.50, 10.0),
            m if m.contains("gpt-4") => (10.0, 30.0),
            m if m.contains("o1") => (15.0, 60.0),
            m if m.contains("o3") => (10.0, 40.0),
            _ => (3.0, 15.0), // default to sonnet pricing
        };
        self.cost_per_m_input = input;
        self.cost_per_m_output = output;
    }
}

impl LlmPanel {
    pub fn new() -> Self {
        Self {
            lines: vec!["Welcome to RustPilot. Press Ctrl+3 to open the Prompt Manager.".into()],
            current_line: String::new(),
            scroll_offset: 0,
            following: true,
            model: String::new(),
            tokens_in: 0,
            tokens_out: 0,
            streaming: false,
            pending_response: String::new(),
            usage: UsageTracker::new(),
            show_usage: false,
        }
    }

    pub fn push_chunk(&mut self, chunk: LlmChunk) {
        match chunk {
            LlmChunk::Text(text) => {
                self.streaming = true;
                self.pending_response.push_str(&text);
                for ch in text.chars() {
                    if ch == '\n' {
                        self.lines.push(std::mem::take(&mut self.current_line));
                    } else {
                        self.current_line.push(ch);
                    }
                }
                if self.following {
                    self.scroll_offset = 0;
                }
            }
            LlmChunk::ToolUse { name, input } => {
                self.lines.push(format!("┌─ 🔧 {} ─", name));
                // Show compact input (first few lines)
                let input_lines: Vec<&str> = input.lines().collect();
                for line in input_lines.iter().take(5) {
                    self.lines.push(format!("│ {}", line));
                }
                if input_lines.len() > 5 {
                    self.lines.push(format!("│ ... ({} more lines)", input_lines.len() - 5));
                }
                self.lines.push("└──────────".into());
            }
            LlmChunk::Done {
                tokens_in,
                tokens_out,
            } => {
                // Flush current partial line
                if !self.current_line.is_empty() {
                    self.lines.push(std::mem::take(&mut self.current_line));
                }
                self.lines.push(String::new());
                self.tokens_in += tokens_in;
                self.tokens_out += tokens_out;
                self.streaming = false;
                self.usage.record_turn(tokens_in, tokens_out);
                // pending_response is consumed by App::poll_llm_updates to save to storage
            }
            LlmChunk::Error(msg) => {
                self.lines.push(format!("[ERROR] {}", msg));
                self.streaming = false;
                self.pending_response.clear();
            }
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(crossterm::event::KeyModifiers::CONTROL);
        match key.code {
            // Toggle usage monitor
            KeyCode::Char('u') if ctrl => {
                self.show_usage = !self.show_usage;
                return;
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.scroll_offset += 1;
                self.following = false;
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if self.scroll_offset > 0 {
                    self.scroll_offset -= 1;
                }
                if self.scroll_offset == 0 {
                    self.following = true;
                }
            }
            KeyCode::PageUp => {
                self.scroll_offset += 20;
                self.following = false;
            }
            KeyCode::PageDown => {
                self.scroll_offset = self.scroll_offset.saturating_sub(20);
                if self.scroll_offset == 0 {
                    self.following = true;
                }
            }
            // End = jump to bottom, follow
            KeyCode::End => {
                self.scroll_offset = 0;
                self.following = true;
            }
            _ => {}
        }
    }

    pub fn total_lines(&self) -> usize {
        self.lines.len() + if self.current_line.is_empty() { 0 } else { 1 }
    }
}
