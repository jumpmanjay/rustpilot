use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::llm::LlmManager;
use crate::storage::Store;

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

    // Compose state
    pub compose_lines: Vec<String>,
    pub compose_cursor_row: usize,
    pub compose_cursor_col: usize,

    // References queued from other panels
    pub pending_references: Vec<String>,

    // History view
    pub history_scroll: usize,
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
            compose_lines: vec![String::new()],
            compose_cursor_row: 0,
            compose_cursor_col: 0,
            pending_references: Vec::new(),
            history_scroll: 0,
        }
    }

    /// Insert a reference from another panel (file path, line ref, etc.)
    pub fn insert_reference(&mut self, reference: &str, _is_include: bool) {
        // If composing, insert at cursor. Otherwise queue it.
        if self.view == PromptView::Compose {
            let row = self.compose_cursor_row;
            self.compose_lines[row].insert_str(self.compose_cursor_col, reference);
            self.compose_cursor_col += reference.len();
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
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                if self.current_project.is_some() {
                    // Navigating threads
                    if self.selected_thread > 0 {
                        self.selected_thread -= 1;
                    }
                } else {
                    // Navigating projects
                    if self.selected_project > 0 {
                        self.selected_project -= 1;
                    }
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
                    // Select project
                    if let Some(proj) = self.projects.get(self.selected_project).cloned() {
                        self.current_project = Some(proj.clone());
                        self.threads = store.list_threads(&proj).unwrap_or_default();
                        self.selected_thread = 0;
                    }
                } else {
                    // Select thread → go to compose
                    if let Some(thread) = self.threads.get(self.selected_thread).cloned() {
                        self.current_thread = Some(thread);
                        self.view = PromptView::Compose;
                        self.compose_lines = vec![String::new()];
                        self.compose_cursor_row = 0;
                        self.compose_cursor_col = 0;

                        // Insert any pending references
                        for r in self.pending_references.drain(..) {
                            self.compose_lines[0].push_str(&r);
                            self.compose_lines[0].push(' ');
                        }
                        self.compose_cursor_col = self.compose_lines[0].len();
                    }
                }
            }
            KeyCode::Backspace => {
                // Go back from threads to projects
                if self.current_project.is_some() {
                    self.current_project = None;
                    self.current_thread = None;
                    self.selected_thread = 0;
                }
            }
            // Ctrl+N: new project/thread
            KeyCode::Char('n') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if self.current_project.is_none() {
                    // TODO: prompt for project name (inline input)
                    let name = format!("project-{}", self.projects.len() + 1);
                    let _ = store.create_project(&name);
                    self.projects = store.list_projects().unwrap_or_default();
                } else if let Some(ref proj) = self.current_project {
                    let name = format!("thread-{}", self.threads.len() + 1);
                    let _ = store.create_thread(proj, &name);
                    self.threads = store.list_threads(proj).unwrap_or_default();
                }
            }
            _ => {}
        }
    }

    fn handle_compose_key(&mut self, key: KeyEvent, llm: &mut LlmManager, store: &mut Store) {
        match key.code {
            // Send prompt: Ctrl+Enter
            KeyCode::Enter if key.modifiers.contains(KeyModifiers::CONTROL) => {
                let prompt_text = self.compose_lines.join("\n");
                if !prompt_text.trim().is_empty() {
                    if let (Some(proj), Some(thread)) =
                        (&self.current_project, &self.current_thread)
                    {
                        // Resolve references and store
                        let resolved = crate::refs::resolve_references(&prompt_text);
                        let _ = store.append_message(proj, thread, "user", &prompt_text);

                        // Send to LLM
                        llm.send_prompt(&resolved);

                        // Clear compose
                        self.compose_lines = vec![String::new()];
                        self.compose_cursor_row = 0;
                        self.compose_cursor_col = 0;
                    }
                }
            }

            // Back to browser: Escape
            KeyCode::Esc => {
                self.view = PromptView::Browser;
            }

            // View history: Ctrl+H
            KeyCode::Char('h') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.view = PromptView::History;
                self.history_scroll = 0;
            }

            // Basic text editing
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.compose_lines[self.compose_cursor_row].insert(self.compose_cursor_col, c);
                self.compose_cursor_col += 1;
            }
            KeyCode::Backspace if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                if self.compose_cursor_col > 0 {
                    self.compose_lines[self.compose_cursor_row]
                        .remove(self.compose_cursor_col - 1);
                    self.compose_cursor_col -= 1;
                } else if self.compose_cursor_row > 0 {
                    let line = self.compose_lines.remove(self.compose_cursor_row);
                    self.compose_cursor_row -= 1;
                    self.compose_cursor_col = self.compose_lines[self.compose_cursor_row].len();
                    self.compose_lines[self.compose_cursor_row].push_str(&line);
                }
            }
            KeyCode::Enter => {
                let rest = self.compose_lines[self.compose_cursor_row]
                    .split_off(self.compose_cursor_col);
                self.compose_cursor_row += 1;
                self.compose_lines.insert(self.compose_cursor_row, rest);
                self.compose_cursor_col = 0;
            }
            KeyCode::Up => {
                if self.compose_cursor_row > 0 {
                    self.compose_cursor_row -= 1;
                    let len = self.compose_lines[self.compose_cursor_row].len();
                    if self.compose_cursor_col > len {
                        self.compose_cursor_col = len;
                    }
                }
            }
            KeyCode::Down => {
                if self.compose_cursor_row + 1 < self.compose_lines.len() {
                    self.compose_cursor_row += 1;
                    let len = self.compose_lines[self.compose_cursor_row].len();
                    if self.compose_cursor_col > len {
                        self.compose_cursor_col = len;
                    }
                }
            }
            _ => {}
        }
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
