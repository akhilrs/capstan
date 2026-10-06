//! The view layer: model and view state in, styled lines out. Implemented by the dash-view package.
pub mod types;

pub use types::*;

use crate::model::{DashAction, DashModel};

pub fn make_theme(_options: ThemeOptions) -> Theme {
    unimplemented!("make_theme is implemented by dash-view")
}

impl Theme {
    /// The colour of a role, or `None` when colour is off.
    pub fn color(&self, _role: ColorRole) -> Option<String> {
        unimplemented!("Theme::color is implemented by dash-view")
    }

    /// The gradient colour at `fraction` (0 to 1), or `None` when colour is off.
    pub fn gradient(&self, _fraction: f64) -> Option<String> {
        unimplemented!("Theme::gradient is implemented by dash-view")
    }
}

pub fn build_frame(_model: &DashModel, _view: &ViewState, _theme: &Theme) -> Frame {
    unimplemented!("build_frame is implemented by dash-view")
}

/// Panels that fit on screen, in focus order.
pub fn visible_panels(_model: &DashModel) -> Vec<PanelId> {
    unimplemented!("visible_panels is implemented by dash-view")
}

pub fn footer_hints(_focus: PanelId, _ascii: bool) -> Vec<Hint> {
    unimplemented!("footer_hints is implemented by dash-view")
}

pub fn help_overlay(_size: Size, _theme: &Theme) -> Overlay {
    unimplemented!("help_overlay is implemented by dash-view")
}

pub fn confirm_overlay(_action: &DashAction, _size: Size, _theme: &Theme) -> Overlay {
    unimplemented!("confirm_overlay is implemented by dash-view")
}

pub fn observe_overlay(_peek: &PeekView, _size: Size, _theme: &Theme) -> Overlay {
    unimplemented!("observe_overlay is implemented by dash-view")
}

/// Terminal cells the text takes: wide characters count two.
pub fn cell_width(_text: &str) -> usize {
    unimplemented!("cell_width is implemented by dash-view")
}
