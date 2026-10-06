//! The glyph sets: Unicode, and the ASCII fallback used for TERM=dumb and non-UTF-8 locales (`glyphs.ts`).

#[derive(Clone, Copy, Debug)]
pub struct Edge {
    pub tl: &'static str,
    pub tr: &'static str,
    pub bl: &'static str,
    pub br: &'static str,
    pub h: &'static str,
    pub v: &'static str,
}

#[derive(Clone, Copy, Debug)]
pub struct Glyphs {
    pub ascii: bool,
    pub edge: Edge,
    pub edge_focused: Edge,
    pub tab_open: &'static str,
    pub tab_close: &'static str,
    /// Hotkey number prefix for panel 1 to 5 (index 0 unused).
    pub hotkey: [&'static str; 6],
    /// One entry per animation frame.
    pub spinner: &'static [&'static str],
    pub working: &'static str,
    pub idle: &'static str,
    pub attention: &'static str,
    pub ended: &'static str,
    pub link_up: &'static str,
    pub link_down: &'static str,
    pub full: &'static str,
    pub empty: &'static str,
    pub stage_good: &'static str,
    pub stage_progress: &'static str,
    pub stage_bad: &'static str,
    pub selected: &'static str,
    pub changed: &'static str,
    pub thumb: &'static str,
    pub arrow: &'static str,
    pub rule: &'static str,
    pub notified: &'static str,
    pub not_notified: &'static str,
    pub up: &'static str,
    pub down: &'static str,
}

const UNICODE: Glyphs = Glyphs {
    ascii: false,
    edge: Edge {
        tl: "╭",
        tr: "╮",
        bl: "╰",
        br: "╯",
        h: "─",
        v: "│",
    },
    edge_focused: Edge {
        tl: "┏",
        tr: "┓",
        bl: "┗",
        br: "┛",
        h: "━",
        v: "┃",
    },
    tab_open: "┤",
    tab_close: "├",
    hotkey: ["", "¹", "²", "³", "⁴", "⁵"],
    spinner: &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"],
    working: "*",
    idle: "●",
    attention: "▲",
    ended: "○",
    link_up: "●",
    link_down: "○",
    full: "█",
    empty: "░",
    stage_good: "█",
    stage_progress: "▓",
    stage_bad: "▒",
    selected: "▌",
    changed: "+",
    thumb: "█",
    arrow: "──►",
    rule: "─",
    notified: "●",
    not_notified: "·",
    up: "↑",
    down: "↓",
};

const ASCII: Glyphs = Glyphs {
    ascii: true,
    edge: Edge {
        tl: "+",
        tr: "+",
        bl: "+",
        br: "+",
        h: "-",
        v: "|",
    },
    edge_focused: Edge {
        tl: "+",
        tr: "+",
        bl: "+",
        br: "+",
        h: "=",
        v: "|",
    },
    tab_open: "[",
    tab_close: "]",
    hotkey: ["", "1.", "2.", "3.", "4.", "5."],
    spinner: &["|", "/", "-", "\\"],
    working: "*",
    idle: "o",
    attention: "!",
    ended: "-",
    link_up: "*",
    link_down: "o",
    full: "#",
    empty: ".",
    stage_good: "#",
    stage_progress: "+",
    stage_bad: "x",
    selected: ">",
    changed: "+",
    thumb: "#",
    arrow: "->",
    rule: "-",
    notified: "*",
    not_notified: ".",
    up: "up",
    down: "down",
};

pub fn glyphs_for(ascii: bool) -> Glyphs {
    if ascii {
        ASCII
    } else {
        UNICODE
    }
}

impl Glyphs {
    /// The spinner frame for a tick.
    pub fn spinner_at(&self, tick: u64) -> &'static str {
        self.spinner[(tick % self.spinner.len() as u64) as usize]
    }
}
