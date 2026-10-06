//! Frame lines and overlays into the ratatui buffer, one terminal cell at a time.
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};

use crate::view::{Line, Overlay, Span};

/// How many colours the terminal shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColorDepth {
    TrueColor,
    Ansi256,
    Ansi16,
}

impl ColorDepth {
    /// From `COLORTERM` and `TERM`: `truecolor` or `24bit` means 24-bit colour, a `256color` terminal 256 colours, else 16.
    pub fn detect(colorterm: Option<&str>, term: Option<&str>) -> Self {
        let colorterm = colorterm.unwrap_or("").to_ascii_lowercase();
        if colorterm == "truecolor" || colorterm == "24bit" {
            ColorDepth::TrueColor
        } else if term.is_some_and(|t| t.contains("256")) {
            ColorDepth::Ansi256
        } else {
            ColorDepth::Ansi16
        }
    }
}

fn parse_hex(hex: &str) -> Option<(u8, u8, u8)> {
    let digits = hex.strip_prefix('#')?;
    let value = |from: usize| u8::from_str_radix(digits.get(from..from + 2)?, 16).ok();
    match digits.len() {
        6 => Some((value(0)?, value(2)?, value(4)?)),
        3 => {
            let one = |from: usize| u8::from_str_radix(digits.get(from..from + 1)?, 16).ok();
            Some((one(0)? * 17, one(1)? * 17, one(2)? * 17))
        }
        _ => None,
    }
}

/// The xterm 256-colour index nearest to an RGB colour (grey ramp or 6x6x6 cube).
fn nearest_256(r: u8, g: u8, b: u8) -> u8 {
    if r == g && g == b {
        return match r {
            0..=7 => 16,
            248..=255 => 231,
            _ => 232 + ((u16::from(r) - 8) * 24 / 240) as u8,
        };
    }
    let level = |c: u8| (u16::from(c) * 5 + 127) / 255;
    (16 + 36 * level(r) + 6 * level(g) + level(b)) as u8
}

/// The basic 16 colours (xterm's default values) with the nearest one chosen by distance in RGB.
fn nearest_16(r: u8, g: u8, b: u8) -> Color {
    const PALETTE: [(u8, u8, u8, Color); 16] = [
        (0, 0, 0, Color::Black),
        (205, 0, 0, Color::Red),
        (0, 205, 0, Color::Green),
        (205, 205, 0, Color::Yellow),
        (0, 0, 238, Color::Blue),
        (205, 0, 205, Color::Magenta),
        (0, 205, 205, Color::Cyan),
        (229, 229, 229, Color::Gray),
        (127, 127, 127, Color::DarkGray),
        (255, 0, 0, Color::LightRed),
        (0, 255, 0, Color::LightGreen),
        (255, 255, 0, Color::LightYellow),
        (92, 92, 255, Color::LightBlue),
        (255, 0, 255, Color::LightMagenta),
        (0, 255, 255, Color::LightCyan),
        (255, 255, 255, Color::White),
    ];
    let distance = |p: &(u8, u8, u8, Color)| {
        let d = |a: u8, b: u8| (i32::from(a) - i32::from(b)).pow(2);
        d(p.0, r) + d(p.1, g) + d(p.2, b)
    };
    PALETTE
        .iter()
        .min_by_key(|p| distance(p))
        .map_or(Color::Reset, |p| p.3)
}

/// A `#rrggbb` colour at the terminal's depth; `None` when the text is not a hex colour.
pub fn color_of(hex: &str, depth: ColorDepth) -> Option<Color> {
    let (r, g, b) = parse_hex(hex)?;
    Some(match depth {
        ColorDepth::TrueColor => Color::Rgb(r, g, b),
        ColorDepth::Ansi256 => Color::Indexed(nearest_256(r, g, b)),
        ColorDepth::Ansi16 => nearest_16(r, g, b),
    })
}

fn style_of(span: &Span, depth: ColorDepth) -> Style {
    let mut style = Style::default();
    if let Some(color) = span.color.as_deref().and_then(|c| color_of(c, depth)) {
        style = style.fg(color);
    }
    if let Some(color) = span.bg.as_deref().and_then(|c| color_of(c, depth)) {
        style = style.bg(color);
    }
    if span.bold == Some(true) {
        style = style.add_modifier(Modifier::BOLD);
    }
    if span.dim == Some(true) {
        style = style.add_modifier(Modifier::DIM);
    }
    style
}

/// Paints one line at row `y` from column `x`, clipped at `area`. A wide character takes two cells; a zero-width one
/// joins the cell before it.
pub fn paint_line(
    buffer: &mut Buffer,
    area: Rect,
    x: u16,
    y: u16,
    line: &Line,
    depth: ColorDepth,
    cell_width: fn(&str) -> usize,
) {
    if y >= area.height {
        return;
    }
    let right = area.width;
    let mut column = x;
    let mut previous: Option<u16> = None;
    for span in line {
        let style = style_of(span, depth);
        for c in span.text.chars() {
            let mut one = [0u8; 4];
            let text: &str = c.encode_utf8(&mut one);
            let width = cell_width(text);
            if width == 0 {
                if let Some(at) = previous {
                    if let Some(cell) = buffer.cell_mut((area.x + at, area.y + y)) {
                        let joined = format!("{}{text}", cell.symbol());
                        cell.set_symbol(&joined);
                    }
                }
                continue;
            }
            if column >= right || column + width as u16 > right {
                return;
            }
            // Half of a wide character left behind by this one would draw garbage: blank it.
            if column > 0 {
                if let Some(cell) = buffer.cell_mut((area.x + column - 1, area.y + y)) {
                    if cell_width(cell.symbol()) > 1 {
                        cell.set_symbol(" ");
                    }
                }
            }
            if let Some(cell) = buffer.cell_mut((area.x + column + width as u16, area.y + y)) {
                if cell.symbol().is_empty() {
                    cell.set_symbol(" ");
                }
            }
            if let Some(cell) = buffer.cell_mut((area.x + column, area.y + y)) {
                cell.set_symbol(text);
                cell.set_style(style);
            }
            for follow in 1..width as u16 {
                if let Some(cell) = buffer.cell_mut((area.x + column + follow, area.y + y)) {
                    cell.set_symbol("");
                    cell.set_style(style);
                }
            }
            previous = Some(column);
            column += width as u16;
        }
    }
}

/// The dashboard lines, then the overlay on top of them.
pub fn paint_screen(
    buffer: &mut Buffer,
    area: Rect,
    lines: &[Line],
    overlay: Option<&Overlay>,
    depth: ColorDepth,
    cell_width: fn(&str) -> usize,
) {
    for (row, line) in lines.iter().enumerate() {
        if row >= area.height as usize {
            break;
        }
        paint_line(buffer, area, 0, row as u16, line, depth, cell_width);
    }
    if let Some(overlay) = overlay {
        for (row, line) in overlay.lines.iter().enumerate() {
            let y = overlay.top.saturating_add(row as u16);
            paint_line(buffer, area, overlay.left, y, line, depth, cell_width);
        }
    }
}

/// Repaints the rows in `rows` (the frame line, then any overlay lines on it) into a buffer that already shows the
/// rest of the screen. A repainted row starts blank, so the result is what `paint_screen` gives for the whole screen.
pub fn paint_rows(
    buffer: &mut Buffer,
    area: Rect,
    rows: &[usize],
    lines: &[Line],
    overlay: Option<&Overlay>,
    depth: ColorDepth,
    cell_width: fn(&str) -> usize,
) {
    for &row in rows {
        if row >= area.height as usize {
            continue;
        }
        let y = row as u16;
        for x in 0..area.width {
            if let Some(cell) = buffer.cell_mut((area.x + x, area.y + y)) {
                cell.reset();
            }
        }
        if let Some(line) = lines.get(row) {
            paint_line(buffer, area, 0, y, line, depth, cell_width);
        }
    }
    if let Some(overlay) = overlay {
        for (index, line) in overlay.lines.iter().enumerate() {
            let y = overlay.top.saturating_add(index as u16);
            if rows.contains(&(y as usize)) {
                paint_line(buffer, area, overlay.left, y, line, depth, cell_width);
            }
        }
    }
}

/// Plain text at the top left, for the placeholders.
pub fn paint_text(buffer: &mut Buffer, area: Rect, text: &str, cell_width: fn(&str) -> usize) {
    let line = vec![Span {
        text: text.to_string(),
        ..Span::default()
    }];
    paint_line(buffer, area, 0, 0, &line, ColorDepth::TrueColor, cell_width);
}
