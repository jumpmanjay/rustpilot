use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::prompt::PromptPanel;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodeView {
    Explorer,
    Editor,
    // Future: Diff, Search
}

pub struct CodePanel {
    pub view: CodeView,

    // File explorer state
    pub cwd: String,
    pub entries: Vec<FileEntry>,
    pub selected_idx: usize,
    pub tree_scroll: usize,

    // Editor state
    pub file_path: Option<String>,
    pub lines: Vec<String>,
    pub cursor_row: usize,
    pub cursor_col: usize,
    pub scroll_offset: usize,
    pub modified: bool,

    // Line selection for references
    pub select_anchor: Option<usize>,
}

#[derive(Debug, Clone)]
pub struct FileEntry {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub depth: usize,
    pub expanded: bool,
}

impl CodePanel {
    pub fn new() -> Self {
        let cwd = std::env::current_dir()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| ".".to_string());

        let mut panel = Self {
            view: CodeView::Explorer,
            cwd: cwd.clone(),
            entries: Vec::new(),
            selected_idx: 0,
            tree_scroll: 0,
            file_path: None,
            lines: Vec::new(),
            cursor_row: 0,
            cursor_col: 0,
            scroll_offset: 0,
            modified: false,
            select_anchor: None,
        };
        panel.refresh_entries();
        panel
    }

    pub fn refresh_entries(&mut self) {
        self.entries.clear();
        if let Ok(rd) = std::fs::read_dir(&self.cwd) {
            let mut items: Vec<_> = rd
                .filter_map(|e| e.ok())
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
            // Dirs first, then alphabetical
            items.sort_by(|a, b| {
                b.is_dir
                    .cmp(&a.is_dir)
                    .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            });
            self.entries = items;
        }
    }

    pub fn open_file(&mut self, path: &str) {
        if let Ok(content) = std::fs::read_to_string(path) {
            self.file_path = Some(path.to_string());
            self.lines = content.lines().map(|l| l.to_string()).collect();
            if self.lines.is_empty() {
                self.lines.push(String::new());
            }
            self.cursor_row = 0;
            self.cursor_col = 0;
            self.scroll_offset = 0;
            self.modified = false;
            self.select_anchor = None;
            self.view = CodeView::Editor;
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent, prompt: &mut PromptPanel) {
        match self.view {
            CodeView::Explorer => self.handle_explorer_key(key, prompt),
            CodeView::Editor => self.handle_editor_key(key, prompt),
        }
    }

    fn handle_explorer_key(&mut self, key: KeyEvent, prompt: &mut PromptPanel) {
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
                // Go up one directory
                if let Some(parent) = std::path::Path::new(&self.cwd).parent() {
                    self.cwd = parent.to_string_lossy().to_string();
                    self.selected_idx = 0;
                    self.refresh_entries();
                }
            }
            // Ctrl+R: send selected path as tag reference to prompt
            KeyCode::Char('r') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if let Some(entry) = self.entries.get(self.selected_idx) {
                    prompt.insert_reference(&format!("@{}", entry.path), false);
                }
            }
            // Ctrl+Shift+R: send as include reference
            KeyCode::Char('R') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if let Some(entry) = self.entries.get(self.selected_idx) {
                    prompt.insert_reference(&format!("@@{}", entry.path), true);
                }
            }
            _ => {}
        }
    }

    fn handle_editor_key(&mut self, key: KeyEvent, prompt: &mut PromptPanel) {
        match key.code {
            // Navigation
            KeyCode::Up => {
                if self.cursor_row > 0 {
                    self.cursor_row -= 1;
                    self.clamp_cursor_col();
                }
            }
            KeyCode::Down => {
                if self.cursor_row + 1 < self.lines.len() {
                    self.cursor_row += 1;
                    self.clamp_cursor_col();
                }
            }
            KeyCode::Left => {
                if self.cursor_col > 0 {
                    self.cursor_col -= 1;
                }
            }
            KeyCode::Right => {
                let line_len = self.lines[self.cursor_row].len();
                if self.cursor_col < line_len {
                    self.cursor_col += 1;
                }
            }
            KeyCode::Home => self.cursor_col = 0,
            KeyCode::End => self.cursor_col = self.lines[self.cursor_row].len(),

            // Save: Ctrl+S
            KeyCode::Char('s') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.save_file();
            }

            // Back to explorer: Ctrl+E
            KeyCode::Char('e') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.view = CodeView::Explorer;
            }

            // Send tag reference: Ctrl+R
            KeyCode::Char('r') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if let Some(ref path) = self.file_path {
                    let line_ref = if let Some(anchor) = self.select_anchor {
                        let (start, end) = if anchor <= self.cursor_row {
                            (anchor + 1, self.cursor_row + 1)
                        } else {
                            (self.cursor_row + 1, anchor + 1)
                        };
                        if start == end {
                            format!("@{}:{}", path, start)
                        } else {
                            format!("@{}:{}-{}", path, start, end)
                        }
                    } else {
                        format!("@{}:{}", path, self.cursor_row + 1)
                    };
                    prompt.insert_reference(&line_ref, false);
                }
            }

            // Send include reference: Ctrl+Shift+R
            KeyCode::Char('R') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if let Some(ref path) = self.file_path {
                    let line_ref = if let Some(anchor) = self.select_anchor {
                        let (start, end) = if anchor <= self.cursor_row {
                            (anchor + 1, self.cursor_row + 1)
                        } else {
                            (self.cursor_row + 1, anchor + 1)
                        };
                        if start == end {
                            format!("@@{}:{}", path, start)
                        } else {
                            format!("@@{}:{}-{}", path, start, end)
                        }
                    } else {
                        format!("@@{}:{}", path, self.cursor_row + 1)
                    };
                    prompt.insert_reference(&line_ref, true);
                }
            }

            // Toggle line selection anchor: Ctrl+L
            KeyCode::Char('l') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if self.select_anchor.is_some() {
                    self.select_anchor = None;
                } else {
                    self.select_anchor = Some(self.cursor_row);
                }
            }

            // Basic text editing
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                let row = self.cursor_row;
                self.lines[row].insert(self.cursor_col, c);
                self.cursor_col += 1;
                self.modified = true;
            }
            KeyCode::Backspace if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                if self.cursor_col > 0 {
                    self.lines[self.cursor_row].remove(self.cursor_col - 1);
                    self.cursor_col -= 1;
                    self.modified = true;
                } else if self.cursor_row > 0 {
                    let line = self.lines.remove(self.cursor_row);
                    self.cursor_row -= 1;
                    self.cursor_col = self.lines[self.cursor_row].len();
                    self.lines[self.cursor_row].push_str(&line);
                    self.modified = true;
                }
            }
            KeyCode::Enter if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                let rest = self.lines[self.cursor_row].split_off(self.cursor_col);
                self.cursor_row += 1;
                self.lines.insert(self.cursor_row, rest);
                self.cursor_col = 0;
                self.modified = true;
            }
            _ => {}
        }

        // Keep cursor in viewport
        self.adjust_scroll();
    }

    fn clamp_cursor_col(&mut self) {
        let line_len = self.lines[self.cursor_row].len();
        if self.cursor_col > line_len {
            self.cursor_col = line_len;
        }
    }

    fn adjust_scroll(&mut self) {
        // Will be called with actual viewport height during render
        // For now, basic logic
        if self.cursor_row < self.scroll_offset {
            self.scroll_offset = self.cursor_row;
        }
    }

    pub fn adjust_scroll_for_height(&mut self, height: usize) {
        if height == 0 {
            return;
        }
        if self.cursor_row < self.scroll_offset {
            self.scroll_offset = self.cursor_row;
        }
        if self.cursor_row >= self.scroll_offset + height {
            self.scroll_offset = self.cursor_row - height + 1;
        }
    }

    fn save_file(&mut self) {
        if let Some(ref path) = self.file_path {
            let content = self.lines.join("\n");
            if std::fs::write(path, &content).is_ok() {
                self.modified = false;
            }
        }
    }
}
