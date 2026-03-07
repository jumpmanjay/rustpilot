use anyhow::Result;
use crossterm::event::KeyEvent;
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

    pub fn visible_panels(&self) -> Vec<PanelId> {
        self.layout
            .iter()
            .copied()
            .filter(|p| self.visible[*p as usize])
            .collect()
    }
}
