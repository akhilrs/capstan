//! The commented starter capstan.toml and the designer role prompt that `cstan init` writes, byte for byte.

/// `STARTER_CONFIG` of `src/config/starter.ts`.
pub const STARTER_CONFIG: &str = include_str!("starter.toml");

/// `DESIGNER_PROMPT_PATH` of `src/roles/designer-prompt.ts`.
pub const DESIGNER_PROMPT_PATH: &str = "roles/designer.md";

/// `DESIGNER_PROMPT`: exactly the bytes of the tracked roles/designer.md.
pub const DESIGNER_PROMPT: &str = include_str!("designer.md");
