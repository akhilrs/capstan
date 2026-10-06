//! Titled borders: panel title, hotkey number and tabs embedded in the top edge, tabs and a counter in the bottom edge (`border.ts`).
use super::format::{cell_width, truncate};
use super::glyphs::Glyphs;
use super::lines::{span, Style};
use super::types::{Line, Span};

#[derive(Clone, Debug)]
pub struct Tab {
    pub text: String,
    pub color: Option<String>,
}

impl Tab {
    pub fn new(text: impl Into<String>, color: Option<String>) -> Tab {
        Tab {
            text: text.into(),
            color,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct BorderColors {
    pub border: Option<String>,
    pub title: Option<String>,
}

fn tab_spans(tabs: &[Tab], g: &Glyphs, border: &Option<String>) -> Vec<Span> {
    let edge = || Style::color(border.clone());
    tabs.iter()
        .flat_map(|tab| {
            [
                span(g.tab_open, edge()),
                span(format!(" {} ", tab.text), Style::color(tab.color.clone())),
                span(g.tab_close, edge()),
            ]
        })
        .collect()
}

fn tabs_width(tabs: &[Tab]) -> usize {
    tabs.iter().map(|t| cell_width(&t.text) + 4).sum()
}

#[derive(Clone, Debug, Default)]
pub struct TopBorder {
    pub number: Option<usize>,
    pub title: String,
    /// Tabs right after the title, for the header.
    pub left_tabs: Vec<Tab>,
    /// Right-aligned tabs; the last ones are dropped first when the line is too narrow.
    pub tabs: Vec<Tab>,
    pub focused: bool,
}

/// The top edge as one line of exactly `width` cells.
pub fn top_border(width: usize, o: &TopBorder, g: &Glyphs, colors: &BorderColors) -> Line {
    let e = if o.focused { g.edge_focused } else { g.edge };
    let prefix = o
        .number
        .map(|n| g.hotkey.get(n).copied().unwrap_or(""))
        .unwrap_or("");
    let mut title = format!("{prefix}{}", o.title);
    let mut left = o.left_tabs.clone();
    let mut right = o.tabs.clone();
    let fill = |title: &str, left: &[Tab], right: &[Tab]| -> i64 {
        width as i64
            - 3
            - cell_width(title) as i64
            - if left.is_empty() {
                0
            } else {
                1 + tabs_width(left) as i64
            }
            - if right.is_empty() {
                0
            } else {
                tabs_width(right) as i64 + 1
            }
    };
    while fill(&title, &left, &right) < 0 && !right.is_empty() {
        right.pop();
    }
    while fill(&title, &left, &right) < 0 && !left.is_empty() {
        left.pop();
    }
    let f = fill(&title, &left, &right);
    if f < 0 {
        let keep = (cell_width(&title) as i64 + f).max(1) as usize;
        title = truncate(&title, keep);
    }
    let b = &colors.border;
    let mut out = vec![
        span(format!("{}{}", e.tl, e.h), Style::color(b.clone())),
        span(title, Style::color(colors.title.clone()).bold()),
    ];
    if !left.is_empty() {
        out.push(span(e.h, Style::color(b.clone())));
        out.extend(tab_spans(&left, g, b));
    }
    out.push(span(e.h.repeat(f.max(0) as usize), Style::color(b.clone())));
    out.extend(tab_spans(&right, g, b));
    out.push(span(
        format!("{}{}", if right.is_empty() { "" } else { e.h }, e.tr),
        Style::color(b.clone()),
    ));
    out
}

#[derive(Clone, Debug, Default)]
pub struct BottomBorder {
    pub left: Vec<Tab>,
    pub right: Vec<Tab>,
    pub focused: bool,
}

/// The bottom edge as one line of exactly `width` cells.
pub fn bottom_border(width: usize, o: &BottomBorder, g: &Glyphs, border: &Option<String>) -> Line {
    let e = if o.focused { g.edge_focused } else { g.edge };
    let mut left = o.left.clone();
    let mut right = o.right.clone();
    let fill = |left: &[Tab], right: &[Tab]| -> i64 {
        width as i64
            - 2
            - if left.is_empty() {
                0
            } else {
                1 + tabs_width(left) as i64
            }
            - if right.is_empty() {
                0
            } else {
                tabs_width(right) as i64 + 1
            }
    };
    while fill(&left, &right) < 0 && !left.is_empty() {
        left.pop();
    }
    while fill(&left, &right) < 0 && !right.is_empty() {
        right.pop();
    }
    let f = fill(&left, &right).max(0) as usize;
    let mut out = vec![span(
        format!("{}{}", e.bl, if left.is_empty() { "" } else { e.h }),
        Style::color(border.clone()),
    )];
    out.extend(tab_spans(&left, g, border));
    out.push(span(e.h.repeat(f), Style::color(border.clone())));
    out.extend(tab_spans(&right, g, border));
    out.push(span(
        format!("{}{}", if right.is_empty() { "" } else { e.h }, e.br),
        Style::color(border.clone()),
    ));
    out
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Thumb {
    pub start: usize,
    pub size: usize,
}

/// The scroll thumb range inside `rows` body rows: where the visible window of `view` out of `total` sits.
pub fn thumb_range(total: usize, view: usize, top: usize, rows: usize) -> Option<Thumb> {
    if total <= view || view == 0 || rows == 0 {
        return None;
    }
    let size = (((view as f64 / total as f64) * rows as f64).round() as usize).max(1);
    let travel = rows as f64 - size as f64;
    let start = ((top as f64 / (total - view) as f64) * travel)
        .round()
        .max(0.0) as usize;
    Some(Thumb { start, size })
}
