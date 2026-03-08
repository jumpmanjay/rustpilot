use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::collections::HashMap;

use super::editor::TextBuffer;
use super::prompt::PromptPanel;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditorMode {
    Files,
    SourceControl,
}

pub struct CodePanel {
    pub mode: EditorMode,

    // File explorer state
    pub cwd: String,
    pub entries: Vec<FileEntry>,
    pub selected_idx: usize,
    pub tree_scroll: usize,

    // Editor state
    pub file_path: Option<String>,
    pub buffer: TextBuffer,

    // Open file buffers — preserves unsaved changes when switching files
    pub open_buffers: HashMap<String, TextBuffer>,

    // Source control state
    pub scm: ScmState,

    // Viewport size (set during render)
    pub viewport_height: usize,

    /// Show hidden (dot) files in explorer
    pub show_hidden: bool,

    /// Tab scroll offset (for when many tabs are open)
    pub tab_scroll: usize,
}

#[derive(Debug, Clone)]
pub struct ScmState {
    pub entries: Vec<ScmEntry>,
    pub selected_idx: usize,
    /// The diff content for the currently selected file
    pub diff_lines: Vec<DiffLine>,
    pub diff_scroll: usize,
    /// Current branch name
    pub branch: String,
    /// Summary counts
    pub staged: usize,
    pub unstaged: usize,
    pub untracked: usize,
}

#[derive(Debug, Clone)]
pub struct ScmEntry {
    pub path: String,
    pub status: ScmStatus,
    pub staged: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScmStatus {
    Modified,
    Added,
    Deleted,
    Renamed,
    Untracked,
}

#[derive(Debug, Clone)]
pub struct DiffLine {
    pub kind: DiffLineKind,
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffLineKind {
    Context,
    Added,
    Removed,
    Header,
}

#[derive(Debug, Clone)]
pub struct FileEntry {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    #[allow(dead_code)]
    pub depth: usize,
    #[allow(dead_code)]
    pub expanded: bool,
}

impl CodePanel {
    pub fn new() -> Self {
        let cwd = std::env::current_dir()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| ".".to_string());

        let mut panel = Self {
            mode: EditorMode::Files,
            cwd: cwd.clone(),
            entries: Vec::new(),
            selected_idx: 0,
            tree_scroll: 0,
            file_path: None,
            buffer: TextBuffer::new(),
            open_buffers: HashMap::new(),
            scm: ScmState {
                entries: Vec::new(),
                selected_idx: 0,
                diff_lines: Vec::new(),
                diff_scroll: 0,
                branch: String::new(),
                staged: 0,
                unstaged: 0,
                untracked: 0,
            },
            viewport_height: 24,
            show_hidden: true,
            tab_scroll: 0,
        };
        panel.refresh_entries();
        panel
    }

    pub fn refresh_entries(&mut self) {
        self.entries.clear();

        // Add ../ entry if not at root
        if let Some(parent) = std::path::Path::new(&self.cwd).parent() {
            self.entries.push(FileEntry {
                name: "..".to_string(),
                path: parent.to_string_lossy().to_string(),
                is_dir: true,
                depth: 0,
                expanded: false,
            });
        }

        if let Ok(rd) = std::fs::read_dir(&self.cwd) {
            let mut items: Vec<_> = rd
                .filter_map(|e| e.ok())
                .filter(|e| {
                    // Show/hide dotfiles based on setting
                    self.show_hidden || !e.file_name().to_string_lossy().starts_with('.')
                })
                .map(|e| {
                    let is_dir = e.file_type().map(|t| t.is_dir()).unwrap_or(false);
                    FileEntry {
                        name: e.file_name().to_string_lossy().to_string(),
                        path: e.path().to_string_lossy().to_string(),
                        is_dir,
                        depth: 0,
                        expanded: false,
                    }
                })
                .collect();
            items.sort_by(|a, b| {
                b.is_dir
                    .cmp(&a.is_dir)
                    .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            });
            self.entries.extend(items);
        }
    }

    pub fn open_file(&mut self, path: &str) {
        // Stash current buffer if we have a file open
        if let Some(ref current_path) = self.file_path {
            let current_path = current_path.clone();
            let current_buf = std::mem::replace(&mut self.buffer, TextBuffer::new());
            self.open_buffers.insert(current_path, current_buf);
        }

        // Check if we already have this file open (with unsaved changes)
        if let Some(buf) = self.open_buffers.remove(path) {
            self.file_path = Some(path.to_string());
            self.buffer = buf;
        } else if let Ok(content) = std::fs::read_to_string(path) {
            self.file_path = Some(path.to_string());
            self.buffer = TextBuffer::from_string(&content);
        }

        // Set comment prefix based on file extension
        if let Some(ext) = std::path::Path::new(path).extension().and_then(|e| e.to_str()) {
            self.buffer.set_comment_for_ext(ext);
        }
    }

    /// Get list of all open buffer paths (for tab bar)
    pub fn open_buffer_paths(&self) -> Vec<String> {
        let mut paths: Vec<String> = self.open_buffers.keys().cloned().collect();
        if let Some(ref current) = self.file_path {
            // Current file first
            paths.retain(|p| p != current);
            paths.insert(0, current.clone());
        }
        paths
    }

    /// Switch to a specific open buffer by path
    #[allow(dead_code)]
    pub fn switch_to_buffer(&mut self, path: &str) {
        if self.file_path.as_deref() == Some(path) {
            return; // already active
        }
        self.open_file(path);
    }

    /// Close a buffer by path. Returns true if closed.
    #[allow(dead_code)]
    pub fn close_buffer(&mut self, path: &str) -> bool {
        if self.file_path.as_deref() == Some(path) {
            // Closing active buffer — switch to another or clear
            let other: Option<String> = self.open_buffers.keys().next().cloned();
            if let Some(next) = other {
                self.open_file(&next);
            } else {
                self.file_path = None;
                self.buffer = TextBuffer::new();
            }
            true
        } else {
            self.open_buffers.remove(path).is_some()
        }
    }

    /// Create a new untitled buffer
    pub fn new_file(&mut self) {
        // Stash current buffer
        if let Some(ref current_path) = self.file_path {
            let current_path = current_path.clone();
            let current_buf = std::mem::replace(&mut self.buffer, TextBuffer::new());
            self.open_buffers.insert(current_path, current_buf);
        }
        self.file_path = None;
        self.buffer = TextBuffer::new();
        self.buffer.modified = true;
    }

    /// Close the current tab. Returns true if closed.
    pub fn close_current_tab(&mut self) -> bool {
        if let Some(ref path) = self.file_path.clone() {
            self.close_buffer(path)
        } else {
            // Untitled buffer — switch to next open buffer or clear
            let other: Option<String> = self.open_buffers.keys().next().cloned();
            if let Some(next) = other {
                self.open_file(&next);
            } else {
                self.buffer = TextBuffer::new();
            }
            true
        }
    }

    /// Save current buffer to a specific path (Save As)
    pub fn save_file_as(&mut self, path: &str) -> bool {
        let content = self.buffer.to_string();
        if std::fs::write(path, &content).is_ok() {
            // Remove from old path if it existed
            if let Some(ref old_path) = self.file_path {
                self.open_buffers.remove(old_path);
            }
            self.file_path = Some(path.to_string());
            self.buffer.modified = false;
            // Set comment prefix for new extension
            if let Some(ext) = std::path::Path::new(path).extension().and_then(|e| e.to_str()) {
                self.buffer.set_comment_for_ext(ext);
            }
            true
        } else {
            false
        }
    }

    /// Public explorer key handler (called from App when Explorer panel is focused)
    pub fn handle_explorer_key_pub(&mut self, key: KeyEvent, prompt: &mut PromptPanel) {
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                if self.selected_idx > 0 {
                    self.selected_idx -= 1;
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if self.selected_idx + 1 < self.entries.len() {
                    self.selected_idx += 1;
                }
            }
            KeyCode::Enter => {
                if let Some(entry) = self.entries.get(self.selected_idx).cloned() {
                    if entry.is_dir {
                        self.cwd = entry.path;
                        self.selected_idx = 0;
                        self.refresh_entries();
                    } else {
                        self.open_file(&entry.path);
                    }
                }
            }
            KeyCode::Backspace => {
                if let Some(parent) = std::path::Path::new(&self.cwd).parent() {
                    self.cwd = parent.to_string_lossy().to_string();
                    self.selected_idx = 0;
                    self.refresh_entries();
                }
            }
            KeyCode::Char('r') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if let Some(entry) = self.entries.get(self.selected_idx) {
                    prompt.insert_reference(&format!("@{}", entry.path), false);
                }
            }
            KeyCode::Char('R') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if let Some(entry) = self.entries.get(self.selected_idx) {
                    prompt.insert_reference(&format!("@@{}", entry.path), true);
                }
            }
            _ => {}
        }
    }

    /// Public editor key handler (called from App when Editor panel is focused)
    pub fn handle_editor_key_pub(&mut self, key: KeyEvent, prompt: &mut PromptPanel) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);

        match key.code {
            KeyCode::Char('s') if ctrl => {
                if let Some(path) = self.save_file() {
                    prompt.record_saved_file(&path);
                }
                return;
            }
            KeyCode::Char('r') if ctrl => {
                if let Some(ref path) = self.file_path {
                    let line_ref = self.make_line_ref(path, "@");
                    prompt.insert_reference(&line_ref, false);
                }
                return;
            }
            KeyCode::Char('R') if ctrl => {
                if let Some(ref path) = self.file_path {
                    let line_ref = self.make_line_ref(path, "@@");
                    prompt.insert_reference(&line_ref, true);
                }
                return;
            }
            _ => {}
        }

        self.buffer.handle_key(key, self.viewport_height);
    }

    fn make_line_ref(&self, path: &str, prefix: &str) -> String {
        if let Some((sr, _, er, _)) = self.buffer.selection_range() {
            let start = sr + 1;
            let end = er + 1;
            if start == end {
                format!("{}{}:{}", prefix, path, start)
            } else {
                format!("{}{}:{}-{}", prefix, path, start, end)
            }
        } else {
            format!("{}{}:{}", prefix, path, self.buffer.cursor_row + 1)
        }
    }

    // ─── Source Control ───

    pub fn toggle_mode(&mut self) {
        match self.mode {
            EditorMode::Files => {
                self.mode = EditorMode::SourceControl;
                self.refresh_scm();
            }
            EditorMode::SourceControl => {
                self.mode = EditorMode::Files;
            }
        }
    }

    pub fn refresh_scm(&mut self) {
        self.scm.entries.clear();
        self.scm.staged = 0;
        self.scm.unstaged = 0;
        self.scm.untracked = 0;

        // Get branch
        if let Ok(output) = std::process::Command::new("git")
            .args(["branch", "--show-current"])
            .current_dir(&self.cwd)
            .output()
        {
            self.scm.branch = String::from_utf8_lossy(&output.stdout).trim().to_string();
        }

        // Get status (porcelain v1 for easy parsing)
        if let Ok(output) = std::process::Command::new("git")
            .args(["status", "--porcelain"])
            .current_dir(&self.cwd)
            .output()
        {
            let status_str = String::from_utf8_lossy(&output.stdout);
            for line in status_str.lines() {
                if line.len() < 4 {
                    continue;
                }
                let index_status = line.as_bytes()[0];
                let worktree_status = line.as_bytes()[1];
                let path = line[3..].to_string();

                let (status, staged) = match (index_status, worktree_status) {
                    (b'?', b'?') => {
                        self.scm.untracked += 1;
                        (ScmStatus::Untracked, false)
                    }
                    (b'A', _) => {
                        self.scm.staged += 1;
                        (ScmStatus::Added, true)
                    }
                    (b'D', _) => {
                        self.scm.staged += 1;
                        (ScmStatus::Deleted, true)
                    }
                    (b'R', _) => {
                        self.scm.staged += 1;
                        (ScmStatus::Renamed, true)
                    }
                    (b'M', _) => {
                        self.scm.staged += 1;
                        (ScmStatus::Modified, true)
                    }
                    (_, b'M') => {
                        self.scm.unstaged += 1;
                        (ScmStatus::Modified, false)
                    }
                    (_, b'D') => {
                        self.scm.unstaged += 1;
                        (ScmStatus::Deleted, false)
                    }
                    _ => {
                        self.scm.unstaged += 1;
                        (ScmStatus::Modified, false)
                    }
                };

                self.scm.entries.push(ScmEntry {
                    path,
                    status,
                    staged,
                });
            }
        }

        self.scm.selected_idx = 0;
        if !self.scm.entries.is_empty() {
            self.load_diff_for_selected();
        }
    }

    fn load_diff_for_selected(&mut self) {
        self.scm.diff_lines.clear();
        self.scm.diff_scroll = 0;

        if let Some(entry) = self.scm.entries.get(self.scm.selected_idx) {
            let path = entry.path.clone();
            let staged = entry.staged;

            let args = if staged {
                vec!["diff", "--cached", "--", &path]
            } else {
                vec!["diff", "--", &path]
            };

            if let Ok(output) = std::process::Command::new("git")
                .args(&args)
                .current_dir(&self.cwd)
                .output()
            {
                let diff_str = String::from_utf8_lossy(&output.stdout);
                for line in diff_str.lines() {
                    let kind = if line.starts_with('+') && !line.starts_with("+++") {
                        DiffLineKind::Added
                    } else if line.starts_with('-') && !line.starts_with("---") {
                        DiffLineKind::Removed
                    } else if line.starts_with("@@")
                        || line.starts_with("diff ")
                        || line.starts_with("index ")
                        || line.starts_with("---")
                        || line.starts_with("+++")
                    {
                        DiffLineKind::Header
                    } else {
                        DiffLineKind::Context
                    };
                    self.scm.diff_lines.push(DiffLine {
                        kind,
                        text: line.to_string(),
                    });
                }
            }

            // For untracked files, show the whole file content as "added"
            if self.scm.diff_lines.is_empty() {
                if let Some(entry) = self.scm.entries.get(self.scm.selected_idx) {
                    if entry.status == ScmStatus::Untracked {
                        let full_path = format!("{}/{}", self.cwd, entry.path);
                        if let Ok(content) = std::fs::read_to_string(&full_path) {
                            self.scm.diff_lines.push(DiffLine {
                                kind: DiffLineKind::Header,
                                text: format!("New file: {}", entry.path),
                            });
                            for line in content.lines() {
                                self.scm.diff_lines.push(DiffLine {
                                    kind: DiffLineKind::Added,
                                    text: format!("+{}", line),
                                });
                            }
                        }
                    }
                }
            }
        }
    }

    pub fn handle_scm_explorer_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                if self.scm.selected_idx > 0 {
                    self.scm.selected_idx -= 1;
                    self.load_diff_for_selected();
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if self.scm.selected_idx + 1 < self.scm.entries.len() {
                    self.scm.selected_idx += 1;
                    self.load_diff_for_selected();
                }
            }
            KeyCode::Enter => {
                // Open the file in the editor
                if let Some(entry) = self.scm.entries.get(self.scm.selected_idx) {
                    let full_path = format!("{}/{}", self.cwd, entry.path);
                    self.open_file(&full_path);
                    self.mode = EditorMode::Files;
                }
            }
            // 's' to stage, 'u' to unstage
            KeyCode::Char('s') => {
                if let Some(entry) = self.scm.entries.get(self.scm.selected_idx) {
                    let path = entry.path.clone();
                    let _ = std::process::Command::new("git")
                        .args(["add", "--", &path])
                        .current_dir(&self.cwd)
                        .output();
                    self.refresh_scm();
                }
            }
            KeyCode::Char('u') => {
                if let Some(entry) = self.scm.entries.get(self.scm.selected_idx) {
                    let path = entry.path.clone();
                    let _ = std::process::Command::new("git")
                        .args(["reset", "HEAD", "--", &path])
                        .current_dir(&self.cwd)
                        .output();
                    self.refresh_scm();
                }
            }
            // 'r' to refresh
            KeyCode::Char('r') => {
                self.refresh_scm();
            }
            _ => {}
        }
    }

    pub fn handle_scm_diff_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                self.scm.diff_scroll = self.scm.diff_scroll.saturating_sub(1);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if self.scm.diff_scroll + 1 < self.scm.diff_lines.len() {
                    self.scm.diff_scroll += 1;
                }
            }
            KeyCode::PageUp => {
                self.scm.diff_scroll = self.scm.diff_scroll.saturating_sub(20);
            }
            KeyCode::PageDown => {
                self.scm.diff_scroll = (self.scm.diff_scroll + 20)
                    .min(self.scm.diff_lines.len().saturating_sub(1));
            }
            _ => {}
        }
    }

    fn save_file(&mut self) -> Option<String> {
        if let Some(ref path) = self.file_path {
            let content = self.buffer.to_string();
            if std::fs::write(path, &content).is_ok() {
                self.buffer.modified = false;
                self.open_buffers.remove(path);
                return Some(path.clone());
            }
        }
        None
    }
}
