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
    pub visible: [bool; 4], // [Explorer, Editor, Llm, Prompt]

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
            focused: PanelId::Editor,
            visible: [true, true, true, true],
            should_quit: false,
            quit_confirm: false,
            panel_rects: Vec::new(),
            mouse_dragging: false,
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
        match self.focused {
            PanelId::Explorer => self.code_panel.handle_explorer_key_pub(key, &mut self.prompt_panel),
            PanelId::Editor => self.code_panel.handle_editor_key_pub(key, &mut self.prompt_panel),
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

                    match panel_id {
                        PanelId::Editor => {
                            click_to_cursor(&mut self.code_panel.buffer, lx, ly, 5, true);
                        }
                        PanelId::Explorer => {
                            let idx = ly as usize + self.code_panel.tree_scroll;
                            if idx < self.code_panel.entries.len() {
                                self.code_panel.selected_idx = idx;
                            }
                        }
                        PanelId::Prompt => {
                            use crate::panels::prompt::PromptView;
                            if self.prompt_panel.view == PromptView::Compose {
                                click_to_cursor(&mut self.prompt_panel.compose, lx, ly, 0, true);
                            }
                        }
                        PanelId::Llm => {}
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
