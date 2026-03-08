/// Embedded terminal panel — runs a shell process and captures output.
use std::io::Read;
use std::process::{Child, Command, Stdio};

pub struct TerminalPanel {
    /// Output lines accumulated from the shell
    pub lines: Vec<String>,
    /// Current partial line
    #[allow(dead_code)]
    pub current_line: String,
    /// Input buffer for typing commands
    pub input: String,
    /// Scroll offset from bottom
    pub scroll_offset: usize,
    /// Following output
    pub following: bool,
    /// Shell process
    #[allow(dead_code)]
    child: Option<Child>,
    /// Whether the terminal is visible
    pub visible: bool,
}

impl TerminalPanel {
    pub fn new() -> Self {
        Self {
            lines: vec!["Terminal (Ctrl+` to toggle)".into()],
            current_line: String::new(),
            input: String::new(),
            scroll_offset: 0,
            following: true,
            child: None,
            visible: false,
        }
    }

    /// Run a command and capture output
    pub fn run_command(&mut self, cmd: &str, cwd: &str) {
        self.lines.push(format!("$ {}", cmd));

        match Command::new("bash")
            .args(["-c", cmd])
            .current_dir(cwd)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
        {
            Ok(mut child) => {
                // Read output synchronously (blocking — for MVP)
                let stdout = child.stdout.take().map(|mut s| {
                    let mut buf = String::new();
                    let _ = s.read_to_string(&mut buf);
                    buf
                }).unwrap_or_default();

                let stderr = child.stderr.take().map(|mut s| {
                    let mut buf = String::new();
                    let _ = s.read_to_string(&mut buf);
                    buf
                }).unwrap_or_default();

                let _ = child.wait();

                for line in stdout.lines() {
                    self.lines.push(line.to_string());
                }
                if !stderr.is_empty() {
                    for line in stderr.lines() {
                        self.lines.push(format!("[stderr] {}", line));
                    }
                }
            }
            Err(e) => {
                self.lines.push(format!("[error] {}", e));
            }
        }

        self.lines.push(String::new());
        if self.following {
            self.scroll_offset = 0;
        }
    }

    #[allow(dead_code)]
    pub fn total_lines(&self) -> usize {
        self.lines.len()
    }

    pub fn toggle(&mut self) {
        self.visible = !self.visible;
    }

    pub fn handle_input_char(&mut self, c: char) {
        self.input.push(c);
    }

    pub fn handle_backspace(&mut self) {
        self.input.pop();
    }

    pub fn handle_enter(&mut self, cwd: &str) {
        let cmd = self.input.clone();
        self.input.clear();
        if !cmd.is_empty() {
            self.run_command(&cmd, cwd);
        }
    }

    #[allow(dead_code)]
    pub fn scroll_up(&mut self, amount: usize) {
        self.scroll_offset += amount;
        self.following = false;
    }

    #[allow(dead_code)]
    pub fn scroll_down(&mut self, amount: usize) {
        self.scroll_offset = self.scroll_offset.saturating_sub(amount);
        if self.scroll_offset == 0 {
            self.following = true;
        }
    }
}
