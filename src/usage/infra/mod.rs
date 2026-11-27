pub mod claude;
pub mod codex;
pub mod fs_helpers;
pub mod gemini;
pub mod json_lite;
pub mod opencode;

use super::ports::UsageProvider;

pub fn default_providers() -> Vec<Box<dyn UsageProvider>> {
    vec![
        Box::new(claude::ClaudeProvider),
        Box::new(codex::CodexProvider),
        Box::new(gemini::GeminiProvider),
        Box::new(opencode::OpenCodeProvider),
    ]
}
