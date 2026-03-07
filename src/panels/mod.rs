pub mod code;
pub mod editor;
pub mod llm;
pub mod prompt;

pub use code::CodePanel;
pub use llm::LlmPanel;
pub use prompt::PromptPanel;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanelId {
    Explorer = 0,
    Editor = 1,
    Llm = 2,
    Prompt = 3,
}
