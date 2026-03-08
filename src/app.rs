use anyhow::Result;
use crossterm::event::KeyEvent;
use ratatui::layout::Rect;
use crate::config::Config;
use crate::llm::LlmManager;
use crate::panels::{CodePanel, LlmPanel, PanelId, PromptPanel, TerminalPanel};
use crate::storage::Store;

/// Top-level application state
pub struct App {
    #[allow(dead_code)]
    pub config: Config,
    pub store: Store,
    pub llm: LlmManager,

    // Panels
    pub code_panel: CodePanel,
    pub llm_panel: LlmPanel,
    pub prompt_panel: PromptPanel,
    pub terminal_panel: TerminalPanel,

    /// Split editor: secondary file path + buffer (side-by-side)
    pub split_editor: Option<SplitEditor>,

    // Layout
    pub focused: PanelId,
    pub visible: [bool; 4], // [Explorer, Editor, Llm, Prompt]

    #[allow(dead_code)]
    pub should_quit: bool,
    pub quit_confirm: bool,
    pub quit_unsaved_files: Vec<String>,

    /// Panel rectangles from last render (for mouse hit-testing)
    pub panel_rects: Vec<(PanelId, Rect)>,
    /// Whether mouse is currently dragging (for selection)
    pub mouse_dragging: bool,

    /// Double-click tracking
    pub last_click: Option<(std::time::Instant, PanelId, u16, u16)>,

    /// Overlay mode (file finder, search, etc.)
    pub overlay: Option<Overlay>,

    /// Go to line overlay
    pub goto_line_input: Option<String>,
}

/// Side-by-side split editor state
pub struct SplitEditor {
    pub file_path: String,
    pub buffer: crate::panels::editor::TextBuffer,
    pub focused: bool, // true = right (split) is focused
}

#[derive(Debug)]
pub enum Overlay {
    FileFinder {
        query: String,
        results: Vec<String>,
        selected: usize,
    },
    FindInFile {
        query: String,
        matches: Vec<(usize, usize)>, // (row, col)
        current: usize,
    },
    FindInWorkspace {
        query: String,
        results: Vec<GrepResult>,
        selected: usize,
    },
}

#[derive(Debug, Clone)]
pub struct GrepResult {
    pub path: String,
    pub line_num: usize,
    pub line_text: String,
}

impl App {
    pub fn new() -> Result<Self> {
        let config = Config::load_or_default()?;
        let store = Store::new(&config)?;
        let llm = LlmManager::new(&config);

        let mut prompt_panel = PromptPanel::new();
        // Load projects from storage on startup
        prompt_panel.projects = store.list_projects().unwrap_or_default();

        Ok(Self {
            config,
            store,
            llm,
            code_panel: CodePanel::new(),
            llm_panel: LlmPanel::new(),
            prompt_panel,
            terminal_panel: TerminalPanel::new(),
            split_editor: None,
            focused: PanelId::Editor,
            visible: [true, true, true, true],
            should_quit: false,
            quit_confirm: false,
            quit_unsaved_files: Vec::new(),
            panel_rects: Vec::new(),
            mouse_dragging: false,
            last_click: None,
            overlay: None,
            goto_line_input: None,
        })
    }

    const ALL_PANELS: [PanelId; 4] = [
        PanelId::Explorer,
        PanelId::Editor,
        PanelId::Llm,
        PanelId::Prompt,
    ];

    pub fn toggle_panel(&mut self, panel: PanelId) {
        let idx = panel as usize;
        self.visible[idx] = !self.visible[idx];

        if !self.visible[self.focused as usize] {
            self.cycle_focus();
        }
    }

    pub fn cycle_focus(&mut self) {
        let start = self.focused as usize;
        for i in 1..=4 {
            let next = (start + i) % 4;
            if self.visible[next] {
                self.focused = Self::ALL_PANELS[next];
                return;
            }
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent) {
        use crate::panels::code::EditorMode;
        match self.focused {
            PanelId::Explorer => {
                match self.code_panel.mode {
                    EditorMode::Files => self.code_panel.handle_explorer_key_pub(key, &mut self.prompt_panel),
                    EditorMode::SourceControl => self.code_panel.handle_scm_explorer_key(key),
                }
            }
            PanelId::Editor => {
                match self.code_panel.mode {
                    EditorMode::Files => self.code_panel.handle_editor_key_pub(key, &mut self.prompt_panel),
                    EditorMode::SourceControl => self.code_panel.handle_scm_diff_key(key),
                }
            }
            PanelId::Llm => self.llm_panel.handle_key(key),
            PanelId::Prompt => self.prompt_panel.handle_key(key, &mut self.llm, &mut self.store),
            PanelId::Terminal => {},
        }
    }

    pub fn poll_llm_updates(&mut self) {
        let was_streaming = self.llm_panel.streaming;
        self.llm.poll_updates(&mut self.llm_panel);

        // When streaming just finished, save the assistant response to storage
        if was_streaming && !self.llm_panel.streaming && !self.llm_panel.pending_response.is_empty() {
            if let (Some(proj), Some(thread)) = (&self.prompt_panel.current_project, &self.prompt_panel.current_thread) {
                let _ = self.store.append_message(proj, thread, "assistant", &self.llm_panel.pending_response);
            }
            self.llm_panel.pending_response.clear();
        }
    }

    pub fn unsaved_files(&self) -> Vec<String> {
        let mut files = Vec::new();
        // Check current buffer
        if self.code_panel.buffer.modified {
            if let Some(ref path) = self.code_panel.file_path {
                files.push(path.clone());
            }
        }
        // Check stashed buffers
        for (path, buf) in &self.code_panel.open_buffers {
            if buf.modified {
                files.push(path.clone());
            }
        }
        files
    }

    // ─── Overlay: File Finder (Ctrl+P) ───

    pub fn open_file_finder(&mut self) {
        self.overlay = Some(Overlay::FileFinder {
            query: String::new(),
            results: Vec::new(),
            selected: 0,
        });
    }

    fn file_finder_search(cwd: &str, query: &str) -> Vec<String> {
        use ignore::WalkBuilder;

        if query.is_empty() {
            return Vec::new();
        }
        let query_lower = query.to_lowercase();
        let mut results = Vec::new();

        let walker = WalkBuilder::new(cwd)
            .hidden(true) // respect .gitignore + skip hidden
            .max_depth(Some(10))
            .build();

        for entry in walker.flatten() {
            if entry.file_type().map_or(true, |ft| ft.is_dir()) {
                continue;
            }
            let path = entry.path().to_string_lossy();
            let relative = path
                .strip_prefix(cwd)
                .unwrap_or(&path)
                .trim_start_matches('/');
            if relative.to_lowercase().contains(&query_lower) {
                results.push(relative.to_string());
                if results.len() >= 50 {
                    break;
                }
            }
        }
        results
    }

    // ─── Overlay: Find in File (Ctrl+F) ───

    pub fn open_find_in_file(&mut self) {
        self.overlay = Some(Overlay::FindInFile {
            query: String::new(),
            matches: Vec::new(),
            current: 0,
        });
    }

    fn find_in_file_search(lines: &[String], query: &str) -> Vec<(usize, usize)> {
        if query.is_empty() {
            return Vec::new();
        }
        let query_lower = query.to_lowercase();
        let mut matches = Vec::new();
        for (row, line) in lines.iter().enumerate() {
            let line_lower = line.to_lowercase();
            let mut start = 0;
            while let Some(pos) = line_lower[start..].find(&query_lower) {
                matches.push((row, start + pos));
                start += pos + query_lower.len();
            }
        }
        matches
    }

    // ─── Overlay: Find in Workspace (Ctrl+Shift+F) ───

    pub fn open_find_in_workspace(&mut self) {
        self.overlay = Some(Overlay::FindInWorkspace {
            query: String::new(),
            results: Vec::new(),
            selected: 0,
        });
    }

    fn find_in_workspace_search(cwd: &str, query: &str) -> Vec<GrepResult> {
        use ignore::WalkBuilder;

        if query.is_empty() {
            return Vec::new();
        }
        let query_lower = query.to_lowercase();
        let mut results = Vec::new();

        let walker = WalkBuilder::new(cwd)
            .hidden(true)
            .max_depth(Some(10))
            .build();

        for entry in walker.flatten() {
            if entry.file_type().map_or(true, |ft| ft.is_dir()) {
                continue;
            }
            let path = entry.path();
            if let Ok(content) = std::fs::read_to_string(path) {
                let relative = path
                    .to_string_lossy()
                    .strip_prefix(cwd)
                    .unwrap_or(&path.to_string_lossy())
                    .trim_start_matches('/')
                    .to_string();
                for (i, line) in content.lines().enumerate() {
                    if line.to_lowercase().contains(&query_lower) {
                        results.push(GrepResult {
                            path: relative.clone(),
                            line_num: i + 1,
                            line_text: line.to_string(),
                        });
                        if results.len() >= 100 {
                            return results;
                        }
                    }
                }
            }
        }
        results
    }

    // ─── Overlay Key Handling ───

    pub fn handle_overlay_key(&mut self, key: KeyEvent) {
        use crossterm::event::KeyCode;

        // Common: Esc closes any overlay
        if key.code == KeyCode::Esc {
            self.overlay = None;
            return;
        }

        match self.overlay {
            Some(Overlay::FileFinder {
                ref mut query,
                ref mut results,
                ref mut selected,
            }) => {
                match key.code {
                    KeyCode::Enter => {
                        if let Some(path) = results.get(*selected).cloned() {
                            let full_path = format!("{}/{}", self.code_panel.cwd, path);
                            self.code_panel.open_file(&full_path);
                            self.focused = PanelId::Editor;
                        }
                        self.overlay = None;
                    }
                    KeyCode::Up => *selected = selected.saturating_sub(1),
                    KeyCode::Down => {
                        if *selected + 1 < results.len() {
                            *selected += 1;
                        }
                    }
                    KeyCode::Backspace => {
                        query.pop();
                        *results = Self::file_finder_search(&self.code_panel.cwd, query);
                        *selected = 0;
                    }
                    KeyCode::Char(c) => {
                        query.push(c);
                        *results = Self::file_finder_search(&self.code_panel.cwd, query);
                        *selected = 0;
                    }
                    _ => {}
                }
            }

            Some(Overlay::FindInFile {
                ref mut query,
                ref mut matches,
                ref mut current,
            }) => {
                match key.code {
                    KeyCode::Enter => {
                        if let Some(&(row, col)) = matches.get(*current) {
                            self.code_panel.buffer.cursor_row = row;
                            self.code_panel.buffer.cursor_col = col;
                            self.code_panel.buffer.select_anchor = None;
                            self.focused = PanelId::Editor;
                        }
                        if !matches.is_empty() {
                            *current = (*current + 1) % matches.len();
                        }
                    }
                    KeyCode::Up => {
                        if !matches.is_empty() {
                            *current = if *current == 0 { matches.len() - 1 } else { *current - 1 };
                            if let Some(&(row, col)) = matches.get(*current) {
                                self.code_panel.buffer.cursor_row = row;
                                self.code_panel.buffer.cursor_col = col;
                            }
                        }
                    }
                    KeyCode::Down => {
                        if !matches.is_empty() {
                            *current = (*current + 1) % matches.len();
                            if let Some(&(row, col)) = matches.get(*current) {
                                self.code_panel.buffer.cursor_row = row;
                                self.code_panel.buffer.cursor_col = col;
                            }
                        }
                    }
                    KeyCode::Backspace => {
                        query.pop();
                        *matches = Self::find_in_file_search(&self.code_panel.buffer.lines, query);
                        *current = 0;
                        if let Some(&(row, col)) = matches.first() {
                            self.code_panel.buffer.cursor_row = row;
                            self.code_panel.buffer.cursor_col = col;
                        }
                    }
                    KeyCode::Char(c) => {
                        query.push(c);
                        *matches = Self::find_in_file_search(&self.code_panel.buffer.lines, query);
                        *current = 0;
                        if let Some(&(row, col)) = matches.first() {
                            self.code_panel.buffer.cursor_row = row;
                            self.code_panel.buffer.cursor_col = col;
                        }
                    }
                    _ => {}
                }
            }

            Some(Overlay::FindInWorkspace {
                ref mut query,
                ref mut results,
                ref mut selected,
            }) => {
                match key.code {
                    KeyCode::Enter => {
                        if let Some(result) = results.get(*selected).cloned() {
                            let full_path = format!("{}/{}", self.code_panel.cwd, result.path);
                            self.code_panel.open_file(&full_path);
                            self.code_panel.buffer.cursor_row = result.line_num.saturating_sub(1);
                            self.code_panel.buffer.cursor_col = 0;
                            self.focused = PanelId::Editor;
                        }
                        self.overlay = None;
                    }
                    KeyCode::Up => *selected = selected.saturating_sub(1),
                    KeyCode::Down => {
                        if *selected + 1 < results.len() {
                            *selected += 1;
                        }
                    }
                    KeyCode::Backspace => {
                        query.pop();
                        *results = Self::find_in_workspace_search(&self.code_panel.cwd, query);
                        *selected = 0;
                    }
                    KeyCode::Char(c) => {
                        query.push(c);
                        *results = Self::find_in_workspace_search(&self.code_panel.cwd, query);
                        *selected = 0;
                    }
                    _ => {}
                }
            }

            None => {}
        }
    }

    pub fn handle_mouse(&mut self, mouse: crossterm::event::MouseEvent) {
        use crossterm::event::{MouseButton, MouseEventKind};
        let x = mouse.column;
        let y = mouse.row;

        let panel_at = self
            .panel_rects
            .iter()
            .find(|(_, rect)| {
                x >= rect.x && x < rect.x + rect.width && y >= rect.y && y < rect.y + rect.height
            })
            .map(|(id, rect)| (*id, *rect));

        // Helper: position cursor in a TextBuffer given click coords
        fn click_to_cursor(
            buf: &mut crate::panels::editor::TextBuffer,
            local_x: u16,
            local_y: u16,
            gutter: u16,
            clear_selection: bool,
        ) {
            let col = local_x.saturating_sub(gutter) as usize + buf.scroll_col;
            let row = (local_y as usize + buf.scroll_row).min(buf.lines.len().saturating_sub(1));
            let col = col.min(buf.lines[row].len());
            if clear_selection {
                buf.select_anchor = None;
            } else if buf.select_anchor.is_none() {
                buf.select_anchor = Some((buf.cursor_row, buf.cursor_col));
            }
            buf.cursor_row = row;
            buf.cursor_col = col;
        }

        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                if let Some((panel_id, rect)) = panel_at {
                    self.focused = panel_id;
                    self.mouse_dragging = true;
                    let lx = x.saturating_sub(rect.x + 1);
                    let ly = y.saturating_sub(rect.y + 1);

                    // Detect double-click (within 400ms, same panel, same position)
                    let now = std::time::Instant::now();
                    let is_double = self.last_click.map_or(false, |(t, pid, px, py)| {
                        pid == panel_id && px == x && py == y && now.duration_since(t).as_millis() < 400
                    });
                    self.last_click = Some((now, panel_id, x, y));

                    match panel_id {
                        PanelId::Editor => {
                            click_to_cursor(&mut self.code_panel.buffer, lx, ly, 5, true);
                        }
                        PanelId::Explorer => {
                            let idx = ly as usize + self.code_panel.tree_scroll;
                            if idx < self.code_panel.entries.len() {
                                self.code_panel.selected_idx = idx;
                                if is_double {
                                    // Double-click: open file or enter directory
                                    if let Some(entry) = self.code_panel.entries.get(idx).cloned() {
                                        if entry.is_dir {
                                            self.code_panel.cwd = entry.path;
                                            self.code_panel.selected_idx = 0;
                                            self.code_panel.refresh_entries();
                                        } else {
                                            self.code_panel.open_file(&entry.path);
                                            self.focused = PanelId::Editor;
                                        }
                                    }
                                }
                            }
                        }
                        PanelId::Prompt => {
                            use crate::panels::prompt::PromptView;
                            if self.prompt_panel.view == PromptView::Compose {
                                click_to_cursor(&mut self.prompt_panel.compose, lx, ly, 0, true);
                            }
                        }
                        PanelId::Llm => {}
                        PanelId::Terminal => {}
                    }
                }
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                if !self.mouse_dragging {
                    return;
                }
                if let Some((panel_id, rect)) = panel_at {
                    let lx = x.saturating_sub(rect.x + 1);
                    let ly = y.saturating_sub(rect.y + 1);
                    match panel_id {
                        PanelId::Editor => {
                            click_to_cursor(&mut self.code_panel.buffer, lx, ly, 5, false);
                        }
                        PanelId::Prompt => {
                            use crate::panels::prompt::PromptView;
                            if self.prompt_panel.view == PromptView::Compose {
                                click_to_cursor(&mut self.prompt_panel.compose, lx, ly, 0, false);
                            }
                        }
                        _ => {}
                    }
                }
            }
            MouseEventKind::Up(MouseButton::Left) => {
                self.mouse_dragging = false;
            }
            MouseEventKind::ScrollUp => {
                if let Some((panel_id, _)) = panel_at {
                    match panel_id {
                        PanelId::Editor => {
                            self.code_panel.buffer.scroll_row =
                                self.code_panel.buffer.scroll_row.saturating_sub(3);
                        }
                        PanelId::Explorer => {
                            self.code_panel.selected_idx =
                                self.code_panel.selected_idx.saturating_sub(3);
                        }
                        PanelId::Llm => {
                            self.llm_panel.scroll_offset += 3;
                            self.llm_panel.following = false;
                        }
                        PanelId::Prompt => {
                            self.prompt_panel.compose.scroll_row =
                                self.prompt_panel.compose.scroll_row.saturating_sub(3);
                        }
                        _ => {}
                    }
                }
            }
            MouseEventKind::ScrollDown => {
                if let Some((panel_id, _)) = panel_at {
                    match panel_id {
                        PanelId::Editor => {
                            let max = self.code_panel.buffer.lines.len().saturating_sub(1);
                            self.code_panel.buffer.scroll_row =
                                (self.code_panel.buffer.scroll_row + 3).min(max);
                        }
                        PanelId::Explorer => {
                            let max = self.code_panel.entries.len().saturating_sub(1);
                            self.code_panel.selected_idx =
                                (self.code_panel.selected_idx + 3).min(max);
                        }
                        PanelId::Llm => {
                            self.llm_panel.scroll_offset =
                                self.llm_panel.scroll_offset.saturating_sub(3);
                            if self.llm_panel.scroll_offset == 0 {
                                self.llm_panel.following = true;
                            }
                        }
                        PanelId::Prompt => {
                            let max = self.prompt_panel.compose.lines.len().saturating_sub(1);
                            self.prompt_panel.compose.scroll_row =
                                (self.prompt_panel.compose.scroll_row + 3).min(max);
                        }
                        _ => {}
                    }
                }
            }
            MouseEventKind::Down(MouseButton::Middle) => {
                if let Some((panel_id, _)) = panel_at {
                    self.focused = panel_id;
                    match panel_id {
                        PanelId::Editor => self.code_panel.buffer.paste(),
                        PanelId::Prompt => self.prompt_panel.compose.paste(),
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
}
