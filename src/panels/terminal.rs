/// Embedded terminal panel using a real PTY (pseudo-terminal).
/// Supports interactive commands (top, vim, etc.), Ctrl+C, and proper signal isolation.

use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::sync::{Arc, Mutex};

pub struct TerminalPanel {
    /// Output lines for display
    pub lines: Vec<String>,
    /// Current partial line (no newline yet)
    current_line: String,
    /// Input buffer for typing commands
    pub input: String,
    /// Scroll offset from bottom
    pub scroll_offset: usize,
    /// Following output
    pub following: bool,
    /// Whether the terminal is visible
    pub visible: bool,
    /// Whether a command is running
    pub running: bool,
    /// Command history
    pub history: Vec<String>,
    pub history_idx: Option<usize>,
    /// PTY master fd for writing input to the child
    pty_master: Arc<Mutex<Option<OwnedFd>>>,
    /// Shared output buffer from PTY reader thread
    pty_output: Arc<Mutex<Vec<String>>>,
    /// Child PID
    child_pid: Arc<Mutex<Option<i32>>>,
    /// Done flag
    pty_done: Arc<Mutex<bool>>,
}

impl TerminalPanel {
    pub fn new() -> Self {
        Self {
            lines: vec!["Terminal (Ctrl+` to toggle)".into()],
            current_line: String::new(),
            input: String::new(),
            scroll_offset: 0,
            following: true,
            visible: false,
            running: false,
            history: Vec::new(),
            history_idx: None,
            pty_master: Arc::new(Mutex::new(None)),
            pty_output: Arc::new(Mutex::new(Vec::new())),
            child_pid: Arc::new(Mutex::new(None)),
            pty_done: Arc::new(Mutex::new(true)),
        }
    }

    /// Run a command in a PTY
    pub fn run_command(&mut self, cmd: &str, cwd: &str) {
        // Don't run if something is already running
        if self.running {
            self.lines.push("[busy] Previous command still running. Ctrl+C to kill.".into());
            return;
        }

        self.lines.push(format!("$ {}", cmd));
        self.history.push(cmd.to_string());
        self.history_idx = None;

        // Create PTY
        let pty_result = nix::pty::openpty(None, None);
        let pty = match pty_result {
            Ok(pty) => pty,
            Err(e) => {
                self.lines.push(format!("[error] Failed to open PTY: {}", e));
                return;
            }
        };

        let master_fd = pty.master;
        let slave_fd = pty.slave;

        // Set terminal size on the PTY
        let ws = nix::pty::Winsize {
            ws_row: 24,
            ws_col: 120,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        // Safe: we own the fd
        unsafe {
            libc::ioctl(master_fd.as_raw_fd(), libc::TIOCSWINSZ, &ws);
        }

        // Fork
        let cmd_str = cmd.to_string();
        let cwd_str = cwd.to_string();

        match unsafe { libc::fork() } {
            -1 => {
                self.lines.push("[error] Fork failed".into());
                return;
            }
            0 => {
                // ─── Child process ───
                // Create new session (detach from parent's terminal)
                unsafe { libc::setsid(); }

                // Set slave as controlling terminal
                unsafe {
                    libc::ioctl(slave_fd.as_raw_fd(), libc::TIOCSCTTY as _, 0);
                }

                // Redirect stdin/stdout/stderr to slave
                let slave_raw = slave_fd.as_raw_fd();
                unsafe {
                    libc::dup2(slave_raw, 0); // stdin
                    libc::dup2(slave_raw, 1); // stdout
                    libc::dup2(slave_raw, 2); // stderr
                }

                // Close extra fds
                drop(master_fd);
                if slave_raw > 2 {
                    drop(slave_fd);
                }

                // Change directory
                let _ = std::env::set_current_dir(&cwd_str);

                // Set TERM for proper behavior
                unsafe { std::env::set_var("TERM", "xterm-256color"); }

                // Exec bash with the command
                // Source aliases, then run command
                let shell_cmd = format!(
                    "[ -f ~/.bash_aliases ] && . ~/.bash_aliases 2>/dev/null; \
                     [ -f ~/.bashrc ] && . ~/.bashrc 2>/dev/null; \
                     {}",
                    cmd_str
                );

                let c_shell = std::ffi::CString::new("/bin/bash").unwrap();
                let c_arg0 = std::ffi::CString::new("bash").unwrap();
                let c_arg1 = std::ffi::CString::new("-c").unwrap();
                let c_arg2 = std::ffi::CString::new(shell_cmd).unwrap();

                // This replaces the process
                nix::unistd::execv(&c_shell, &[&c_arg0, &c_arg1, &c_arg2]).ok();
                std::process::exit(1);
            }
            child_pid => {
                // ─── Parent process ───
                drop(slave_fd); // Close slave in parent

                // Store state
                {
                    let mut pid = self.child_pid.lock().unwrap();
                    *pid = Some(child_pid);
                }
                {
                    let mut done = self.pty_done.lock().unwrap();
                    *done = false;
                }

                // Store master fd for writing
                // We need to clone the fd for the reader thread
                let master_raw = master_fd.as_raw_fd();
                let reader_fd = unsafe { OwnedFd::from_raw_fd(libc::dup(master_raw)) };

                {
                    let mut master = self.pty_master.lock().unwrap();
                    *master = Some(master_fd);
                }

                self.running = true;

                // Spawn reader thread
                let output = self.pty_output.clone();
                let done_flag = self.pty_done.clone();
                let pid_ref = self.child_pid.clone();

                std::thread::spawn(move || {
                    let mut file = unsafe { std::fs::File::from_raw_fd(reader_fd.as_raw_fd()) };
                    // Prevent double-close
                    std::mem::forget(reader_fd);

                    let mut buf = [0u8; 4096];
                    loop {
                        match file.read(&mut buf) {
                            Ok(0) => break,
                            Ok(n) => {
                                let text = String::from_utf8_lossy(&buf[..n]);
                                let cleaned = strip_ansi_and_control(&text);
                                if !cleaned.is_empty() {
                                    let mut out = output.lock().unwrap();
                                    out.push(cleaned);
                                }
                            }
                            Err(e) => {
                                // EIO is expected when child exits
                                if e.raw_os_error() != Some(5) {
                                    let mut out = output.lock().unwrap();
                                    out.push(format!("[error] Read: {}", e));
                                }
                                break;
                            }
                        }
                    }

                    // Wait for child
                    let pid = {
                        let p = pid_ref.lock().unwrap();
                        *p
                    };
                    if let Some(p) = pid {
                        unsafe { libc::waitpid(p, std::ptr::null_mut(), 0); }
                    }

                    let mut done = done_flag.lock().unwrap();
                    *done = true;
                });
            }
        }
    }

    /// Send raw bytes to the PTY (for interactive input like Ctrl+C)
    fn write_to_pty(&self, data: &[u8]) {
        if let Ok(master) = self.pty_master.lock() {
            if let Some(ref fd) = *master {
                let raw = fd.as_raw_fd();
                unsafe {
                    libc::write(raw, data.as_ptr() as *const libc::c_void, data.len());
                }
            }
        }
    }

    /// Send Ctrl+C (SIGINT) to the terminal process
    pub fn send_ctrl_c(&mut self) {
        if self.running {
            // Write ETX (Ctrl+C) to PTY — the terminal driver sends SIGINT
            self.write_to_pty(&[0x03]);
            self.lines.push("^C".into());
        }
    }

    /// Send Ctrl+D (EOF) to the terminal
    pub fn send_ctrl_d(&mut self) {
        if self.running {
            self.write_to_pty(&[0x04]);
        }
    }

    /// Kill the running process forcefully
    pub fn kill_running(&mut self) {
        if let Ok(pid) = self.child_pid.lock() {
            if let Some(p) = *pid {
                unsafe { libc::kill(p, libc::SIGKILL); }
                self.lines.push("[killed]".into());
            }
        }
        self.running = false;
    }

    /// Poll for new output (call from main loop)
    pub fn poll(&mut self) {
        // Drain output
        {
            let mut out = self.pty_output.lock().unwrap();
            for chunk in out.drain(..) {
                // Split into lines
                for part in chunk.split('\n') {
                    if !part.is_empty() {
                        self.current_line.push_str(part);
                    }
                    // Each \n means flush current line
                    if chunk.contains('\n') {
                        if !self.current_line.is_empty() {
                            self.lines.push(std::mem::take(&mut self.current_line));
                        }
                    }
                }
            }
            if self.following {
                self.scroll_offset = 0;
            }
        }

        // Check done
        {
            let done = self.pty_done.lock().unwrap();
            if *done && self.running {
                // Flush partial line
                if !self.current_line.is_empty() {
                    self.lines.push(std::mem::take(&mut self.current_line));
                }
                self.lines.push(String::new());
                self.running = false;

                // Clean up
                let mut master = self.pty_master.lock().unwrap();
                *master = None;
                let mut pid = self.child_pid.lock().unwrap();
                *pid = None;
            }
        }
    }

    pub fn total_lines(&self) -> usize {
        self.lines.len()
    }

    pub fn toggle(&mut self) {
        self.visible = !self.visible;
    }

    pub fn handle_input_char(&mut self, c: char) {
        if self.running {
            // Send directly to PTY for interactive programs
            let mut buf = [0u8; 4];
            let s = c.encode_utf8(&mut buf);
            self.write_to_pty(s.as_bytes());
        } else {
            self.input.push(c);
        }
    }

    pub fn handle_backspace(&mut self) {
        if self.running {
            self.write_to_pty(&[0x7f]); // DEL
        } else {
            self.input.pop();
        }
    }

    pub fn handle_enter(&mut self, cwd: &str) {
        if self.running {
            self.write_to_pty(b"\n");
        } else {
            let cmd = self.input.clone();
            self.input.clear();
            if !cmd.is_empty() {
                self.run_command(&cmd, cwd);
            }
        }
    }

    pub fn history_up(&mut self) {
        if self.running || self.history.is_empty() { return; }
        let idx = match self.history_idx {
            None => self.history.len() - 1,
            Some(0) => 0,
            Some(i) => i - 1,
        };
        self.history_idx = Some(idx);
        self.input = self.history[idx].clone();
    }

    pub fn history_down(&mut self) {
        if self.running { return; }
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

/// Strip ANSI escape sequences and control characters
fn strip_ansi_and_control(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            // ESC sequence
            if chars.peek() == Some(&'[') {
                chars.next(); // consume '['
                // CSI: consume until alphabetic terminator
                while let Some(&next) = chars.peek() {
                    chars.next();
                    if next.is_ascii_alphabetic() { break; }
                }
            } else if chars.peek() == Some(&']') {
                // OSC: consume until ST (ESC \ or BEL)
                chars.next();
                while let Some(&next) = chars.peek() {
                    chars.next();
                    if next == '\x07' { break; } // BEL
                    if next == '\x1b' {
                        if chars.peek() == Some(&'\\') { chars.next(); break; }
                    }
                }
            } else {
                // Other ESC sequences — skip next char
                chars.next();
            }
            continue;
        }
        if c == '\r' { continue; } // skip carriage return
        if c == '\t' {
            result.push_str("    ");
            continue;
        }
        if c == '\n' || c >= ' ' {
            result.push(c);
        }
    }
    result
}
