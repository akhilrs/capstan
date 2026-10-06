//! Cell widths, truncation, ages and the other text helpers (`format.ts`).
use unicode_segmentation::UnicodeSegmentation;

pub use crate::model::text::clean;

use crate::model::time::parse_iso_ms;

const SPARK: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

/// The ranges the Node implementation counts as two cells, besides Extended_Pictographic.
const WIDE: &[(u32, u32)] = &[
    (0x1100, 0x115F),
    (0x2E80, 0xA4CF),
    (0xAC00, 0xD7A3),
    (0xF900, 0xFAFF),
    (0xFE30, 0xFE6F),
    (0xFF00, 0xFF60),
    (0xFFE0, 0xFFE6),
];

/// The Extended_Pictographic code points, as ranges.
const PICTOGRAPHIC: &[(u32, u32)] = &[
    (0xA9, 0xA9),
    (0xAE, 0xAE),
    (0x203C, 0x203C),
    (0x2049, 0x2049),
    (0x2122, 0x2122),
    (0x2139, 0x2139),
    (0x2194, 0x2199),
    (0x21A9, 0x21AA),
    (0x231A, 0x231B),
    (0x2328, 0x2328),
    (0x2388, 0x2388),
    (0x23CF, 0x23CF),
    (0x23E9, 0x23F3),
    (0x23F8, 0x23FA),
    (0x24C2, 0x24C2),
    (0x25AA, 0x25AB),
    (0x25B6, 0x25B6),
    (0x25C0, 0x25C0),
    (0x25FB, 0x25FE),
    (0x2600, 0x2605),
    (0x2607, 0x2612),
    (0x2614, 0x2685),
    (0x2690, 0x2705),
    (0x2708, 0x2712),
    (0x2714, 0x2714),
    (0x2716, 0x2716),
    (0x271D, 0x271D),
    (0x2721, 0x2721),
    (0x2728, 0x2728),
    (0x2733, 0x2734),
    (0x2744, 0x2744),
    (0x2747, 0x2747),
    (0x274C, 0x274C),
    (0x274E, 0x274E),
    (0x2753, 0x2755),
    (0x2757, 0x2757),
    (0x2763, 0x2767),
    (0x2795, 0x2797),
    (0x27A1, 0x27A1),
    (0x27B0, 0x27B0),
    (0x27BF, 0x27BF),
    (0x2934, 0x2935),
    (0x2B05, 0x2B07),
    (0x2B1B, 0x2B1C),
    (0x2B50, 0x2B50),
    (0x2B55, 0x2B55),
    (0x3030, 0x3030),
    (0x303D, 0x303D),
    (0x3297, 0x3297),
    (0x3299, 0x3299),
    (0x1F000, 0x1F0FF),
    (0x1F10D, 0x1F10F),
    (0x1F12F, 0x1F12F),
    (0x1F16C, 0x1F171),
    (0x1F17E, 0x1F17F),
    (0x1F18E, 0x1F18E),
    (0x1F191, 0x1F19A),
    (0x1F1AD, 0x1F1E5),
    (0x1F201, 0x1F20F),
    (0x1F21A, 0x1F21A),
    (0x1F22F, 0x1F22F),
    (0x1F232, 0x1F23A),
    (0x1F23C, 0x1F23F),
    (0x1F249, 0x1F3FA),
    (0x1F400, 0x1F53D),
    (0x1F546, 0x1F64F),
    (0x1F680, 0x1F6FF),
    (0x1F774, 0x1F77F),
    (0x1F7D5, 0x1F7FF),
    (0x1F80C, 0x1F80F),
    (0x1F848, 0x1F84F),
    (0x1F85A, 0x1F85F),
    (0x1F888, 0x1F88F),
    (0x1F8AE, 0x1F8FF),
    (0x1F90C, 0x1F93A),
    (0x1F93C, 0x1F945),
    (0x1F947, 0x1FAFF),
    (0x1FC00, 0x1FFFD),
];

fn in_ranges(ranges: &[(u32, u32)], code: u32) -> bool {
    ranges
        .binary_search_by(|&(low, high)| {
            if code < low {
                std::cmp::Ordering::Greater
            } else if code > high {
                std::cmp::Ordering::Less
            } else {
                std::cmp::Ordering::Equal
            }
        })
        .is_ok()
}

fn grapheme_width(grapheme: &str) -> usize {
    let wide = grapheme.chars().any(|c| {
        let code = c as u32;
        in_ranges(WIDE, code) || in_ranges(PICTOGRAPHIC, code)
    });
    if wide {
        2
    } else {
        1
    }
}

/// Terminal cells the text takes: wide characters count two.
pub fn cell_width(text: &str) -> usize {
    text.graphemes(true).map(grapheme_width).sum()
}

/// The text cut to `width` cells with an ellipsis, never splitting a character.
pub fn truncate(text: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    if cell_width(text) <= width {
        return text.to_string();
    }
    let mut used = 0;
    let mut out = String::new();
    for segment in text.graphemes(true) {
        let w = grapheme_width(segment);
        if used + w > width - 1 {
            break;
        }
        out.push_str(segment);
        used += w;
    }
    out.push('…');
    out
}

/// The text cut or padded on the right to exactly `width` cells.
pub fn fit(text: &str, width: usize) -> String {
    let mut cut = truncate(text, width);
    let used = cell_width(&cut);
    if used < width {
        cut.push_str(&" ".repeat(width - used));
    }
    cut
}

fn seconds_since(iso: Option<&str>, now_ms: i64) -> Option<i64> {
    let then = parse_iso_ms(iso.filter(|s| !s.is_empty())?)?;
    Some((now_ms - then).div_euclid(1000).max(0))
}

/// Seconds, minutes, hours or days since `iso`; `-` when the time is missing or unreadable.
pub fn age(iso: Option<&str>, now_ms: i64) -> String {
    let Some(seconds) = seconds_since(iso, now_ms) else {
        return "-".to_string();
    };
    if seconds < 60 {
        format!("{seconds}s")
    } else if seconds < 3600 {
        format!("{}m", seconds / 60)
    } else if seconds < 86_400 {
        format!("{}h", seconds / 3600)
    } else {
        format!("{}d", seconds / 86_400)
    }
}

pub fn commit_short(sha: Option<&str>) -> String {
    match sha {
        Some(s) if !s.is_empty() => s.chars().take(7).collect(),
        _ => "-".to_string(),
    }
}

/// One bar per sample, scaled to the largest value in the series (a flat zero series is the lowest bar).
pub fn sparkline(values: &[f64], width: usize) -> String {
    // JavaScript's `slice(-0)` keeps everything, so width 0 shows the whole series.
    let shown = if width == 0 {
        values
    } else {
        &values[values.len().saturating_sub(width)..]
    };
    let max = shown.iter().copied().fold(0.0_f64, f64::max);
    shown
        .iter()
        .map(|&v| {
            if max == 0.0 {
                SPARK[0]
            } else {
                SPARK[(((v / max) * 7.0).floor().max(0.0) as usize).min(7)]
            }
        })
        .collect()
}

/// Appends a sample and keeps the newest `limit`.
pub fn push_sample(ring: &[f64], value: f64, limit: usize) -> Vec<f64> {
    let mut out: Vec<f64> = ring.to_vec();
    out.push(value);
    let start = out.len().saturating_sub(limit);
    out.split_off(start)
}

/// Elapsed time with the next smaller unit: `41s`, `3m41s`, `14m`, `2h05m`, `3d`; `-` when the time is missing.
pub fn age_detail(iso: Option<&str>, now_ms: i64) -> String {
    match seconds_since(iso, now_ms) {
        Some(seconds) => duration_text(seconds),
        None => "-".to_string(),
    }
}

pub fn duration_text(seconds: i64) -> String {
    if seconds < 60 {
        format!("{seconds}s")
    } else if seconds < 600 {
        format!("{}m{:02}s", seconds / 60, seconds % 60)
    } else if seconds < 3600 {
        format!("{}m", seconds / 60)
    } else if seconds < 86_400 {
        format!("{}h{:02}m", seconds / 3600, (seconds % 3600) / 60)
    } else {
        format!("{}d", seconds / 86_400)
    }
}

/// The header's task summary in `width` cells: `idle` for none, `working on: a · b` while every label fits,
/// then `N tasks: a, b, +k` with as many names as fit, and an ellipsis when even that is cut. An empty string
/// for `None` (no task data).
pub fn task_summary(tasks: Option<&[crate::model::TaskEntry]>, width: usize) -> String {
    let Some(tasks) = tasks else {
        return String::new();
    };
    if width == 0 {
        return String::new();
    }
    if tasks.is_empty() {
        return truncate("idle", width);
    }
    let labels: Vec<&str> = tasks.iter().map(|t| t.label.as_str()).collect();
    let full = format!("working on: {}", labels.join(" · "));
    if tasks.len() == 1 || cell_width(&full) <= width {
        return truncate(&full, width);
    }
    let head = format!("{} tasks: ", tasks.len());
    for shown in (1..=tasks.len()).rev() {
        let mut names: Vec<String> = tasks[..shown].iter().map(|t| t.id.clone()).collect();
        let rest = tasks.len() - shown;
        if rest > 0 {
            names.push(format!("+{rest}"));
        }
        let text = format!("{head}{}", names.join(", "));
        if cell_width(&text) <= width {
            return text;
        }
    }
    truncate(
        &format!("{head}{}, +{}", tasks[0].id, tasks.len() - 1),
        width,
    )
}
