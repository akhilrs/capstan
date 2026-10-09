//! Screen parsing: input line, prompts, dialogs (src/herdr/screen.ts).
//!
//! The Node code is regex-heavy and its regexes mean JS things: `\s` is the JS white space set, `.` stops at four line
//! terminators, `\d` is ASCII, `\b` and `i` are ASCII in a non-`u` regex. Where those differ from the `regex` crate the
//! matches are written out by hand; the one escape-sequence pattern is the same text in both and uses no class that
//! differs.

use crate::adapter::validate::{js_trim, js_trim_end, utf16_len};
use crate::api::{InputBlocker, RelayOption, RELAY_PROMPT_MAX_BYTES};
use crate::naming::is_js_space;
use regex::Regex;
use std::sync::LazyLock;

const ESCAPE_SEQUENCE: &str = r"(?:\x1b(?:\][^\x07\x1b]*(?:\x07|\x1b\\)|\[[0-?]*[ -/]*[@-~]|[ -/]*[0-Z\\^-~])|\x{9b}[0-?]*[ -/]*[@-~])";

static ESCAPE_SEQUENCES: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(ESCAPE_SEQUENCE).expect("the pattern is valid"));
static ESCAPE_AT_START: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(&format!("^{ESCAPE_SEQUENCE}")).expect("the pattern is valid"));
static SGR_SEQUENCES: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\x1b\[([0-9;:]*)m").expect("the pattern is valid"));
static WIDENING_HEURISTIC: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i-u)\b(?:always allow|don'?t ask again|do not ask again|switch to|auto mode|accept edits|for this session|from this project|permanently|allow all)\b",
    )
    .expect("the pattern is valid")
});

const PROMPT_SYMBOLS: [&str; 4] = ["❯", "$", "#", "%"];

pub const TRUST_YES: &str = "Yes, I trust this folder";
pub const TRUST_NO: &str = "No, exit";
pub const CODEX_TRUST_YES: &str = "Trust and continue";
pub const CODEX_TRUST_NO: &str = "Quit";

const NBSP: char = '\u{a0}';

/// `/[\u0000-\u0008\u000b\u000c\u000e-\u001f\u007f-\u009f]/`.
fn leftover_control(text: &str) -> bool {
    text.chars().any(|c| {
        matches!(c, '\u{0}'..='\u{8}' | '\u{b}' | '\u{c}' | '\u{e}'..='\u{1f}' | '\u{7f}'..='\u{9f}')
    })
}

/// `/^─{10,}$/`.
fn is_rule_line(text: &str) -> bool {
    text.chars().count() >= 10 && text.chars().all(|c| c == '─')
}

/// The four characters `.` does not match.
fn is_line_terminator(c: char) -> bool {
    matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}')
}

/// The two option texts of a host's trust dialog, or None for a host that shows none.
pub fn trust_texts(kind: &str) -> Option<(&'static str, &'static str)> {
    match kind {
        "claude" => Some((TRUST_YES, TRUST_NO)),
        "codex" => Some((CODEX_TRUST_YES, CODEX_TRUST_NO)),
        _ => None,
    }
}

pub fn parse_trust_dialog_of(kind: &str, plain_screen: &str) -> Option<TrustDialog> {
    match kind {
        "claude" => parse_trust_dialog(plain_screen),
        "codex" => parse_codex_trust_dialog(plain_screen),
        _ => None,
    }
}

pub fn strip_ansi(text: &str) -> String {
    ESCAPE_SEQUENCES.replace_all(text, "").into_owned()
}

/// `text.split(/\r\n|\n|\r/)`.
pub fn split_lines(text: &str) -> Vec<&str> {
    let mut lines = Vec::new();
    let bytes = text.as_bytes();
    let mut start = 0;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\r' => {
                lines.push(&text[start..i]);
                i += if bytes.get(i + 1) == Some(&b'\n') {
                    2
                } else {
                    1
                };
                start = i;
            }
            b'\n' => {
                lines.push(&text[start..i]);
                i += 1;
                start = i;
            }
            _ => i += 1,
        }
    }
    lines.push(&text[start..]);
    lines
}

/// `Number(text)` for text without surrounding white space the way an SGR parameter or a `/proc` field is read: NaN
/// when it is not a number.
pub fn js_number(text: &str) -> f64 {
    let t = js_trim(text);
    if t.is_empty() {
        return 0.0;
    }
    let radix = |digits: &str, radix: u32| -> f64 {
        if digits.is_empty() {
            return f64::NAN;
        }
        let mut value = 0f64;
        for c in digits.chars() {
            match c.to_digit(radix) {
                Some(d) => value = value * f64::from(radix) + f64::from(d),
                None => return f64::NAN,
            }
        }
        value
    };
    if let Some(rest) = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
        return radix(rest, 16);
    }
    if let Some(rest) = t.strip_prefix("0o").or_else(|| t.strip_prefix("0O")) {
        return radix(rest, 8);
    }
    if let Some(rest) = t.strip_prefix("0b").or_else(|| t.strip_prefix("0B")) {
        return radix(rest, 2);
    }
    let (sign, body) = match t.as_bytes()[0] {
        b'+' => (1.0, &t[1..]),
        b'-' => (-1.0, &t[1..]),
        _ => (1.0, t),
    };
    if body == "Infinity" {
        return sign * f64::INFINITY;
    }
    // StrDecimalLiteral: digits [. digits] | . digits, then an optional exponent.
    let b = body.as_bytes();
    let mut i = 0;
    let digits = |i: &mut usize| {
        let from = *i;
        while *i < b.len() && b[*i].is_ascii_digit() {
            *i += 1;
        }
        *i - from
    };
    let int = digits(&mut i);
    let mut frac = 0;
    if i < b.len() && b[i] == b'.' {
        i += 1;
        frac = digits(&mut i);
    }
    if int == 0 && frac == 0 {
        return f64::NAN;
    }
    if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
        i += 1;
        if i < b.len() && (b[i] == b'+' || b[i] == b'-') {
            i += 1;
        }
        if digits(&mut i) == 0 {
            return f64::NAN;
        }
    }
    if i != b.len() {
        return f64::NAN;
    }
    body.parse::<f64>().map_or(f64::NAN, |v| sign * v)
}

#[derive(Clone, Debug)]
struct StyledCharacter {
    character: char,
    dim: bool,
}

/// What an SGR parameter list does to the dim attribute, and where a colour starts: runs `visit` on each parameter code
/// with the cursor skipping the sub-parameters of an extended colour. Returns true when `visit` stops the walk.
fn walk_sgr(params: &str, mut visit: impl FnMut(f64) -> bool) {
    let params: Vec<&str> = params.split(';').collect();
    let mut cursor = 0;
    while cursor < params.len() {
        let code = if params[cursor].is_empty() {
            0.0
        } else {
            js_number(params[cursor])
        };
        if visit(code) {
            return;
        }
        if code == 38.0 || code == 48.0 || code == 58.0 {
            cursor += match params.get(cursor + 1) {
                Some(&"5") => 2,
                Some(&"2") => 4,
                _ => 0,
            };
        }
        cursor += 1;
    }
}

/// Visible characters of one raw line with the dim attribute the terminal would draw them with.
fn styled_characters(line: &str) -> Vec<StyledCharacter> {
    let mut result = Vec::new();
    let mut dim = false;
    let mut index = 0;
    while index < line.len() {
        let rest = &line[index..];
        let first = rest.chars().next().expect("not at the end");
        if first == '\u{1b}' || first == '\u{9b}' {
            if let Some(sequence) = ESCAPE_AT_START.find(rest) {
                let text = sequence.as_str();
                if let Some(params) = text
                    .strip_prefix("\u{1b}[")
                    .and_then(|r| r.strip_suffix('m'))
                    .filter(|p| {
                        p.chars()
                            .all(|c| c.is_ascii_digit() || c == ';' || c == ':')
                    })
                {
                    walk_sgr(params, |code| {
                        if code == 0.0 || code == 22.0 {
                            dim = false;
                        } else if code == 2.0 {
                            dim = true;
                        }
                        false
                    });
                }
                index += text.len();
                continue;
            }
        }
        result.push(StyledCharacter {
            character: first,
            dim,
        });
        index += first.len_utf8();
    }
    result
}

fn is_blank(character: char) -> bool {
    is_js_space(character) || character == NBSP
}

/// `[firstText, ...]`'s first element: the typed text after the marker, "" for a dim placeholder or nothing.
fn first_text(remainder: &[StyledCharacter]) -> String {
    let typed: Vec<&StyledCharacter> = remainder
        .iter()
        .filter(|e| !is_blank(e.character))
        .collect();
    let placeholder = !typed.is_empty() && typed.iter().all(|e| e.dim);
    if placeholder || typed.is_empty() {
        String::new()
    } else {
        let joined: String = remainder.iter().map(|e| e.character).collect();
        js_trim_end(&joined).to_string()
    }
}

/// The match of `^MARKER(?:[ NBSP](.*))?$` on a trimmed line: the text after the marker, or None.
fn after_marker<'a>(line: &'a str, marker: &str, separators: &[char]) -> Option<&'a str> {
    let rest = line.strip_prefix(marker)?;
    if rest.is_empty() {
        return Some("");
    }
    let mut chars = rest.chars();
    let sep = chars.next()?;
    if !separators.contains(&sep) {
        return None;
    }
    let text = chars.as_str();
    if text.chars().any(is_line_terminator) {
        return None;
    }
    Some(text)
}

/// The text typed on the input line of an agent or shell, "" when it is empty, or None when the screen does not show an
/// input line that can be read. The screen must be an ANSI read, because an empty Claude input shows a dim placeholder
/// that a plain read cannot tell from typed text.
pub fn extract_input_line(kind: &str, ansi_screen: &str) -> Option<String> {
    let raw = split_lines(ansi_screen);
    let plain: Vec<String> = raw.iter().map(|l| strip_ansi(l)).collect();
    match kind {
        "shell" => {
            let last = last_non_blank(&plain)?;
            if leftover_control(&plain[last]) {
                return None;
            }
            let line = js_trim_end(&plain[last]);
            after_marker(line, "❯", &[' ', NBSP]).map(|text| js_trim_end(text).to_string())
        }
        "codex" => codex_input_line(&raw, &plain),
        "omp" => omp_input_line(&plain),
        "claude" => claude_input_line(&raw, &plain),
        _ => None,
    }
}

fn last_non_blank(plain: &[String]) -> Option<usize> {
    plain.iter().rposition(|line| !js_trim(line).is_empty())
}

fn claude_input_line(raw: &[&str], plain: &[String]) -> Option<String> {
    let rules: Vec<usize> = plain
        .iter()
        .enumerate()
        .filter(|(_, line)| is_rule_line(js_trim(line)))
        .map(|(index, _)| index)
        .collect();
    if rules.len() < 2 {
        return None;
    }
    let bottom = rules[rules.len() - 1];
    let top = rules[rules.len() - 2];
    if bottom - top < 2 {
        return None;
    }
    if utf16_len(js_trim(&plain[top])) != utf16_len(js_trim(&plain[bottom])) {
        return None;
    }
    if plain[top..=bottom]
        .iter()
        .any(|line| leftover_control(line))
    {
        return None;
    }
    let first = &plain[top + 1];
    // `/^\s*❯(?:[ NBSP]|$)/u`
    let after = first.trim_start_matches(is_js_space).strip_prefix('❯')?;
    if !(after.is_empty() || after.starts_with(' ') || after.starts_with(NBSP)) {
        return None;
    }
    let characters = styled_characters(raw[top + 1]);
    let marker = characters.iter().position(|e| e.character == '❯');
    // Node: findIndex is -1 when absent, so `start` is 0; the plain line has the marker, the raw one always does too.
    let mut start = marker.map_or(0, |m| m + 1);
    if matches!(characters.get(start), Some(e) if e.character == ' ' || e.character == NBSP) {
        start += 1;
    }
    let first_text = first_text(&characters[start.min(characters.len())..]);
    let mut parts = vec![first_text];
    for (offset, line) in raw[top + 2..bottom].iter().enumerate() {
        let visible: Vec<StyledCharacter> = styled_characters(line)
            .into_iter()
            .filter(|e| !is_blank(e.character))
            .collect();
        if !visible.is_empty() && visible.iter().all(|e| e.dim) {
            parts.push(String::new());
        } else {
            parts.push(js_trim_end(&plain[top + 2 + offset]).to_string());
        }
    }
    Some(js_trim_end(&parts.join("\n")).to_string())
}

/// True when the line sets a foreground colour (SGR 38), not a background or underline colour that takes the same
/// sub-parameters.
fn sets_foreground(line: &str) -> bool {
    for sequence in SGR_SEQUENCES.captures_iter(line) {
        let mut found = false;
        walk_sgr(&sequence[1], |code| {
            if code == 38.0 {
                found = true;
            }
            found
        });
        if found {
            return true;
        }
    }
    false
}

/// Codex draws the input as `› text` (a dim placeholder when empty), then any further typed lines, then a status line
/// that starts with a coloured span.
fn codex_input_line(raw: &[&str], plain: &[String]) -> Option<String> {
    if plain
        .iter()
        .any(|line| js_trim(line).starts_with("enter continue"))
    {
        return None;
    }
    let marker = plain.iter().rposition(|line| {
        let t = js_trim_end(line);
        t == "›" || t.starts_with("› ")
    })?;
    let status = (marker + 1..raw.len()).find(|&index| sets_foreground(raw[index]))?;
    if plain[marker..status]
        .iter()
        .any(|line| leftover_control(line))
    {
        return None;
    }
    let characters = styled_characters(raw[marker]);
    let at = characters.iter().position(|e| e.character == '›')?;
    let remainder = &characters[(at + 2).min(characters.len())..];
    let first = first_text(remainder);
    let mut parts = vec![first];
    for line in &plain[marker + 1..status] {
        let stripped = line.strip_prefix("  ").unwrap_or(line);
        parts.push(js_trim_end(stripped).to_string());
    }
    Some(js_trim_end(&parts.join("\n")).to_string())
}

/// OMP's input is the last line, `╰─ text`; nothing is drawn when it is empty.
fn omp_input_line(plain: &[String]) -> Option<String> {
    let last = last_non_blank(plain)?;
    if leftover_control(&plain[last]) {
        return None;
    }
    let line = js_trim_end(&plain[last]);
    let rest = line.strip_prefix("╰─")?;
    if rest.is_empty() {
        return Some(String::new());
    }
    let text = rest.strip_prefix(' ')?;
    if text.chars().any(is_line_terminator) {
        return None;
    }
    Some(js_trim_end(text).to_string())
}

/// A pane the adapter has just created shows the operator's own prompt: only a last line that is exactly one prompt
/// symbol proves that nothing is typed.
pub fn fresh_prompt_ready(plain_screen: &str) -> bool {
    let lines = split_lines(plain_screen);
    let Some(last) = lines.iter().rposition(|line| !js_trim(line).is_empty()) else {
        return false;
    };
    !leftover_control(lines[last]) && PROMPT_SYMBOLS.contains(&js_trim(lines[last]))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrustOption {
    pub text: String,
    pub selected: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TrustDialog {
    Dialog {
        path: String,
        options: Vec<TrustOption>,
        selected_index: Option<usize>,
        confirm_is_last_line: bool,
    },
    WrappedPath,
}

/// The `dialog` result both parsers end with.
fn dialog_of(
    lines: &[&str],
    path_index: usize,
    confirm: usize,
    options: Vec<TrustOption>,
) -> TrustDialog {
    let selected: Vec<usize> = options
        .iter()
        .enumerate()
        .filter(|(_, o)| o.selected)
        .map(|(i, _)| i)
        .collect();
    let last_non_empty = lines.iter().rposition(|l| !js_trim(l).is_empty());
    TrustDialog::Dialog {
        path: js_trim(lines[path_index]).to_string(),
        options,
        selected_index: if selected.len() == 1 {
            Some(selected[0])
        } else {
            None
        },
        confirm_is_last_line: last_non_empty == Some(confirm),
    }
}

/// Finds the title line, the path line after it and whether the path wrapped. `Err(None)` is "no dialog", `Err(Some)`
/// the wrapped-path answer.
fn locate_path(lines: &[&str], title_text: &str) -> Result<usize, Option<TrustDialog>> {
    let Some(title) = lines.iter().rposition(|l| js_trim(l) == title_text) else {
        return Err(None);
    };
    let mut path_index = title + 1;
    while path_index < lines.len() && js_trim(lines[path_index]).is_empty() {
        path_index += 1;
    }
    if path_index >= lines.len() {
        return Err(None);
    }
    if matches!(lines.get(path_index + 1), Some(next) if !js_trim(next).is_empty()) {
        return Err(Some(TrustDialog::WrappedPath));
    }
    Ok(path_index)
}

pub fn parse_trust_dialog(plain_screen: &str) -> Option<TrustDialog> {
    let lines = split_lines(plain_screen);
    let path_index = match locate_path(&lines, "Accessing workspace:") {
        Ok(index) => index,
        Err(answer) => return answer,
    };
    let confirm = lines
        .iter()
        .enumerate()
        .rev()
        .find(|(index, line)| *index > path_index && js_trim(line).starts_with("Enter to confirm"))
        .map(|(index, _)| index)?;
    let mut cursor = confirm as isize - 1;
    while cursor > path_index as isize && js_trim(lines[cursor as usize]).is_empty() {
        cursor -= 1;
    }
    let mut options: Vec<TrustOption> = Vec::new();
    while cursor > path_index as isize && !js_trim(lines[cursor as usize]).is_empty() {
        let trimmed = js_trim(lines[cursor as usize]);
        let selected = trimmed.starts_with('❯');
        let text = if selected {
            trimmed['❯'.len_utf8()..].trim_start_matches(is_js_space)
        } else {
            trimmed
        };
        options.insert(
            0,
            TrustOption {
                text: text.to_string(),
                selected,
            },
        );
        cursor -= 1;
    }
    Some(dialog_of(&lines, path_index, confirm, options))
}

/// `/^(›\s*)?\d+\.\s+(.+)$/u` on a trimmed line: the option text and whether the `›` marker was there.
fn codex_option(trimmed: &str) -> Option<(String, bool)> {
    let (selected, rest) = match trimmed.strip_prefix('›') {
        Some(after) => (true, after.trim_start_matches(is_js_space)),
        None => (false, trimmed),
    };
    let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
    if digits == 0 {
        return None;
    }
    let after_dot = rest[digits..].strip_prefix('.')?;
    let text = after_dot.trim_start_matches(is_js_space);
    if text.len() == after_dot.len() || text.is_empty() || text.chars().any(is_line_terminator) {
        return None;
    }
    Some((text.to_string(), selected))
}

/// Codex: `Folder access`, the path, a question, `› 1. Trust and continue`, `2. Quit`, `enter continue · esc quit`.
pub fn parse_codex_trust_dialog(plain_screen: &str) -> Option<TrustDialog> {
    let lines = split_lines(plain_screen);
    let path_index = match locate_path(&lines, "Folder access") {
        Ok(index) => index,
        Err(answer) => return answer,
    };
    let confirm = lines
        .iter()
        .enumerate()
        .rev()
        .find(|(index, line)| *index > path_index && js_trim(line).starts_with("enter continue"))
        .map(|(index, _)| index)?;
    let mut options = Vec::new();
    for line in &lines[path_index + 1..confirm] {
        if let Some((text, selected)) = codex_option(js_trim(line)) {
            options.push(TrustOption { text, selected });
        }
    }
    Some(dialog_of(&lines, path_index, confirm, options))
}

/// Option wording a recorded Claude Code 2.1.288 capture shows, with what it does.
const PROVEN_SAFE_OPTIONS: [&str; 4] = [
    "Yes",
    "No",
    "No, and tell Claude what to do differently",
    "Yes, and tell Claude what to do next",
];
const PROVEN_WIDENING_PREFIXES: [&str; 3] = [
    "Yes, and always allow access to ",
    "Yes, and switch to auto mode",
    "Yes, and switch to accept edits",
];

/// What each text-capable option reads as once Tab has opened its field, as recorded.
pub fn text_field_wording(original: &str) -> Option<&'static str> {
    match original {
        "No" => Some("No, and tell Claude what to do differently"),
        "Yes" => Some("Yes, and tell Claude what to do next"),
        _ => None,
    }
}

/// The footer line of a Claude permission dialog, trimmed, or None when the last line is something else.
pub fn prompt_footer(ansi_screen: &str) -> Option<String> {
    let lines: Vec<String> = split_lines(ansi_screen)
        .into_iter()
        .map(strip_ansi)
        .collect();
    let footer = match last_non_blank(&lines) {
        Some(last) => js_trim(&lines[last]),
        None => "",
    };
    footer
        .starts_with("Esc to cancel")
        .then(|| footer.to_string())
}

fn widens_permissions(text: &str) -> bool {
    if PROVEN_SAFE_OPTIONS.contains(&text) {
        return false;
    }
    if PROVEN_WIDENING_PREFIXES.iter().any(|p| text.starts_with(p)) {
        return true;
    }
    WIDENING_HEURISTIC.is_match(text)
}

/// A Tab opens an inline text field only on these proven option wordings: the Bash prompt's `No` and the Write prompt's
/// `Yes`.
fn accepts_text(header: &str, question: &str, text: &str) -> bool {
    if header == "Bash command" && question == "Do you want to proceed?" {
        return text == "No";
    }
    // `/^Do you want to create .+\?$/`
    if let Some(rest) = question.strip_prefix("Do you want to create ") {
        if let Some(middle) = rest.strip_suffix('?') {
            if !middle.is_empty() && !middle.chars().any(is_line_terminator) {
                return text == "Yes";
            }
        }
    }
    false
}

/// `/^ (❯| ) (\d+)\. (\S.*)$/u`: the marker, the number and the text.
fn option_line(line: &str) -> Option<(bool, u64, &str)> {
    let rest = line.strip_prefix(' ')?;
    let (marked, rest) = if let Some(r) = rest.strip_prefix('❯') {
        (true, r)
    } else {
        (false, rest.strip_prefix(' ')?)
    };
    let rest = rest.strip_prefix(' ')?;
    let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
    if digits == 0 {
        return None;
    }
    let number = rest[..digits].parse::<u64>().unwrap_or(u64::MAX);
    let text = rest[digits..].strip_prefix(". ")?;
    let first = text.chars().next()?;
    if is_js_space(first) || text.chars().any(is_line_terminator) {
        return None;
    }
    Some((marked, number, text))
}

/// Claude soft-wraps a long option (one naming a long path) onto a line with a six-space hanging indent:
/// `/^ {6}(\S.*)$/u`.
fn option_continuation(line: &str) -> Option<&str> {
    let text = line.strip_prefix("      ")?;
    let first = text.chars().next()?;
    if is_js_space(first) || text.chars().any(is_line_terminator) {
        return None;
    }
    Some(text)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostPrompt {
    pub text: String,
    pub options: Vec<RelayOption>,
    pub selected_index: usize,
}

/// `/^ Esc to cancel(?: · Tab to amend)?$/u`.
fn is_prompt_footer(line: &str) -> bool {
    line == " Esc to cancel" || line == " Esc to cancel · Tab to amend"
}

/// Claude Code's permission dialog on a read of the pane: a rule line, the header and detail, the question, numbered
/// options with one `❯` marker and an `Esc to cancel` footer that is the last line. Any other layout (a dialog that is
/// not last, two markers, an option wrapped by a bare line break, control characters, text over the size limit) is
/// None, as is every host but claude.
pub fn parse_host_prompt(kind: &str, ansi_screen: &str) -> Option<HostPrompt> {
    if kind != "claude" {
        return None;
    }
    let lines: Vec<String> = split_lines(ansi_screen)
        .into_iter()
        .map(strip_ansi)
        .collect();
    let last = last_non_blank(&lines)?;
    if lines.iter().any(|line| leftover_control(line)) {
        return None;
    }
    if !is_prompt_footer(js_trim_end(&lines[last])) {
        return None;
    }
    let mut cursor = last as isize - 1;
    if cursor >= 0 && js_trim(&lines[cursor as usize]).is_empty() {
        cursor -= 1;
    }
    struct Row {
        marked: bool,
        number: u64,
        text: String,
    }
    let mut rows: Vec<Row> = Vec::new();
    let mut continuation: Vec<String> = Vec::new();
    while cursor >= 0 {
        let line = js_trim_end(&lines[cursor as usize]);
        match option_line(line) {
            None => match option_continuation(line) {
                None => break,
                Some(wrapped) => {
                    continuation.insert(0, wrapped.to_string());
                    cursor -= 1;
                }
            },
            Some((marked, number, text)) => {
                let mut pieces = vec![text.to_string()];
                pieces.append(&mut continuation);
                rows.insert(
                    0,
                    Row {
                        marked,
                        number,
                        text: pieces.join(" "),
                    },
                );
                cursor -= 1;
            }
        }
    }
    if !continuation.is_empty() {
        return None;
    }
    if rows.len() < 2 {
        return None;
    }
    if rows
        .iter()
        .enumerate()
        .any(|(index, row)| row.number != index as u64 + 1)
    {
        return None;
    }
    let markers: Vec<usize> = rows
        .iter()
        .enumerate()
        .filter(|(_, r)| r.marked)
        .map(|(i, _)| i)
        .collect();
    if markers.len() != 1 {
        return None;
    }
    let question = if cursor >= 0 {
        js_trim(&lines[cursor as usize])
    } else {
        ""
    };
    if !question.ends_with('?') {
        return None;
    }
    let mut rule = cursor - 1;
    while rule >= 0 && !is_rule_line(js_trim(&lines[rule as usize])) {
        let next_blank = lines
            .get((rule + 1) as usize)
            .is_some_and(|l| js_trim(l).is_empty());
        if js_trim(&lines[rule as usize]).is_empty() && next_blank {
            return None;
        }
        rule -= 1;
    }
    if rule < 0 {
        return None;
    }
    let body: Vec<&str> = lines[(rule + 1) as usize..=cursor as usize]
        .iter()
        .map(|l| js_trim_end(l))
        .collect();
    let header = js_trim(body.first().copied().unwrap_or(""));
    if header.is_empty() {
        return None;
    }
    let text = body.join("\n");
    if text.len() > RELAY_PROMPT_MAX_BYTES {
        return None;
    }
    Some(HostPrompt {
        text,
        selected_index: markers[0],
        options: rows
            .iter()
            .map(|row| RelayOption {
                number: row.number as i64,
                text: row.text.clone(),
                accepts_text: accepts_text(header, question, &row.text),
                widens_permissions: widens_permissions(&row.text),
            })
            .collect(),
    })
}

/// `/^\s*(?:❯\s*)?\d+\.\s/u`
fn numbered_row(line: &str) -> bool {
    let rest = line.trim_start_matches(is_js_space);
    let rest = match rest.strip_prefix('❯') {
        Some(after) => after.trim_start_matches(is_js_space),
        None => rest,
    };
    let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
    digits > 0
        && rest[digits..]
            .strip_prefix('.')
            .and_then(|r| r.chars().next())
            .is_some_and(is_js_space)
}

/// A Claude dialog that is not a permission prompt and shows no input box (for example a settings or onboarding dialog)
/// but whose last non-empty line is a footer that offers `Esc to cancel`. Only its text is returned; the only key that
/// may ever be sent to it is Esc. Anything else is None.
pub fn parse_blocking_dialog(kind: &str, ansi_screen: &str) -> Option<String> {
    if kind != "claude" {
        return None;
    }
    if parse_host_prompt(kind, ansi_screen).is_some() {
        return None;
    }
    if extract_input_line(kind, ansi_screen).is_some() {
        return None;
    }
    let lines: Vec<String> = split_lines(ansi_screen)
        .into_iter()
        .map(strip_ansi)
        .collect();
    let last = last_non_blank(&lines)?;
    if lines.iter().any(|line| leftover_control(line)) {
        return None;
    }
    if !lines[last].contains("Esc to cancel") {
        return None;
    }
    // Numbered rows mean a permission prompt that did not parse; it is never relayed as a dialog.
    if lines.iter().any(|line| numbered_row(line)) {
        return None;
    }
    let mut rule = last as isize - 1;
    while rule >= 0 && !is_rule_line(js_trim(&lines[rule as usize])) {
        rule -= 1;
    }
    let mut body: Vec<&str> = lines[(rule + 1) as usize..=last]
        .iter()
        .map(|l| js_trim_end(l))
        .collect();
    while body.first().is_some_and(|l| js_trim(l).is_empty()) {
        body.remove(0);
    }
    while body.last().is_some_and(|l| js_trim(l).is_empty()) {
        body.pop();
    }
    let text = body.join("\n");
    if text.len() > RELAY_PROMPT_MAX_BYTES {
        return None;
    }
    Some(text)
}

/// What keeps an unreadable input line from being read, from one ANSI screen.
pub fn classify_input_blocker(kind: &str, ansi_screen: &str) -> InputBlocker {
    if parse_host_prompt(kind, ansi_screen).is_some() {
        return InputBlocker::PermissionPrompt;
    }
    if parse_blocking_dialog(kind, ansi_screen).is_some() {
        return InputBlocker::Dialog;
    }
    InputBlocker::Unknown
}
