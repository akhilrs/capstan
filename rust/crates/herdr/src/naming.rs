//! Project slug and Herdr agent names (src/herdr/naming.ts).

use unicode_normalization::UnicodeNormalization;

/// Herdr's own rule for an agent name: it must be unique among the live agents of one server.
/// `/^[a-z][a-z0-9_-]{0,31}$/`.
pub fn is_herdr_agent_name(value: &str) -> bool {
    let bytes = value.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 32
        && bytes[0].is_ascii_lowercase()
        && bytes[1..]
            .iter()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'_' | b'-'))
}

pub const MAX_SLUG_CHARS: usize = 10;
const MAX_LABEL_NAME_CHARS: usize = 24;
const MAX_LABEL_CHARS: usize = 64;
/// Roles may be named so that `<slug>-<role>-<up to 4 digits>` still fits Herdr's 32 characters.
pub const MAX_ROLE_NAME_CHARS: usize = 32 - MAX_SLUG_CHARS - 1 - 1 - 4;

/// `String.prototype.trim`.
fn js_trim(text: &str) -> &str {
    text.trim_matches(is_js_space)
}

/// The characters `String.prototype.trim` and `\s` treat as space.
pub(crate) fn is_js_space(c: char) -> bool {
    matches!(
        c,
        '\u{9}'..='\u{d}'
            | ' '
            | '\u{a0}'
            | '\u{1680}'
            | '\u{2000}'..='\u{200a}'
            | '\u{2028}'
            | '\u{2029}'
            | '\u{202f}'
            | '\u{205f}'
            | '\u{3000}'
            | '\u{feff}'
    )
}

/// The project's display name: `[project] name` when set, else the base name of its directory.
pub fn project_display_name(configured: Option<&str>, project_root: &str) -> String {
    let name = js_trim(configured.unwrap_or(""));
    // path.basename: the last non-empty segment ("" for "/").
    // path.basename: the last segment, trailing slashes ignored ("" for "/").
    let base = || {
        project_root
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .unwrap_or("")
            .to_string()
    };
    let chosen = if name.is_empty() {
        base()
    } else {
        name.to_string()
    };
    let clean: String = chosen
        .chars()
        .filter(|c| {
            let code = *c as u32;
            code >= 0x20 && !(0x7f..=0x9f).contains(&code)
        })
        .collect();
    let clean = js_trim(&clean);
    let cut: String = clean.chars().take(MAX_LABEL_NAME_CHARS).collect();
    let cut = js_trim(&cut);
    if cut.is_empty() {
        "capstan".to_string()
    } else {
        cut.to_string()
    }
}

/// Lower case, runs of anything outside a-z0-9 become one `-`, a leading digit gets a `p`, at most ten characters.
pub fn project_slug(display_name: &str) -> String {
    // `.normalize("NFKD").replace(/\p{M}+/gu, "").toLowerCase()`
    let decomposed: String = display_name.nfkd().collect();
    let without_marks: String = decomposed.chars().filter(|c| !is_mark(*c)).collect();
    let lower = js_lower(&without_marks);
    let mut dashed = String::new();
    let mut in_run = false;
    for c in lower.chars() {
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            dashed.push(c);
            in_run = false;
        } else if !in_run {
            dashed.push('-');
            in_run = true;
        }
    }
    let base = dashed.trim_matches('-');
    let lettered = if base.starts_with(|c: char| c.is_ascii_lowercase()) {
        base.to_string()
    } else {
        format!("p{base}")
    };
    // `.slice(0, 10)`: UTF-16 units, but the text is ASCII by now.
    let cut: String = lettered.chars().take(MAX_SLUG_CHARS).collect();
    let trimmed = cut.trim_end_matches('-');
    if trimmed.is_empty() {
        "p".to_string()
    } else {
        trimmed.to_string()
    }
}

/// `\p{M}`: the general categories Mn, Mc and Me.
fn is_mark(c: char) -> bool {
    MARK.is_match(c.encode_utf8(&mut [0u8; 4]))
}

static MARK: std::sync::LazyLock<regex::Regex> =
    std::sync::LazyLock::new(|| regex::Regex::new(r"^\p{M}$").expect("the pattern is valid"));

/// `String.prototype.toLowerCase` for the characters that matter here (full Unicode lowercasing).
fn js_lower(text: &str) -> String {
    text.to_lowercase()
}

/// The name Herdr sees for a ledger agent id.
pub fn herdr_agent_name(slug: &str, agent_id: &str) -> Result<String, String> {
    let name = format!("{slug}-{agent_id}");
    if !is_herdr_agent_name(&name) {
        return Err(format!(
            "the agent name {name} does not fit Herdr's rule (a lower-case letter first, then a-z, 0-9, _ or -, at most 32 characters)"
        ));
    }
    Ok(name)
}

/// `<project> · <part>`, cut so the whole label stays within the limit.
pub fn workspace_label(display_name: &str, part: &str) -> String {
    let label = format!("{display_name} · {part}");
    label.chars().take(MAX_LABEL_CHARS).collect()
}
