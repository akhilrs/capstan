//! Text helpers for launcher notes and refusals (src/launcher/text.ts), the screen cleaning of `cstan observe`
//! (`sanitizeScreen` of src/observe.ts) and the task text rules of src/task-text.ts.

use capstan_kernel::helpers::{is_js_space, js_trim, normalize_text, strip_terminal_sequences};
use regex::Regex;
use std::sync::LazyLock;
use unicode_segmentation::UnicodeSegmentation;

pub const OBSERVE_MAX_BYTES: usize = 8000;
const CUT_MARKER: &str = "[earlier text cut]\n";
pub const MAX_TASK_TITLE: usize = 200;

fn pattern(source: &str) -> Regex {
    Regex::new(source).expect("the pattern is valid")
}

static OSC: LazyLock<Regex> = LazyLock::new(|| {
    pattern(r"(?:\x{1b}\]|\x{9d})[^\x{07}\x{1b}\x{9c}]*(?:\x{07}|\x{1b}\\|\x{9c})?")
});
static DCS: LazyLock<Regex> = LazyLock::new(|| {
    pattern(r"(?:\x{1b}[PX^_]|[\x{90}\x{98}\x{9e}\x{9f}])[^\x{1b}\x{9c}]*(?:\x{1b}\\|\x{9c})?")
});
static CSI: LazyLock<Regex> = LazyLock::new(|| pattern(r"(?:\x{1b}\[|\x{9b})[0-?]*[ -/]*[@-~]"));
static TWO_BYTE: LazyLock<Regex> = LazyLock::new(|| pattern(r"\x{1b}[ -/]*[0-~]"));
static BLANKS: LazyLock<Regex> = LazyLock::new(|| pattern(r"[\p{Cc}\p{Cf}\p{Zl}\p{Zp}\s]+"));

/// Control and format characters, line separators and runs of blanks become one space, and the ends are trimmed.
pub fn fold_blanks(text: &str) -> String {
    js_trim(&BLANKS.replace_all(text, " ")).to_string()
}

/// The UTF-16 length of a text, as JavaScript counts it.
pub fn utf16_len(text: &str) -> usize {
    text.encode_utf16().count()
}

/// `oneLine` of src/launcher/text.ts: escape sequences (CSI with any parameter bytes, OSC, DCS and the other string forms
/// even when unterminated, the 8-bit C1 forms, and two-byte escapes), control and format characters, line separators and
/// runs of blanks are removed or become one space. The text is cut at a grapheme boundary and kept within `max_length`
/// UTF-16 units, so combining marks cannot stretch it; a first grapheme longer than that leaves nothing.
pub fn one_line(text: &str, max_length: usize) -> String {
    let step = OSC.replace_all(text, "");
    let step = DCS.replace_all(&step, "");
    let step = CSI.replace_all(&step, "");
    let step = TWO_BYTE.replace_all(&step, "");
    let step = BLANKS.replace_all(&step, " ");
    let clean = js_trim(&step);
    let mut result = String::new();
    let mut units = 0;
    for part in clean.graphemes(true) {
        let size = utf16_len(part);
        if units + size > max_length {
            break;
        }
        result.push_str(part);
        units += size;
    }
    js_trim(&result).to_string()
}

/// `tail` of src/observe.ts: the part of `text` that fits `max_bytes` UTF-8 bytes, taken from the end on a user-perceived
/// character boundary.
fn tail(text: &str, max_bytes: usize) -> String {
    let segments: Vec<&str> = text.graphemes(true).collect();
    let mut bytes = 0;
    let mut start = segments.len();
    while start > 0 {
        let size = segments[start - 1].len();
        if bytes + size > max_bytes {
            break;
        }
        bytes += size;
        start -= 1;
    }
    segments[start..].concat()
}

/// `sanitizeScreen`: plain text of a screen: sequences removed, line endings folded, control and format characters made
/// spaces (LF kept), cut from the top when long.
pub fn sanitize_screen(raw: &str) -> String {
    let clean = normalize_text(&strip_terminal_sequences(raw));
    if clean.len() <= OBSERVE_MAX_BYTES {
        return clean;
    }
    let room = OBSERVE_MAX_BYTES - CUT_MARKER.len();
    let kept = tail(&clean, room);
    let trimmed = kept.trim_start_matches(is_js_space);
    format!("{CUT_MARKER}{trimmed}")
}

static TASK_REF: LazyLock<Regex> = LazyLock::new(|| {
    pattern(r"^[A-Za-z0-9][A-Za-z0-9._:-]{0,127}(/[A-Za-z0-9][A-Za-z0-9._:-]{0,127})?$")
});
static FOLD_LINES: LazyLock<Regex> = LazyLock::new(|| pattern(r"[\t\r\n]+"));
static TITLE_CONTROL: LazyLock<Regex> = LazyLock::new(|| pattern(r"[\p{Cc}\p{Cf}\p{Zl}\p{Zp}]"));
static TITLE_SPACE: LazyLock<Regex> = LazyLock::new(|| pattern(r"\s+"));

/// `TASK_REF_PATTERN`.
pub fn is_task_ref(text: &str) -> bool {
    TASK_REF.is_match(text)
}

/// `normalizeTaskTitle`: runs of tab, CR and LF folded to one space, other control characters refused, whitespace folded,
/// cut to 200 characters. The error is the message of the `TypeError`.
pub fn normalize_task_title(raw: &str) -> Result<String, &'static str> {
    let folded = FOLD_LINES.replace_all(raw, " ");
    if TITLE_CONTROL.is_match(&folded) {
        return Err("the title must not contain control characters");
    }
    let spaced = TITLE_SPACE.replace_all(&folded, " ");
    let cut: String = js_trim(&spaced).chars().take(MAX_TASK_TITLE).collect();
    let title = js_trim(&cut);
    if title.is_empty() {
        return Err("the title must not be empty");
    }
    Ok(title.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_line_strips_sequences_and_folds_blanks() {
        assert_eq!(
            one_line("\u{1b}[31mred\u{1b}[0m  and\n\tblue\u{7}", 100),
            "red and blue"
        );
        assert_eq!(one_line("\u{1b}]0;title\u{7}after", 100), "after");
        assert_eq!(one_line("  a   b  ", 100), "a b");
    }

    #[test]
    fn one_line_cuts_on_a_grapheme_boundary_in_utf16_units() {
        assert_eq!(one_line("abcdef", 3), "abc");
        // A combining mark stays with its base; a grapheme that does not fit is dropped whole.
        assert_eq!(one_line("e\u{301}e\u{301}", 2), "e\u{301}");
        assert_eq!(one_line("\u{1f600}\u{1f600}", 2), "\u{1f600}");
        assert_eq!(one_line("\u{1f600}", 1), "");
    }

    #[test]
    fn sanitize_screen_cuts_long_screens_from_the_top() {
        let long = "x".repeat(9000);
        let out = sanitize_screen(&long);
        assert!(out.starts_with(CUT_MARKER));
        assert_eq!(out.len(), OBSERVE_MAX_BYTES);
        assert_eq!(sanitize_screen("a\u{1b}[1mb"), "ab");
    }

    #[test]
    fn task_text_rules() {
        assert!(is_task_ref("plan-1/pkg_a"));
        assert!(is_task_ref("req-1"));
        assert!(!is_task_ref("-x"));
        assert!(!is_task_ref("a/b/c"));
        assert_eq!(normalize_task_title(" a\n b ").unwrap(), "a b");
        assert!(normalize_task_title("\u{1}").is_err());
        assert!(normalize_task_title("  ").is_err());
    }
}
