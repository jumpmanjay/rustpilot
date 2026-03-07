pub mod code;
pub mod llm;
pub mod prompt;

pub use code::CodePanel;
pub use llm::LlmPanel;
pub use prompt::PromptPanel;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanelId {
    Code = 0,
    Llm = 1,
    Prompt = 2,
}
