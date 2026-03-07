use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::editor::TextBuffer;
use super::prompt::PromptPanel;

pub struct CodePanel {
    // File explorer state
    pub cwd: String,
    pub entries: Vec<FileEntry>,
    pub selected_idx: usize,
    pub tree_scroll: usize,

    // Editor state
    pub file_path: Option<String>,
    pub buffer: TextBuffer,

    // Viewport size (set during render)
    pub viewport_height: usize,
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
            cwd: cwd.clone(),
            entries: Vec::new(),
            selected_idx: 0,
            tree_scroll: 0,
            file_path: None,
            buffer: TextBuffer::new(),
            viewport_height: 24,
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
            self.buffer = TextBuffer::from_string(&content);
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
                self.save_file();
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

    fn save_file(&mut self) {
        if let Some(ref path) = self.file_path {
            let content = self.buffer.to_string();
            if std::fs::write(path, &content).is_ok() {
                self.buffer.modified = false;
            }
        }
    }
}
