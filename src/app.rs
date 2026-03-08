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

    /// Save As overlay
    pub save_as_input: Option<String>,

    /// Menu bar state
    pub menu: Option<MenuState>,

    /// Auto-save interval tracking
    pub last_autosave: std::time::Instant,

    /// Context menu (right-click)
    pub context_menu: Option<ContextMenu>,

    /// Panel size ratios (explorer_width, right_pane_percent)
    pub explorer_width: u16,
    pub right_pane_percent: u16,
}

#[derive(Debug, Clone)]
pub struct MenuState {
    pub active_menu: usize,   // 0=File, 1=Edit, 2=View
    pub selected_item: usize,
    #[allow(dead_code)]
    pub open: bool,
}

/// Context menu (right-click)
#[derive(Debug, Clone)]
pub struct ContextMenu {
    pub x: u16,
    pub y: u16,
    pub items: Vec<ContextMenuItem>,
    pub selected: usize,
    pub source: ContextSource,
}

#[derive(Debug, Clone)]
pub struct ContextMenuItem {
    pub label: String,
    pub action: String,
}

#[derive(Debug, Clone)]
pub enum ContextSource {
    Explorer { path: String, is_dir: bool },
    PromptProject { name: String },
    PromptThread { project: String, name: String },
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
        replace: String,
        matches: Vec<(usize, usize)>, // (row, col)
        current: usize,
        case_sensitive: bool,
        use_regex: bool,
        editing_replace: bool, // true = typing in replace field
    },
    FindInWorkspace {
        query: String,
        results: Vec<GrepResult>,
        selected: usize,
        case_sensitive: bool,
        use_regex: bool,
        file_pattern: String, // e.g. "*.rs"
        editing_pattern: bool, // true = typing in file pattern field
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

        let mut llm_panel = LlmPanel::new();
        llm_panel.usage.set_model_pricing(&config.model);
        llm_panel.usage.budget_limit = config.llm.budget;

        Ok(Self {
            config,
            store,
            llm,
            code_panel: CodePanel::new(),
            llm_panel,
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
            save_as_input: None,
            menu: None,
            context_menu: None,
            last_autosave: std::time::Instant::now(),
            explorer_width: 30,
            right_pane_percent: 50,
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

    /// Auto-save modified buffers (called periodically from main loop)
    pub fn auto_save(&mut self) {
        if self.last_autosave.elapsed().as_secs() < 30 {
            return;
        }
        self.last_autosave = std::time::Instant::now();

        // Save current buffer if it has a path and is modified
        if let Some(ref path) = self.code_panel.file_path {
            if self.code_panel.buffer.modified {
                let content = self.code_panel.buffer.to_string();
                if std::fs::write(path, &content).is_ok() {
                    self.code_panel.buffer.modified = false;
                }
            }
        }

        // Save stashed buffers
        let paths: Vec<String> = self.code_panel.open_buffers.keys().cloned().collect();
        for path in paths {
            if let Some(buf) = self.code_panel.open_buffers.get_mut(&path) {
                if buf.modified {
                    let content = buf.to_string();
                    if std::fs::write(&path, &content).is_ok() {
                        buf.modified = false;
                    }
                }
            }
        }
    }

    /// Adjust panel size with keyboard (Ctrl+Alt+Left/Right)
    /// Execute a context menu action
    pub fn execute_context_action(&mut self, action: &str, source: &ContextSource) {
        use crate::panels::code::{ExplorerNaming, ExplorerNamingKind};
        match source {
            ContextSource::Explorer { path, is_dir } => {
                match action {
                    "rename" => {
                        let name = std::path::Path::new(path)
                            .file_name()
                            .and_then(|n| n.to_str())
                            .unwrap_or("")
                            .to_string();
                        self.code_panel.explorer_naming = Some(ExplorerNaming {
                            input: name,
                            kind: ExplorerNamingKind::Rename,
                            original_path: Some(path.clone()),
                        });
                    }
                    "open" => {
                        if *is_dir {
                            self.code_panel.cwd = path.clone();
                            self.code_panel.selected_idx = 0;
                            self.code_panel.refresh_entries();
                        } else {
                            self.code_panel.open_file(path);
                            self.focused = PanelId::Editor;
                        }
                    }
                    "new_file" => {
                        self.code_panel.explorer_naming = Some(ExplorerNaming {
                            input: String::new(),
                            kind: ExplorerNamingKind::NewFile,
                            original_path: None,
                        });
                    }
                    "new_folder" => {
                        self.code_panel.explorer_naming = Some(ExplorerNaming {
                            input: String::new(),
                            kind: ExplorerNamingKind::NewFolder,
                            original_path: None,
                        });
                    }
                    "ref" => {
                        self.prompt_panel.insert_reference(&format!("@{}", path), false);
                    }
                    "include" => {
                        self.prompt_panel.insert_reference(&format!("@@{}", path), true);
                    }
                    "delete" => {
                        if *is_dir {
                            let _ = std::fs::remove_dir(path);
                        } else {
                            let _ = std::fs::remove_file(path);
                        }
                        self.code_panel.refresh_entries();
                    }
                    _ => {}
                }
            }
            ContextSource::PromptProject { name } => {
                match action {
                    "open" => {
                        self.prompt_panel.current_project = Some(name.clone());
                        self.prompt_panel.threads = self.store.list_threads(name).unwrap_or_default();
                        self.prompt_panel.selected_thread = 0;
                    }
                    "rename" => {
                        self.prompt_panel.naming_input = Some(name.clone());
                        self.prompt_panel.naming_what = "rename-project".to_string();
                    }
                    "delete" => {
                        let dir = self.store.project_dir(name);
                        let _ = std::fs::remove_dir_all(&dir);
                        self.prompt_panel.projects = self.store.list_projects().unwrap_or_default();
                    }
                    _ => {}
                }
            }
            ContextSource::PromptThread { project, name } => {
                match action {
                    "open" => {
                        self.prompt_panel.current_thread = Some(name.clone());
                        self.prompt_panel.view = crate::panels::prompt::PromptView::Compose;
                        self.prompt_panel.compose.clear();
                    }
                    "rename" => {
                        self.prompt_panel.naming_input = Some(name.clone());
                        self.prompt_panel.naming_what = "rename-thread".to_string();
                    }
                    "delete" => {
                        let dir = self.store.threads_dir(project);
                        let _ = std::fs::remove_file(dir.join(format!("{}.jsonl", name)));
                        self.prompt_panel.threads = self.store.list_threads(project).unwrap_or_default();
                    }
                    _ => {}
                }
            }
        }
    }

    /// Open context menu for the currently focused panel (keyboard shortcut)
    pub fn open_context_menu_for_focused(&mut self) {
        // Find the panel rect for positioning
        let rect = self.panel_rects.iter()
            .find(|(id, _)| *id == self.focused)
            .map(|(_, r)| *r);
        let (cx, cy) = rect.map_or((10, 5), |r| (r.x + r.width / 3, r.y + r.height / 3));

        match self.focused {
            PanelId::Explorer => {
                if let Some(entry) = self.code_panel.entries.get(self.code_panel.selected_idx) {
                    if entry.name == ".." { return; }
                    let mut items = vec![
                        ContextMenuItem { label: "Rename (F2)".into(), action: "rename".into() },
                    ];
                    if entry.is_dir {
                        items.push(ContextMenuItem { label: "New File".into(), action: "new_file".into() });
                        items.push(ContextMenuItem { label: "New Folder".into(), action: "new_folder".into() });
                        items.push(ContextMenuItem { label: "Delete Folder".into(), action: "delete".into() });
                    } else {
                        items.push(ContextMenuItem { label: "Open".into(), action: "open".into() });
                        items.push(ContextMenuItem { label: "Add @ref".into(), action: "ref".into() });
                        items.push(ContextMenuItem { label: "Add @@include".into(), action: "include".into() });
                        items.push(ContextMenuItem { label: "Delete".into(), action: "delete".into() });
                    }
                    self.context_menu = Some(ContextMenu {
                        x: cx, y: cy,
                        items,
                        selected: 0,
                        source: ContextSource::Explorer {
                            path: entry.path.clone(),
                            is_dir: entry.is_dir,
                        },
                    });
                }
            }
            PanelId::Prompt => {
                use crate::panels::prompt::PromptView;
                if self.prompt_panel.view == PromptView::Browser {
                    if self.prompt_panel.current_project.is_none() {
                        if let Some(name) = self.prompt_panel.projects.get(self.prompt_panel.selected_project).cloned() {
                            self.context_menu = Some(ContextMenu {
                                x: cx, y: cy,
                                items: vec![
                                    ContextMenuItem { label: "Open".into(), action: "open".into() },
                                    ContextMenuItem { label: "Rename (F2)".into(), action: "rename".into() },
                                    ContextMenuItem { label: "Delete".into(), action: "delete".into() },
                                ],
                                selected: 0,
                                source: ContextSource::PromptProject { name },
                            });
                        }
                    } else {
                        if let Some(name) = self.prompt_panel.threads.get(self.prompt_panel.selected_thread).cloned() {
                            let project = self.prompt_panel.current_project.clone().unwrap_or_default();
                            self.context_menu = Some(ContextMenu {
                                x: cx, y: cy,
                                items: vec![
                                    ContextMenuItem { label: "Open".into(), action: "open".into() },
                                    ContextMenuItem { label: "Rename (F2)".into(), action: "rename".into() },
                                    ContextMenuItem { label: "Delete".into(), action: "delete".into() },
                                ],
                                selected: 0,
                                source: ContextSource::PromptThread { project, name },
                            });
                        }
                    }
                }
            }
            _ => {}
        }
    }

    pub fn adjust_explorer_width(&mut self, delta: i16) {
        let new_width = (self.explorer_width as i16 + delta).clamp(15, 60) as u16;
        self.explorer_width = new_width;
    }

    pub fn adjust_right_pane(&mut self, delta: i16) {
        let new_pct = (self.right_pane_percent as i16 + delta).clamp(20, 80) as u16;
        self.right_pane_percent = new_pct;
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
            replace: String::new(),
            matches: Vec::new(),
            current: 0,
            case_sensitive: false,
            use_regex: false,
            editing_replace: false,
        });
    }

    fn find_in_file_search(lines: &[String], query: &str, case_sensitive: bool, use_regex: bool) -> Vec<(usize, usize)> {
        if query.is_empty() {
            return Vec::new();
        }
        let mut matches = Vec::new();

        if use_regex {
            let pattern = if case_sensitive {
                regex::Regex::new(query)
            } else {
                regex::Regex::new(&format!("(?i){}", query))
            };
            if let Ok(re) = pattern {
                for (row, line) in lines.iter().enumerate() {
                    for m in re.find_iter(line) {
                        matches.push((row, m.start()));
                    }
                }
            }
        } else if case_sensitive {
            for (row, line) in lines.iter().enumerate() {
                let mut start = 0;
                while let Some(pos) = line[start..].find(query) {
                    matches.push((row, start + pos));
                    start += pos + query.len();
                }
            }
        } else {
            let query_lower = query.to_lowercase();
            for (row, line) in lines.iter().enumerate() {
                let line_lower = line.to_lowercase();
                let mut start = 0;
                while let Some(pos) = line_lower[start..].find(&query_lower) {
                    matches.push((row, start + pos));
                    start += pos + query_lower.len();
                }
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
            case_sensitive: false,
            use_regex: false,
            file_pattern: String::new(),
            editing_pattern: false,
        });
    }

    fn find_in_workspace_search(cwd: &str, query: &str, case_sensitive: bool, use_regex: bool, file_pattern: &str) -> Vec<GrepResult> {
        use ignore::WalkBuilder;

        if query.is_empty() {
            return Vec::new();
        }
        let mut results = Vec::new();

        let mut walker_builder = WalkBuilder::new(cwd);
        walker_builder.hidden(true).max_depth(Some(10));

        // Apply file pattern filter
        if !file_pattern.is_empty() {
            let mut overrides = ignore::overrides::OverrideBuilder::new(cwd);
            let _ = overrides.add(file_pattern);
            if let Ok(ov) = overrides.build() {
                walker_builder.overrides(ov);
            }
        }

        let re = if use_regex {
            if case_sensitive {
                regex::Regex::new(query).ok()
            } else {
                regex::Regex::new(&format!("(?i){}", query)).ok()
            }
        } else {
            None
        };

        let query_lower = if !case_sensitive { query.to_lowercase() } else { String::new() };

        for entry in walker_builder.build().flatten() {
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
                    let matches = if let Some(ref re) = re {
                        re.is_match(line)
                    } else if case_sensitive {
                        line.contains(query)
                    } else {
                        line.to_lowercase().contains(&query_lower)
                    };
                    if matches {
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
                ref mut replace,
                ref mut matches,
                ref mut current,
                ref mut case_sensitive,
                ref mut use_regex,
                ref mut editing_replace,
            }) => {
                let ctrl = key.modifiers.contains(crossterm::event::KeyModifiers::CONTROL);
                let alt = key.modifiers.contains(crossterm::event::KeyModifiers::ALT);
                match key.code {
                    // Alt+C: toggle case sensitive
                    KeyCode::Char('c') if alt => {
                        *case_sensitive = !*case_sensitive;
                        *matches = Self::find_in_file_search(&self.code_panel.buffer.lines, query, *case_sensitive, *use_regex);
                        *current = 0;
                    }
                    // Alt+R: toggle regex
                    KeyCode::Char('r') if alt => {
                        *use_regex = !*use_regex;
                        *matches = Self::find_in_file_search(&self.code_panel.buffer.lines, query, *case_sensitive, *use_regex);
                        *current = 0;
                    }
                    // Tab: switch between find/replace fields
                    KeyCode::Tab => {
                        *editing_replace = !*editing_replace;
                    }
                    // Ctrl+Shift+1: replace current match
                    KeyCode::Char('h') if ctrl => {
                        if let Some(&(row, col)) = matches.get(*current) {
                            let line = &mut self.code_panel.buffer.lines[row];
                            let end = (col + query.len()).min(line.len());
                            line.replace_range(col..end, replace);
                            self.code_panel.buffer.modified = true;
                            *matches = Self::find_in_file_search(&self.code_panel.buffer.lines, query, *case_sensitive, *use_regex);
                            if *current >= matches.len() { *current = 0; }
                        }
                    }
                    // Ctrl+Alt+Enter: replace all
                    KeyCode::Enter if ctrl && alt => {
                        // Replace all matches (from bottom to top to preserve positions)
                        let mut sorted = matches.clone();
                        sorted.reverse();
                        for (row, col) in sorted {
                            let line = &mut self.code_panel.buffer.lines[row];
                            let end = (col + query.len()).min(line.len());
                            line.replace_range(col..end, replace);
                        }
                        if !matches.is_empty() {
                            self.code_panel.buffer.modified = true;
                        }
                        *matches = Self::find_in_file_search(&self.code_panel.buffer.lines, query, *case_sensitive, *use_regex);
                        *current = 0;
                    }
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
                        if *editing_replace {
                            replace.pop();
                        } else {
                            query.pop();
                            *matches = Self::find_in_file_search(&self.code_panel.buffer.lines, query, *case_sensitive, *use_regex);
                            *current = 0;
                            if let Some(&(row, col)) = matches.first() {
                                self.code_panel.buffer.cursor_row = row;
                                self.code_panel.buffer.cursor_col = col;
                            }
                        }
                    }
                    KeyCode::Char(c) if !ctrl && !alt => {
                        if *editing_replace {
                            replace.push(c);
                        } else {
                            query.push(c);
                            *matches = Self::find_in_file_search(&self.code_panel.buffer.lines, query, *case_sensitive, *use_regex);
                            *current = 0;
                            if let Some(&(row, col)) = matches.first() {
                                self.code_panel.buffer.cursor_row = row;
                                self.code_panel.buffer.cursor_col = col;
                            }
                        }
                    }
                    _ => {}
                }
            }

            Some(Overlay::FindInWorkspace {
                ref mut query,
                ref mut results,
                ref mut selected,
                ref mut case_sensitive,
                ref mut use_regex,
                ref mut file_pattern,
                ref mut editing_pattern,
            }) => {
                let ctrl = key.modifiers.contains(crossterm::event::KeyModifiers::CONTROL);
                let alt = key.modifiers.contains(crossterm::event::KeyModifiers::ALT);
                match key.code {
                    KeyCode::Char('c') if alt => {
                        *case_sensitive = !*case_sensitive;
                        *results = Self::find_in_workspace_search(&self.code_panel.cwd, query, *case_sensitive, *use_regex, file_pattern);
                        *selected = 0;
                    }
                    KeyCode::Char('r') if alt => {
                        *use_regex = !*use_regex;
                        *results = Self::find_in_workspace_search(&self.code_panel.cwd, query, *case_sensitive, *use_regex, file_pattern);
                        *selected = 0;
                    }
                    KeyCode::Tab => {
                        *editing_pattern = !*editing_pattern;
                    }
                    KeyCode::Enter if *editing_pattern => {
                        // Apply pattern and switch back to query
                        *editing_pattern = false;
                        *results = Self::find_in_workspace_search(&self.code_panel.cwd, query, *case_sensitive, *use_regex, file_pattern);
                        *selected = 0;
                    }
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
                    KeyCode::Up if !*editing_pattern => *selected = selected.saturating_sub(1),
                    KeyCode::Down if !*editing_pattern => {
                        if *selected + 1 < results.len() {
                            *selected += 1;
                        }
                    }
                    KeyCode::Backspace => {
                        if *editing_pattern {
                            file_pattern.pop();
                        } else {
                            query.pop();
                            *results = Self::find_in_workspace_search(&self.code_panel.cwd, query, *case_sensitive, *use_regex, file_pattern);
                            *selected = 0;
                        }
                    }
                    KeyCode::Char(c) if !ctrl && !alt => {
                        if *editing_pattern {
                            file_pattern.push(c);
                        } else {
                            query.push(c);
                            *results = Self::find_in_workspace_search(&self.code_panel.cwd, query, *case_sensitive, *use_regex, file_pattern);
                            *selected = 0;
                        }
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
                // Context menu click
                if let Some(ref cm) = self.context_menu.clone() {
                    let menu_width: u16 = 22;
                    let menu_height = cm.items.len() as u16 + 2; // borders
                    if x >= cm.x && x < cm.x + menu_width
                        && y >= cm.y && y < cm.y + menu_height
                    {
                        let item_idx = (y - cm.y).saturating_sub(1) as usize; // -1 for border
                        if item_idx < cm.items.len() {
                            let action = cm.items[item_idx].action.clone();
                            let source = cm.source.clone();
                            self.context_menu = None;
                            self.execute_context_action(&action, &source);
                            return;
                        }
                    }
                    self.context_menu = None;
                    // Fall through to normal click handling
                }

                // Menu bar click (row 0)
                if y == 0 {
                    let menu_idx = if x < 7 { 0 }       // "  File "
                        else if x < 14 { 1 }             // " Edit "
                        else if x < 21 { 2 }             // " View "
                        else { usize::MAX };
                    if menu_idx < 3 {
                        if self.menu.as_ref().map_or(false, |m| m.active_menu == menu_idx) {
                            self.menu = None; // toggle off
                        } else {
                            self.menu = Some(MenuState {
                                active_menu: menu_idx,
                                selected_item: 0,
                                open: true,
                            });
                        }
                    } else {
                        self.menu = None;
                    }
                    return;
                }

                // Menu dropdown item click
                if let Some(ref menu) = self.menu.clone() {
                    let items_count: usize = match menu.active_menu {
                        0 => 6, // File
                        1 => 6, // Edit
                        2 => 5, // View
                        _ => 0,
                    };
                    let dropdown_x: u16 = match menu.active_menu {
                        0 => 0,
                        1 => 7,
                        2 => 14,
                        _ => 0,
                    };
                    let dropdown_width: u16 = 30;
                    let dropdown_top: u16 = 1; // below menu bar
                    let dropdown_bottom = dropdown_top + items_count as u16 + 2; // +2 for borders

                    if x >= dropdown_x && x < dropdown_x + dropdown_width
                        && y > dropdown_top && y < dropdown_bottom - 1
                    {
                        let item_idx = (y - dropdown_top - 1) as usize; // -1 for top border
                        let active = menu.active_menu;
                        self.menu = None;
                        // Dispatch menu action (same as Enter handler in main.rs)
                        match (active, item_idx) {
                            (0, 0) => { self.code_panel.new_file(); }
                            (0, 1) => { self.open_file_finder(); }
                            (0, 2) => {
                                if let Some(ref path) = self.code_panel.file_path.clone() {
                                    let content = self.code_panel.buffer.to_string();
                                    if std::fs::write(path, &content).is_ok() {
                                        self.code_panel.buffer.modified = false;
                                    }
                                } else {
                                    self.save_as_input = Some(String::new());
                                }
                            }
                            (0, 3) => { self.save_as_input = Some(String::new()); }
                            (0, 4) => { self.code_panel.close_current_tab(); }
                            (0, 5) => {
                                self.quit_unsaved_files = self.unsaved_files();
                                self.quit_confirm = true;
                            }
                            (1, 0) => { self.code_panel.buffer.undo(); }
                            (1, 1) => { self.code_panel.buffer.redo(); }
                            (1, 2) => { self.code_panel.buffer.copy(); }
                            (1, 3) => { self.code_panel.buffer.cut(); }
                            (1, 4) => { self.code_panel.buffer.paste(); }
                            (1, 5) => { self.code_panel.buffer.select_all(); }
                            (2, 0) => { self.toggle_panel(PanelId::Explorer); }
                            (2, 1) => { self.toggle_panel(PanelId::Llm); }
                            (2, 2) => { self.toggle_panel(PanelId::Prompt); }
                            (2, 3) => { self.terminal_panel.toggle(); }
                            (2, 4) => {
                                self.code_panel.show_hidden = !self.code_panel.show_hidden;
                                self.code_panel.refresh_entries();
                            }
                            _ => {}
                        }
                        return;
                    }

                    // Clicked outside dropdown — close it
                    self.menu = None;
                    // Don't return — let the click fall through to panels
                }

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
                            // Check if clicking on tab bar (first row of editor panel after border)
                            let tabs = self.code_panel.open_buffer_paths();
                            if ly == 0 && tabs.len() > 0 {
                                // Estimate which tab was clicked based on x position
                                let mut tab_x: u16 = 0;
                                let offset = if self.code_panel.tab_scroll > 0 { 2 } else { 0 };
                                tab_x += offset;
                                for (_i, path) in tabs.iter().skip(self.code_panel.tab_scroll).enumerate() {
                                    let name_len = std::path::Path::new(path)
                                        .file_name()
                                        .and_then(|n| n.to_str())
                                        .unwrap_or(path)
                                        .len() as u16;
                                    let tab_width = name_len + 4; // " name● " + "│"
                                    if lx >= tab_x && lx < tab_x + tab_width {
                                        self.code_panel.switch_to_buffer(path);
                                        break;
                                    }
                                    tab_x += tab_width + 1; // +1 for separator
                                }
                            } else {
                                // Adjust for tab bar + status bar
                                let adjusted_ly = ly.saturating_sub(1); // tab bar
                                click_to_cursor(&mut self.code_panel.buffer, lx, adjusted_ly, 5, true);
                            }
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
                            match self.prompt_panel.view {
                                PromptView::Compose => {
                                    click_to_cursor(&mut self.prompt_panel.compose, lx, ly, 0, true);
                                }
                                PromptView::Browser => {
                                    self.prompt_panel.handle_browser_click(lx, ly, &mut self.store);
                                }
                                _ => {}
                            }
                        }
                        PanelId::Llm => {}
                        PanelId::Terminal => {}
                    }
                }
            }
            MouseEventKind::Down(MouseButton::Right) => {
                // Right-click context menu
                self.context_menu = None; // close any existing

                if let Some((panel_id, rect)) = panel_at {
                    let lx = x.saturating_sub(rect.x + 1);
                    let ly = y.saturating_sub(rect.y + 1);

                    match panel_id {
                        PanelId::Explorer => {
                            let idx = ly as usize + self.code_panel.tree_scroll;
                            if idx < self.code_panel.entries.len() {
                                self.code_panel.selected_idx = idx;
                                let entry = &self.code_panel.entries[idx];
                                if entry.name == ".." {
                                    return;
                                }
                                let mut items = vec![
                                    ContextMenuItem { label: "Rename (F2)".into(), action: "rename".into() },
                                ];
                                if entry.is_dir {
                                    items.push(ContextMenuItem { label: "New File".into(), action: "new_file".into() });
                                    items.push(ContextMenuItem { label: "New Folder".into(), action: "new_folder".into() });
                                    items.push(ContextMenuItem { label: "Delete Folder".into(), action: "delete".into() });
                                } else {
                                    items.push(ContextMenuItem { label: "Open".into(), action: "open".into() });
                                    items.push(ContextMenuItem { label: "Add @ref".into(), action: "ref".into() });
                                    items.push(ContextMenuItem { label: "Add @@include".into(), action: "include".into() });
                                    items.push(ContextMenuItem { label: "Delete".into(), action: "delete".into() });
                                }
                                self.context_menu = Some(ContextMenu {
                                    x, y,
                                    items,
                                    selected: 0,
                                    source: ContextSource::Explorer {
                                        path: entry.path.clone(),
                                        is_dir: entry.is_dir,
                                    },
                                });
                            }
                        }
                        PanelId::Prompt => {
                            use crate::panels::prompt::PromptView;
                            if self.prompt_panel.view == PromptView::Browser {
                                if self.prompt_panel.current_project.is_none() {
                                    let idx = ly as usize;
                                    if idx < self.prompt_panel.projects.len() {
                                        self.prompt_panel.selected_project = idx;
                                        let name = self.prompt_panel.projects[idx].clone();
                                        self.context_menu = Some(ContextMenu {
                                            x, y,
                                            items: vec![
                                                ContextMenuItem { label: "Open".into(), action: "open".into() },
                                                ContextMenuItem { label: "Rename (F2)".into(), action: "rename".into() },
                                                ContextMenuItem { label: "Delete".into(), action: "delete".into() },
                                            ],
                                            selected: 0,
                                            source: ContextSource::PromptProject { name },
                                        });
                                    }
                                } else {
                                    let idx = ly as usize;
                                    if idx < self.prompt_panel.threads.len() {
                                        self.prompt_panel.selected_thread = idx;
                                        let name = self.prompt_panel.threads[idx].clone();
                                        let project = self.prompt_panel.current_project.clone().unwrap_or_default();
                                        self.context_menu = Some(ContextMenu {
                                            x, y,
                                            items: vec![
                                                ContextMenuItem { label: "Open".into(), action: "open".into() },
                                                ContextMenuItem { label: "Rename (F2)".into(), action: "rename".into() },
                                                ContextMenuItem { label: "Delete".into(), action: "delete".into() },
                                            ],
                                            selected: 0,
                                            source: ContextSource::PromptThread { project, name },
                                        });
                                    }
                                }
                            }
                        }
                        _ => {}
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
