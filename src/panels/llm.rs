use crossterm::event::{KeyCode, KeyEvent};

/// Content chunk from the LLM stream
#[derive(Debug, Clone)]
pub enum LlmChunk {
    Text(String),
    #[allow(dead_code)]
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
        }
    }

    pub fn push_chunk(&mut self, chunk: LlmChunk) {
        match chunk {
            LlmChunk::Text(text) => {
                self.streaming = true;
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
                self.lines.push(format!("── Tool: {} ──", name));
                for line in input.lines() {
                    self.lines.push(format!("  {}", line));
                }
                self.lines.push("──────────".into());
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
            }
            LlmChunk::Error(msg) => {
                self.lines.push(format!("[ERROR] {}", msg));
                self.streaming = false;
            }
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent) {
        match key.code {
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
