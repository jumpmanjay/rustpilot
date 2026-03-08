pub mod code;
pub mod editor;
pub mod llm;
pub mod prompt;
pub mod terminal;

pub use code::CodePanel;
pub use llm::LlmPanel;
pub use prompt::PromptPanel;
pub use terminal::TerminalPanel;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanelId {
    Explorer = 0,
    Editor = 1,
    Llm = 2,
    Prompt = 3,
    Terminal = 4,
}
