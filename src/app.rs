use anyhow::Result;
use crossterm::event::{KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use crate::config::Config;
use crate::llm::LlmManager;
use crate::panels::{CodePanel, LlmPanel, PanelId, PromptPanel};
use crate::storage::Store;

/// Top-level application state
pub struct App {
    pub config: Config,
    pub store: Store,
    pub llm: LlmManager,

    // Panels
    pub code_panel: CodePanel,
    pub llm_panel: LlmPanel,
    pub prompt_panel: PromptPanel,

    // Layout
    pub focused: PanelId,
    pub visible: [bool; 3], // [Code, Llm, Prompt]

    // Panel order: which panel is in which position
    // Default: [Code, Llm, Prompt]
    pub layout: [PanelId; 3],

    pub should_quit: bool,
    pub quit_confirm: bool,

    /// Panel rectangles from last render (for mouse hit-testing)
    pub panel_rects: Vec<(PanelId, Rect)>,
    /// Whether mouse is currently dragging (for selection)
    pub mouse_dragging: bool,
}

impl App {
    pub fn new() -> Result<Self> {
        let config = Config::load_or_default()?;
        let store = Store::new(&config)?;
        let llm = LlmManager::new(&config);

        Ok(Self {
            config,
            store,
            llm,
            code_panel: CodePanel::new(),
            llm_panel: LlmPanel::new(),
            prompt_panel: PromptPanel::new(),
            focused: PanelId::Code,
            visible: [true, true, true],
            layout: [PanelId::Code, PanelId::Llm, PanelId::Prompt],
            should_quit: false,
            quit_confirm: false,
            panel_rects: Vec::new(),
            mouse_dragging: false,
        })
    }

    pub fn toggle_panel(&mut self, panel: PanelId) {
        let idx = panel as usize;
        self.visible[idx] = !self.visible[idx];

        // If we hid the focused panel, move focus to next visible
        if !self.visible[self.focused as usize] {
            self.cycle_focus();
        }
    }

    pub fn cycle_focus(&mut self) {
        let start = self.focused as usize;
        for i in 1..=3 {
            let next = (start + i) % 3;
            if self.visible[next] {
                self.focused = match next {
                    0 => PanelId::Code,
                    1 => PanelId::Llm,
                    2 => PanelId::Prompt,
                    _ => unreachable!(),
                };
                return;
            }
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent) {
        match self.focused {
            PanelId::Code => self.code_panel.handle_key(key, &mut self.prompt_panel),
            PanelId::Llm => self.llm_panel.handle_key(key),
            PanelId::Prompt => self.prompt_panel.handle_key(key, &mut self.llm, &mut self.store),
        }
    }

    pub fn poll_llm_updates(&mut self) {
        self.llm.poll_updates(&mut self.llm_panel);
    }

    pub fn handle_mouse(&mut self, mouse: MouseEvent) {
        let x = mouse.column;
        let y = mouse.row;

        // Find which panel the mouse is over
        let panel_at = self
            .panel_rects
            .iter()
            .find(|(_, rect)| {
                x >= rect.x
                    && x < rect.x + rect.width
                    && y >= rect.y
                    && y < rect.y + rect.height
            })
            .map(|(id, rect)| (*id, *rect));

        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                if let Some((panel_id, rect)) = panel_at {
                    // Focus the clicked panel
                    self.focused = panel_id;
                    self.mouse_dragging = true;

                    // Position cursor in editor panels
                    let local_x = x.saturating_sub(rect.x + 1); // +1 for border
                    let local_y = y.saturating_sub(rect.y + 1); // +1 for border

                    match panel_id {
                        PanelId::Code => {
                            use crate::panels::code::CodeView;
                            match self.code_panel.view {
                                CodeView::Editor => {
                                    let buf = &mut self.code_panel.buffer;
                                    let gutter = 5u16;
                                    let col = local_x.saturating_sub(gutter) as usize + buf.scroll_col;
                                    let row = local_y as usize + buf.scroll_row;
                                    let row = row.min(buf.lines.len().saturating_sub(1));
                                    let col = col.min(buf.lines[row].len());
                                    buf.select_anchor = None;
                                    buf.cursor_row = row;
                                    buf.cursor_col = col;
                                }
                                CodeView::Explorer => {
                                    let idx = local_y as usize;
                                    if idx < self.code_panel.entries.len() {
                                        self.code_panel.selected_idx = idx;
                                    }
                                }
                            }
                        }
                        PanelId::Prompt => {
                            use crate::panels::prompt::PromptView;
                            if self.prompt_panel.view == PromptView::Compose {
                                let buf = &mut self.prompt_panel.compose;
                                let col = local_x as usize + buf.scroll_col;
                                let row = local_y as usize + buf.scroll_row;
                                let row = row.min(buf.lines.len().saturating_sub(1));
                                let col = col.min(buf.lines[row].len());
                                buf.select_anchor = None;
                                buf.cursor_row = row;
                                buf.cursor_col = col;
                            }
                        }
                        PanelId::Llm => {
                            // Click in LLM panel — just focus, no cursor
                        }
                    }
                }
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                if !self.mouse_dragging {
                    return;
                }
                if let Some((panel_id, rect)) = panel_at {
                    let local_x = x.saturating_sub(rect.x + 1);
                    let local_y = y.saturating_sub(rect.y + 1);

                    match panel_id {
                        PanelId::Code => {
                            use crate::panels::code::CodeView;
                            if self.code_panel.view == CodeView::Editor {
                                let buf = &mut self.code_panel.buffer;
                                let gutter = 5u16;
                                let col = local_x.saturating_sub(gutter) as usize + buf.scroll_col;
                                let row = local_y as usize + buf.scroll_row;
                                let row = row.min(buf.lines.len().saturating_sub(1));
                                let col = col.min(buf.lines[row].len());
                                // Start selection if not already
                                if buf.select_anchor.is_none() {
                                    buf.select_anchor =
                                        Some((buf.cursor_row, buf.cursor_col));
                                }
                                buf.cursor_row = row;
                                buf.cursor_col = col;
                            }
                        }
                        PanelId::Prompt => {
                            use crate::panels::prompt::PromptView;
                            if self.prompt_panel.view == PromptView::Compose {
                                let buf = &mut self.prompt_panel.compose;
                                let col = local_x as usize + buf.scroll_col;
                                let row = local_y as usize + buf.scroll_row;
                                let row = row.min(buf.lines.len().saturating_sub(1));
                                let col = col.min(buf.lines[row].len());
                                if buf.select_anchor.is_none() {
                                    buf.select_anchor =
                                        Some((buf.cursor_row, buf.cursor_col));
                                }
                                buf.cursor_row = row;
                                buf.cursor_col = col;
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
                        PanelId::Code => {
                            use crate::panels::code::CodeView;
                            match self.code_panel.view {
                                CodeView::Editor => {
                                    let buf = &mut self.code_panel.buffer;
                                    buf.scroll_row = buf.scroll_row.saturating_sub(3);
                                }
                                CodeView::Explorer => {
                                    if self.code_panel.selected_idx >= 3 {
                                        self.code_panel.selected_idx -= 3;
                                    } else {
                                        self.code_panel.selected_idx = 0;
                                    }
                                }
                            }
                        }
                        PanelId::Llm => {
                            self.llm_panel.scroll_offset += 3;
                            self.llm_panel.following = false;
                        }
                        PanelId::Prompt => {
                            use crate::panels::prompt::PromptView;
                            match self.prompt_panel.view {
                                PromptView::Compose => {
                                    let buf = &mut self.prompt_panel.compose;
                                    buf.scroll_row = buf.scroll_row.saturating_sub(3);
                                }
                                PromptView::History => {
                                    self.prompt_panel.history_scroll += 3;
                                }
                                PromptView::Browser => {
                                    if self.prompt_panel.current_project.is_some() {
                                        self.prompt_panel.selected_thread =
                                            self.prompt_panel.selected_thread.saturating_sub(3);
                                    } else {
                                        self.prompt_panel.selected_project =
                                            self.prompt_panel.selected_project.saturating_sub(3);
                                    }
                                }
                            }
                        }
                    }
                }
            }
            MouseEventKind::ScrollDown => {
                if let Some((panel_id, _)) = panel_at {
                    match panel_id {
                        PanelId::Code => {
                            use crate::panels::code::CodeView;
                            match self.code_panel.view {
                                CodeView::Editor => {
                                    let buf = &mut self.code_panel.buffer;
                                    buf.scroll_row = (buf.scroll_row + 3)
                                        .min(buf.lines.len().saturating_sub(1));
                                }
                                CodeView::Explorer => {
                                    self.code_panel.selected_idx = (self
                                        .code_panel
                                        .selected_idx
                                        + 3)
                                    .min(
                                        self.code_panel.entries.len().saturating_sub(1),
                                    );
                                }
                            }
                        }
                        PanelId::Llm => {
                            self.llm_panel.scroll_offset =
                                self.llm_panel.scroll_offset.saturating_sub(3);
                            if self.llm_panel.scroll_offset == 0 {
                                self.llm_panel.following = true;
                            }
                        }
                        PanelId::Prompt => {
                            use crate::panels::prompt::PromptView;
                            match self.prompt_panel.view {
                                PromptView::Compose => {
                                    let buf = &mut self.prompt_panel.compose;
                                    buf.scroll_row = (buf.scroll_row + 3)
                                        .min(buf.lines.len().saturating_sub(1));
                                }
                                PromptView::History => {
                                    self.prompt_panel.history_scroll =
                                        self.prompt_panel.history_scroll.saturating_sub(3);
                                }
                                PromptView::Browser => {
                                    if self.prompt_panel.current_project.is_some() {
                                        self.prompt_panel.selected_thread =
                                            (self.prompt_panel.selected_thread + 3).min(
                                                self.prompt_panel
                                                    .threads
                                                    .len()
                                                    .saturating_sub(1),
                                            );
                                    } else {
                                        self.prompt_panel.selected_project =
                                            (self.prompt_panel.selected_project + 3).min(
                                                self.prompt_panel
                                                    .projects
                                                    .len()
                                                    .saturating_sub(1),
                                            );
                                    }
                                }
                            }
                        }
                    }
                }
            }
            MouseEventKind::Down(MouseButton::Middle) => {
                // Middle-click paste (Unix convention)
                if let Some((panel_id, _)) = panel_at {
                    self.focused = panel_id;
                    match panel_id {
                        PanelId::Code => {
                            use crate::panels::code::CodeView;
                            if self.code_panel.view == CodeView::Editor {
                                self.code_panel.buffer.paste();
                            }
                        }
                        PanelId::Prompt => {
                            use crate::panels::prompt::PromptView;
                            if self.prompt_panel.view == PromptView::Compose {
                                self.prompt_panel.compose.paste();
                            }
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }

    pub fn visible_panels(&self) -> Vec<PanelId> {
        self.layout
            .iter()
            .copied()
            .filter(|p| self.visible[*p as usize])
            .collect()
    }
}
