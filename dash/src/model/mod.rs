//! The view model: one operator `status` response in, display rows out.
pub mod actions;
pub mod helpers;
pub mod text;
pub mod time;
pub mod types;

pub use actions::*;
pub use helpers::*;
pub use types::*;

/// One operator `status` response to display rows. Implemented by the dash-model package.
pub fn build_dash_model(
    _status: &serde_json::Value,
    _now_ms: i64,
    _worker_limit: Option<i64>,
) -> DashModel {
    unimplemented!("build_dash_model is implemented by dash-model")
}
