//! Styled text as data: a screen is a list of lines, a line is a list of spans (`lines.ts`).
use super::format::{cell_width, fit};
use super::types::{Line, Span};

/// The style of a span without its text; `None` leaves a field as it is when applied.
#[derive(Clone, Debug, Default)]
pub struct Style {
    pub color: Option<String>,
    pub bg: Option<String>,
    pub bold: Option<bool>,
    pub dim: Option<bool>,
}

impl Style {
    pub fn color(color: Option<String>) -> Style {
        Style {
            color,
            ..Style::default()
        }
    }

    pub fn bold(mut self) -> Style {
        self.bold = Some(true);
        self
    }
}

pub fn span(text: impl Into<String>, style: Style) -> Span {
    Span {
        text: text.into(),
        color: style.color,
        bg: style.bg,
        bold: style.bold,
        dim: style.dim,
    }
}

/// A span with no style.
pub fn plain(text: impl Into<String>) -> Span {
    span(text, Style::default())
}

pub fn plain_line(line: &[Span]) -> String {
    line.iter().map(|s| s.text.as_str()).collect()
}

pub fn line_width(line: &[Span]) -> usize {
    line.iter().map(|s| cell_width(&s.text)).sum()
}

/// The line cut or padded to exactly `width` cells. Cutting keeps the styles of the spans that survive.
pub fn fit_line(line: &[Span], width: usize) -> Line {
    let mut out: Line = Vec::new();
    let mut used = 0;
    for s in line {
        if width <= used {
            break;
        }
        let room = width - used;
        let w = cell_width(&s.text);
        if w <= room {
            out.push(s.clone());
            used += w;
            continue;
        }
        out.push(Span {
            text: fit(&s.text, room),
            ..s.clone()
        });
        used = width;
        break;
    }
    if used < width {
        out.push(plain(" ".repeat(width - used)));
    }
    out
}

pub fn blank_line(width: usize) -> Line {
    vec![plain(" ".repeat(width))]
}

/// Every span of the line with the style overrides applied.
pub fn style_line(line: &[Span], style: &Style) -> Line {
    line.iter()
        .map(|s| Span {
            text: s.text.clone(),
            color: style.color.clone().or_else(|| s.color.clone()),
            bg: style.bg.clone().or_else(|| s.bg.clone()),
            bold: style.bold.or(s.bold),
            dim: style.dim.or(s.dim),
        })
        .collect()
}

/// Merges neighbouring spans of equal style so the painter emits fewer elements.
pub fn merge_spans(line: impl IntoIterator<Item = Span>) -> Line {
    let mut out: Line = Vec::new();
    for s in line {
        match out.last_mut() {
            Some(last)
                if last.color == s.color
                    && last.bg == s.bg
                    && last.bold == s.bold
                    && last.dim == s.dim =>
            {
                last.text.push_str(&s.text);
            }
            _ => {
                if !s.text.is_empty() {
                    out.push(s);
                }
            }
        }
    }
    out
}

/// The ASCII fallback of a line: the ellipsis becomes `~`. Every other glyph already comes from the ASCII glyph set.
pub fn ascii_line(line: &[Span]) -> Line {
    line.iter()
        .map(|s| {
            if s.text.contains('…') {
                Span {
                    text: s.text.replace('…', "~"),
                    ..s.clone()
                }
            } else {
                s.clone()
            }
        })
        .collect()
}
