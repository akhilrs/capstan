//! The colour palette, the gradient and the state colours (`theme.ts`).
use super::types::{ColorRole, Theme, ThemeOptions};

/// Truecolor values; the painter downsamples them by the terminal's capability.
pub fn palette(role: ColorRole) -> &'static str {
    match role {
        ColorRole::Fg => "#cccccc",
        ColorRole::Bright => "#eeeeee",
        ColorRole::Dim => "#6c6c6c",
        ColorRole::Ok => "#77ca9b",
        ColorRole::Warn => "#cbc06c",
        ColorRole::Bad => "#dc4c4c",
        ColorRole::Info => "#6cb4d8",
        ColorRole::SelectBg => "#3b4252",
        ColorRole::BorderHeader => "#5a5a5a",
        ColorRole::BorderAgents => "#556d59",
        ColorRole::BorderAgentsFocus => "#77ca9b",
        ColorRole::BorderPipeline => "#5c588d",
        ColorRole::BorderPipelineFocus => "#8a85c9",
        ColorRole::BorderQueue => "#805252",
        ColorRole::BorderQueueFocus => "#d17f7f",
        ColorRole::BorderFindings => "#6c6c4b",
        ColorRole::BorderFindingsFocus => "#cbc06c",
        ColorRole::BorderWork => "#4b6a7a",
        ColorRole::BorderWorkFocus => "#7fb3cf",
        ColorRole::Overlay => "#eeeeee",
    }
}

const GRADIENT_STOPS: [(f64, ColorRole); 3] = [
    (0.0, ColorRole::Ok),
    (0.6, ColorRole::Warn),
    (1.0, ColorRole::Bad),
];

fn channels(hex: &str) -> [i64; 3] {
    let n = i64::from_str_radix(&hex[1..], 16).unwrap_or(0);
    [(n >> 16) & 255, (n >> 8) & 255, n & 255]
}

/// The meter and graph colour at `fraction` (0 to 1): ok, then warn at 60 %, then bad.
pub fn gradient_color(fraction: f64) -> String {
    let t = fraction.clamp(0.0, 1.0);
    let upper = GRADIENT_STOPS.iter().position(|&(at, _)| t <= at);
    let Some(upper) = upper.filter(|&u| u > 0) else {
        return palette(GRADIENT_STOPS[0].1).to_string();
    };
    let (from_at, from_role) = GRADIENT_STOPS[upper - 1];
    let (to_at, to_role) = GRADIENT_STOPS[upper];
    let k = (t - from_at) / (to_at - from_at);
    let a = channels(palette(from_role));
    let b = channels(palette(to_role));
    let mut out = String::from("#");
    for i in 0..3 {
        let value = (a[i] as f64 + (b[i] - a[i]) as f64 * k).round() as i64;
        out.push_str(&format!("{value:02x}"));
    }
    out
}

pub fn make_theme(options: ThemeOptions) -> Theme {
    Theme {
        no_color: options.no_color,
        reduced_motion: options.reduced_motion,
        ascii: options.ascii,
    }
}

impl Theme {
    /// The colour of a role, or `None` when colour is off.
    pub fn color(&self, role: ColorRole) -> Option<String> {
        (!self.no_color).then(|| palette(role).to_string())
    }

    /// The gradient colour at `fraction` (0 to 1), or `None` when colour is off.
    pub fn gradient(&self, fraction: f64) -> Option<String> {
        (!self.no_color).then(|| gradient_color(fraction))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StateColor {
    Ok,
    Warn,
    Bad,
    Dim,
}

impl StateColor {
    pub fn role(self) -> ColorRole {
        match self {
            StateColor::Ok => ColorRole::Ok,
            StateColor::Warn => ColorRole::Warn,
            StateColor::Bad => ColorRole::Bad,
            StateColor::Dim => ColorRole::Dim,
        }
    }
}

/// Colour is never the only carrier of meaning: every state also has a word or a symbol.
pub fn color_of_state(state: &str) -> StateColor {
    match state {
        "failed" | "conflicted" | "degraded" | "escalated" | "expired" | "rejected" | "unacked"
        | "findings" => StateColor::Bad,
        "deferred" | "queued" | "sent" | "started" | "running" | "evaluating" | "open" => {
            StateColor::Warn
        }
        "passed" | "merged" | "confirmed" | "accepted" | "healthy" | "active" | "acked" => {
            StateColor::Ok
        }
        _ => StateColor::Dim,
    }
}
