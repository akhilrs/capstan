//! How a checked value is shown to the user in a change list: cut at 160 characters, control characters escaped, and
//! the parts that look like secrets replaced by `<hidden>`. Redaction is a heuristic; a hidden part still counts in the
//! digest, so a change to it is listed.
use serde_json::Value;

use crate::text::{is_unsafe_char, looks_like_credential};

const HIDDEN: &str = "<hidden>";
const MAX_CHARS: usize = 160;
const SECRET_WORDS: [&str; 7] = [
    "token", "password", "passwd", "secret", "key", "auth", "bearer",
];
const SCHEMES: [&str; 4] = ["bearer", "basic", "token", "digest"];

/// The text that stands for `value` in a change list.
pub(crate) fn display_value(value: &Value) -> String {
    let raw = render(value, &mut Redactor::default(), false);
    finish(&raw)
}

fn sensitive_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    SECRET_WORDS.iter().any(|word| lower.contains(word))
}

fn render(value: &Value, redactor: &mut Redactor, quoted: bool) -> String {
    match value {
        Value::String(text) => {
            let shown = redactor.text(text, quoted);
            if quoted {
                Value::String(shown).to_string()
            } else {
                shown
            }
        }
        Value::Array(items) => {
            // One redactor for the whole array: `["--token", "abc"]` is an argument list.
            let mut inner = Redactor::default();
            let parts: Vec<String> = items
                .iter()
                .map(|item| render(item, &mut inner, true))
                .collect();
            format!("[{}]", parts.join(", "))
        }
        Value::Object(members) => {
            let parts: Vec<String> = members
                .iter()
                .map(|(key, member)| {
                    let shown = if sensitive_name(key) {
                        HIDDEN.to_string()
                    } else {
                        render(member, &mut Redactor::default(), true)
                    };
                    format!("{}: {shown}", Value::String(key.clone()))
                })
                .collect();
            format!("{{{}}}", parts.join(", "))
        }
        other => other.to_string(),
    }
}

/// State carried from one word to the next: a flag that names a secret hides the word after it, and an
/// `Authorization:` header hides its scheme and credential.
#[derive(Default)]
struct Redactor {
    hide_next: bool,
    auth_words: u8,
}

impl Redactor {
    fn text(&mut self, text: &str, whole_element: bool) -> String {
        if whole_element && self.hide_next && !text.starts_with('-') {
            self.hide_next = false;
            return HIDDEN.to_string();
        }
        let shown = self.words(text);
        if whole_element && looks_like_credential(text) && !shown.contains(HIDDEN) {
            return HIDDEN.to_string();
        }
        if !whole_element
            && shown.contains('\n')
            && looks_like_credential(text)
            && !shown.contains(HIDDEN)
        {
            return HIDDEN.to_string();
        }
        shown
    }

    fn words(&mut self, text: &str) -> String {
        let mut out = String::new();
        let mut word = String::new();
        for c in text.chars() {
            if c.is_whitespace() {
                if !word.is_empty() {
                    out.push_str(&self.word(&word));
                    word.clear();
                }
                out.push(c);
            } else {
                word.push(c);
            }
        }
        if !word.is_empty() {
            out.push_str(&self.word(&word));
        }
        out
    }

    fn word(&mut self, word: &str) -> String {
        let core = word.trim_matches(|c| matches!(c, '"' | '\'' | ','));
        let is_flag = core.starts_with('-') && core.len() > 1;
        if self.hide_next {
            self.hide_next = false;
            if !is_flag {
                return HIDDEN.to_string();
            }
        }
        if self.auth_words > 0 {
            let scheme = SCHEMES.contains(&core.to_ascii_lowercase().as_str());
            self.auth_words = if self.auth_words == 2 && scheme { 1 } else { 0 };
            return HIDDEN.to_string();
        }
        if core.to_ascii_lowercase().starts_with("authorization:") {
            let rest = &core["authorization:".len()..];
            if rest.is_empty() {
                self.auth_words = 2;
            } else if SCHEMES.contains(&rest.to_ascii_lowercase().as_str()) {
                self.auth_words = 1;
            }
            return format!("{}{HIDDEN}", &core[.."authorization:".len()]);
        }
        if is_flag {
            let flag = core.trim_start_matches('-');
            return match core.split_once('=') {
                Some((head, value)) if sensitive_name(head.trim_start_matches('-')) => {
                    let _ = value;
                    format!("{head}={HIDDEN}")
                }
                Some((head, value)) => format!("{head}={}", plain(value)),
                None => {
                    self.hide_next = sensitive_name(flag);
                    plain(core)
                }
            };
        }
        if let Some((name, value)) = core.split_once('=') {
            let ident = !name.is_empty()
                && name
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'));
            if ident && !name.contains("://") {
                return if sensitive_name(name) && !value.is_empty() {
                    format!("{name}={HIDDEN}")
                } else {
                    format!("{name}={}", plain(value))
                };
            }
        }
        plain(core)
    }
}

/// A word with no name in front of it: a credential shape is hidden, URL userinfo is cut out.
fn plain(word: &str) -> String {
    if looks_like_credential(word) {
        return HIDDEN.to_string();
    }
    match word.find("://") {
        Some(at) => {
            let rest = &word[at + 3..];
            let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
            match rest[..authority_end].rfind('@') {
                Some(user_end) => format!("{}{HIDDEN}@{}", &word[..at + 3], &rest[user_end + 1..]),
                None => word.to_string(),
            }
        }
        None => word.to_string(),
    }
}

/// Cuts at 160 characters (never inside a character) and escapes control characters whole.
fn finish(raw: &str) -> String {
    let mut out = String::new();
    for (count, c) in raw.chars().enumerate() {
        if count == MAX_CHARS {
            out.push('…');
            break;
        }
        match c {
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if is_unsafe_char(c) => out.push_str(&format!("\\u{{{:x}}}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::display_value;
    use serde_json::json;

    fn show(text: &str) -> String {
        display_value(&json!(text))
    }

    #[test]
    fn credential_token_is_hidden() {
        assert_eq!(show("run sk-abcdefgh12345678 now"), "run <hidden> now");
    }

    #[test]
    fn value_after_a_secret_flag_is_hidden() {
        assert_eq!(
            show("tool --token abc123 --verbose"),
            "tool --token <hidden> --verbose"
        );
        assert_eq!(show("tool --api-key=abc"), "tool --api-key=<hidden>");
        assert_eq!(show("tool --password hunter2"), "tool --password <hidden>");
        assert_eq!(
            display_value(&json!(["tool", "--secret", "a b"])),
            r#"["tool", "--secret", "<hidden>"]"#
        );
    }

    #[test]
    fn secret_named_assignment_is_hidden() {
        assert_eq!(show("API_TOKEN=abc run"), "API_TOKEN=<hidden> run");
        assert_eq!(show("MODE=fast run"), "MODE=fast run");
    }

    #[test]
    fn url_userinfo_is_hidden() {
        assert_eq!(
            show("git clone https://user:pw@host.example/repo"),
            "git clone https://<hidden>@host.example/repo"
        );
    }

    #[test]
    fn authorization_value_is_hidden() {
        assert_eq!(
            show("curl -H Authorization: Bearer abc ok"),
            "curl -H Authorization:<hidden> <hidden> <hidden> ok"
        );
        assert_eq!(
            show("Authorization: Basic dXNlcg=="),
            "Authorization:<hidden> <hidden> <hidden>"
        );
    }

    #[test]
    fn short_flags_are_not_guessed_and_plain_values_stay() {
        assert_eq!(show("login -p abc"), "login -p abc");
        assert_eq!(
            show("/usr/local/share/capstan/some/long/path/to/file.toml"),
            "/usr/local/share/capstan/some/long/path/to/file.toml"
        );
        assert_eq!(show("claude-opus-4-8"), "claude-opus-4-8");
        assert_eq!(display_value(&json!(42)), "42");
    }

    #[test]
    fn cut_never_splits_a_character_and_escapes_whole() {
        let text = "é".repeat(200);
        let shown = display_value(&json!(text));
        assert_eq!(shown.chars().count(), 161);
        assert!(shown.ends_with('…'));
        let at_boundary = format!("{}\n{}", "a".repeat(159), "b".repeat(10));
        assert_eq!(
            display_value(&json!(at_boundary)),
            format!("{}\\n…", "a".repeat(159))
        );
        let past = format!("{}\n", "a".repeat(160));
        assert_eq!(display_value(&json!(past)), format!("{}…", "a".repeat(160)));
    }
}
