//! The commented starter capstan.toml and the designer role prompt that `cstan init` writes, byte for byte.

/// `STARTER_CONFIG` of `src/config/starter.ts`.
pub const STARTER_CONFIG: &str = include_str!("starter.toml");

/// The path older starters wrote the designer prompt to (and pointed `prompt_file` at); the prompt is now built in.
/// Kept for the legacy-project case. Originally `DESIGNER_PROMPT_PATH` of `src/roles/designer-prompt.ts`.
pub const DESIGNER_PROMPT_PATH: &str = "roles/designer.md";

/// Alias of [`crate::prompts::DESIGNER_PROMPT`].
pub const DESIGNER_PROMPT: &str = crate::prompts::DESIGNER_PROMPT;
