//! The view model: one operator `status` response in, display rows out.
pub mod actions;
mod build;
pub mod helpers;
mod pipeline;
mod tasks;
pub mod text;
pub mod time;
pub mod types;
mod waiting;

pub use actions::*;
pub use build::build_dash_model;
pub use helpers::*;
pub use types::*;
