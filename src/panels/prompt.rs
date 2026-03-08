use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::editor::TextBuffer;
use crate::llm::LlmManager;
use crate::storage::{Store, Message as StorageMessage};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptView {
    /// Project/thread browser
    Browser,
    /// Composing a prompt
    Compose,
    /// Viewing thread history
    History,
}

pub struct PromptPanel {
    pub view: PromptView,

    // Browser state
    pub projects: Vec<String>,
    pub selected_project: usize,
    pub threads: Vec<String>,
    pub selected_thread: usize,

    // Active project/thread
    pub current_project: Option<String>,
    pub current_thread: Option<String>,

    // Compose state — now using TextBuffer
    pub compose: TextBuffer,

    // References queued from other panels
    pub pending_references: Vec<String>,

    // Files manually saved by the user (tracked for LLM context)
    pub changed_files: Vec<String>,

    // History view
    pub history_messages: Vec<StorageMessage>,
    pub history_scroll: usize,

    // Naming overlay (for new project/thread)
    pub naming_input: Option<String>,
    pub naming_what: String, // "project" or "thread"

    // Viewport size (set during render)
    pub viewport_height: usize,
}

impl PromptPanel {
    pub fn new() -> Self {
        Self {
            view: PromptView::Browser,
            projects: Vec::new(),
            selected_project: 0,
            threads: Vec::new(),
            selected_thread: 0,
            current_project: None,
            current_thread: None,
            compose: TextBuffer::new(),
            pending_references: Vec::new(),
            changed_files: Vec::new(),
            history_messages: Vec::new(),
            history_scroll: 0,
            naming_input: None,
            naming_what: String::new(),
            viewport_height: 24,
        }
    }

    /// Record a file that was manually saved by the user
    pub fn record_saved_file(&mut self, path: &str) {
        if !self.changed_files.contains(&path.to_string()) {
            self.changed_files.push(path.to_string());
        }
    }

    /// Clear the changed files list (after sending a prompt)
    pub fn clear_changed_files(&mut self) {
        self.changed_files.clear();
    }

    /// Insert a reference from another panel (file path, line ref, etc.)
    /// Handle a mouse click at a local y position in the browser view
    pub fn handle_browser_click(&mut self, _x: u16, y: u16, store: &mut Store) {
        let idx = y as usize;
        if self.current_project.is_none() {
            // Clicking on project list
            if idx < self.projects.len() {
                self.selected_project = idx;
                let proj = self.projects[idx].clone();
                self.current_project = Some(proj.clone());
                self.threads = store.list_threads(&proj).unwrap_or_default();
                self.selected_thread = 0;
            }
        } else {
            // Clicking on thread list
            if idx < self.threads.len() {
                self.selected_thread = idx;
                let thread = self.threads[idx].clone();
                self.current_thread = Some(thread);
                self.view = PromptView::Compose;
                self.compose.clear();
                for r in self.pending_references.drain(..) {
                    self.compose.insert_str(&r);
                    self.compose.insert_newline();
                }
            }
        }
    }

    pub fn insert_reference(&mut self, reference: &str, _is_include: bool) {
        if self.view == PromptView::Compose {
            self.compose.insert_str(reference);
            self.compose.insert_newline(); // newline after reference
        } else {
            self.pending_references.push(reference.to_string());
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent, llm: &mut LlmManager, store: &mut Store) {
        match self.view {
            PromptView::Browser => self.handle_browser_key(key, store),
            PromptView::Compose => self.handle_compose_key(key, llm, store),
            PromptView::History => self.handle_history_key(key),
        }
    }

    fn handle_browser_key(&mut self, key: KeyEvent, store: &mut Store) {
        // Naming overlay
        if self.naming_input.is_some() {
            match key.code {
                KeyCode::Esc => { self.naming_input = None; }
                KeyCode::Enter => {
                    if let Some(ref name) = self.naming_input.clone() {
                        if !name.is_empty() {
                            if self.naming_what == "project" {
                                let _ = store.create_project(name);
                                self.projects = store.list_projects().unwrap_or_default();
                            } else if let Some(ref proj) = self.current_project {
                                let _ = store.create_thread(proj, name);
                                self.threads = store.list_threads(proj).unwrap_or_default();
                            }
                        }
                    }
                    self.naming_input = None;
                }
                KeyCode::Char(c) => {
                    if let Some(ref mut input) = self.naming_input {
                        input.push(c);
                    }
                }
                KeyCode::Backspace => {
                    if let Some(ref mut input) = self.naming_input {
                        input.pop();
                    }
                }
                _ => {}
            }
            return;
        }

        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                if self.current_project.is_some() {
                    if self.selected_thread > 0 {
                        self.selected_thread -= 1;
                    }
                } else if self.selected_project > 0 {
                    self.selected_project -= 1;
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if self.current_project.is_some() {
                    if self.selected_thread + 1 < self.threads.len() {
                        self.selected_thread += 1;
                    }
                } else if self.selected_project + 1 < self.projects.len() {
                    self.selected_project += 1;
                }
            }
            KeyCode::Enter => {
                if self.current_project.is_none() {
                    if let Some(proj) = self.projects.get(self.selected_project).cloned() {
                        self.current_project = Some(proj.clone());
                        self.threads = store.list_threads(&proj).unwrap_or_default();
                        self.selected_thread = 0;
                    }
                } else {
                    if let Some(thread) = self.threads.get(self.selected_thread).cloned() {
                        self.current_thread = Some(thread);
                        self.view = PromptView::Compose;
                        self.compose.clear();

                        // Insert any pending references
                        for r in self.pending_references.drain(..) {
                            self.compose.insert_str(&r);
                            self.compose.insert_char(' ');
                        }
                    }
                }
            }
            KeyCode::Backspace => {
                if self.current_project.is_some() {
                    self.current_project = None;
                    self.current_thread = None;
                    self.selected_thread = 0;
                }
            }
            KeyCode::Char('n') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                // Create new project or thread with naming
                self.naming_input = Some(String::new());
                self.naming_what = if self.current_project.is_none() {
                    "project".to_string()
                } else {
                    "thread".to_string()
                };
            }
            _ => {}
        }
    }

    fn handle_compose_key(&mut self, key: KeyEvent, llm: &mut LlmManager, store: &mut Store) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);

        match key.code {
            // Send prompt: Ctrl+Enter
            KeyCode::Enter if ctrl => {
                let prompt_text = self.compose.to_string();
                if !prompt_text.trim().is_empty() {
                    if let (Some(proj), Some(thread)) =
                        (&self.current_project, &self.current_thread)
                    {
                        // Build the full prompt with changed files context
                        let mut full_prompt = String::new();
                        if !self.changed_files.is_empty() {
                            full_prompt.push_str("Files I manually edited since last prompt:\n");
                            for f in &self.changed_files {
                                full_prompt.push_str(&format!("  - {}\n", f));
                            }
                            full_prompt.push('\n');
                        }
                        full_prompt.push_str(&prompt_text);

                        let resolved = crate::refs::resolve_references(&full_prompt);
                        let _ = store.append_message(proj, thread, "user", &full_prompt);

                        // Load conversation history for context
                        let history = store.read_thread(proj, thread).unwrap_or_default();
                        // Convert to (role, content) pairs, excluding the message we just appended
                        let context: Vec<(String, String)> = history.iter()
                            .rev().skip(1).rev() // all but last (the one we just added)
                            .map(|m| (m.role.clone(), m.content.clone()))
                            .collect();

                        let _ = llm.send_prompt_with_history(&resolved, &context);
                        self.compose.clear();
                        self.clear_changed_files();
                    }
                }
                return;
            }
            // Back to browser: Escape
            KeyCode::Esc => {
                self.view = PromptView::Browser;
                return;
            }
            // View history: Ctrl+H
            KeyCode::Char('h') if ctrl => {
                if let (Some(proj), Some(thread)) = (&self.current_project, &self.current_thread) {
                    self.history_messages = store.read_thread(proj, thread).unwrap_or_default();
                }
                self.view = PromptView::History;
                self.history_scroll = 0;
                return;
            }
            _ => {}
        }

        // Delegate to TextBuffer for all editing
        self.compose.handle_key(key, self.viewport_height);
    }

    fn handle_history_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                self.history_scroll += 1;
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if self.history_scroll > 0 {
                    self.history_scroll -= 1;
                }
            }
            KeyCode::Esc => {
                self.view = PromptView::Compose;
            }
            _ => {}
        }
    }
}
