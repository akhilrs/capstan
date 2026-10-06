//! The view layer: model and view state in, styled lines out (`view.ts`, `layout.ts`, `format.ts`, `lines.ts`,
//! `border.ts`, `glyphs.ts`, `theme.ts`, `graph.ts`, `waiting.ts`, `overlays.ts`).
pub mod border;
pub mod format;
pub mod frame;
pub mod glyphs;
pub mod graph;
pub mod header;
pub mod layout;
pub mod lines;
pub mod overlays;
pub mod panels;
pub mod theme;
pub mod types;
pub mod waiting;

pub use format::cell_width;
pub use frame::{build_frame, footer_hints, visible_panels};
pub use overlays::{confirm_overlay, help_overlay, observe_overlay};
pub use theme::make_theme;
pub use types::*;
