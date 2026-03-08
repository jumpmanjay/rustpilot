/// Embedded terminal panel — runs a shell process and captures output.
use std::io::Read;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};

pub struct TerminalPanel {
    /// Output lines accumulated from the shell
    pub lines: Vec<String>,
    /// Input buffer for typing commands
    pub input: String,
    /// Scroll offset from bottom
    pub scroll_offset: usize,
    /// Following output
    pub following: bool,
    /// Whether the terminal is visible
    pub visible: bool,
    /// Background command state
    bg_output: Arc<Mutex<BgOutput>>,
    /// Whether a command is currently running
    pub running: bool,
    /// Running process (for kill)
    bg_child: Arc<Mutex<Option<u32>>>, // pid
    /// Command history
    pub history: Vec<String>,
    pub history_idx: Option<usize>,
}

struct BgOutput {
    lines: Vec<String>,
    done: bool,
    error: Option<String>,
}

impl TerminalPanel {
    pub fn new() -> Self {
        Self {
            lines: vec!["Terminal (Ctrl+` to toggle, Ctrl+C to kill)".into()],
            input: String::new(),
            scroll_offset: 0,
            following: true,
            visible: false,
            bg_output: Arc::new(Mutex::new(BgOutput {
                lines: Vec::new(),
                done: true,
                error: None,
            })),
            running: false,
            bg_child: Arc::new(Mutex::new(None)),
            history: Vec::new(),
            history_idx: None,
        }
    }

    /// Run a command asynchronously in a background thread
    pub fn run_command(&mut self, cmd: &str, cwd: &str) {
        self.lines.push(format!("$ {}", cmd));
        self.history.push(cmd.to_string());
        self.history_idx = None;

        // Reset background output
        {
            let mut bg = self.bg_output.lock().unwrap();
            bg.lines.clear();
            bg.done = false;
            bg.error = None;
        }

        let output = self.bg_output.clone();
        let child_pid = self.bg_child.clone();
        let cmd = cmd.to_string();
        let cwd = cwd.to_string();

        self.running = true;

        std::thread::spawn(move || {
            // Use login interactive shell so aliases (ll, etc.) work
            match Command::new("bash")
                .args(["-lic", &cmd])
                .current_dir(&cwd)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
            {
                Ok(mut child) => {
                    // Store PID for kill
                    {
                        let mut pid = child_pid.lock().unwrap();
                        *pid = Some(child.id());
                    }

                    let stdout = child.stdout.take();
                    let stderr = child.stderr.take();

                    // Read stdout in a thread
                    let out_lines = output.clone();
                    let stdout_handle = std::thread::spawn(move || {
                        if let Some(mut reader) = stdout {
                            let mut buf = [0u8; 4096];
                            let mut partial = String::new();
                            loop {
                                match reader.read(&mut buf) {
                                    Ok(0) => break,
                                    Ok(n) => {
                                        let text = String::from_utf8_lossy(&buf[..n]);
                                        partial.push_str(&text);
                                        // Split on newlines and buffer
                                        while let Some(pos) = partial.find('\n') {
                                            let line = partial[..pos].to_string();
                                            // Strip ANSI escape codes for clean display
                                            let clean = strip_ansi(&line);
                                            let mut bg = out_lines.lock().unwrap();
                                            bg.lines.push(clean);
                                            partial = partial[pos + 1..].to_string();
                                        }
                                    }
                                    Err(_) => break,
                                }
                            }
                            if !partial.is_empty() {
                                let clean = strip_ansi(&partial);
                                let mut bg = out_lines.lock().unwrap();
                                bg.lines.push(clean);
                            }
                        }
                    });

                    // Read stderr
                    let err_lines = output.clone();
                    let stderr_handle = std::thread::spawn(move || {
                        if let Some(mut reader) = stderr {
                            let mut buf = String::new();
                            let _ = reader.read_to_string(&mut buf);
                            if !buf.is_empty() {
                                let mut bg = err_lines.lock().unwrap();
                                for line in buf.lines() {
                                    bg.lines.push(format!("[stderr] {}", strip_ansi(line)));
                                }
                            }
                        }
                    });

                    let _ = stdout_handle.join();
                    let _ = stderr_handle.join();
                    let _ = child.wait();

                    // Clear PID
                    {
                        let mut pid = child_pid.lock().unwrap();
                        *pid = None;
                    }

                    let mut bg = output.lock().unwrap();
                    bg.done = true;
                }
                Err(e) => {
                    let mut bg = output.lock().unwrap();
                    bg.error = Some(format!("[error] {}", e));
                    bg.done = true;
                }
            }
        });
    }

    /// Poll for new output from background command (call from main loop)
    pub fn poll(&mut self) {
        let mut bg = self.bg_output.lock().unwrap();

        // Drain any new lines
        if !bg.lines.is_empty() {
            self.lines.extend(bg.lines.drain(..));
            if self.following {
                self.scroll_offset = 0;
            }
        }

        if let Some(ref err) = bg.error.take() {
            self.lines.push(err.clone());
        }

        if bg.done && self.running {
            self.running = false;
            self.lines.push(String::new());
        }
    }

    /// Kill the running command (Ctrl+C)
    pub fn kill_running(&mut self) {
        let pid = self.bg_child.lock().unwrap().take();
        if let Some(pid) = pid {
            // Kill the process group
            unsafe {
                libc::kill(-(pid as i32), libc::SIGTERM);
            }
            self.lines.push("^C (killed)".into());
            self.running = false;
        }
    }

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

    pub fn history_up(&mut self) {
        if self.history.is_empty() { return; }
        let idx = match self.history_idx {
            None => self.history.len() - 1,
            Some(0) => 0,
            Some(i) => i - 1,
        };
        self.history_idx = Some(idx);
        self.input = self.history[idx].clone();
    }

    pub fn history_down(&mut self) {
        match self.history_idx {
            None => {}
            Some(i) if i + 1 >= self.history.len() => {
                self.history_idx = None;
                self.input.clear();
            }
            Some(i) => {
                self.history_idx = Some(i + 1);
                self.input = self.history[i + 1].clone();
            }
        }
    }

    pub fn scroll_up(&mut self, amount: usize) {
        self.scroll_offset += amount;
        self.following = false;
    }

    pub fn scroll_down(&mut self, amount: usize) {
        self.scroll_offset = self.scroll_offset.saturating_sub(amount);
        if self.scroll_offset == 0 {
            self.following = true;
        }
    }
}

/// Strip ANSI escape codes from a string
fn strip_ansi(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut in_escape = false;
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            in_escape = true;
            continue;
        }
        if in_escape {
            if c == '[' {
                // CSI sequence — consume until letter
                while let Some(&next) = chars.peek() {
                    chars.next();
                    if next.is_ascii_alphabetic() { break; }
                }
            }
            in_escape = false;
            continue;
        }
        // Skip other control chars except tab
        if c == '\t' {
            result.push_str("    ");
        } else if c >= ' ' || c == '\n' {
            result.push(c);
        }
    }
    result
}
