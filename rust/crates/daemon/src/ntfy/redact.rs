//! Keeping secrets out of text that is logged, stored or sent in an answer.

/// The most characters of an error that is stored or answered.
pub const MAX_ERROR_CHARS: usize = 200;
const MASK: &str = "[redacted]";

/// `text` with every non-empty member of `secrets` replaced by `[redacted]`, longest first.
pub fn redact(text: &str, secrets: &[String]) -> String {
    let mut needles: Vec<&str> = secrets
        .iter()
        .map(String::as_str)
        .filter(|s| !s.is_empty())
        .collect();
    needles.sort_by_key(|s| std::cmp::Reverse(s.len()));
    let mut out = text.to_string();
    for needle in needles {
        out = out.replace(needle, MASK);
    }
    out
}

/// Control characters become spaces, the ends are trimmed and the text is cut to [`MAX_ERROR_CHARS`] characters.
pub fn clip(text: &str) -> String {
    let flat: String = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    flat.trim().chars().take(MAX_ERROR_CHARS).collect()
}

/// Redacts, then clips: the form in which an error is stored.
pub fn scrub(text: &str, secrets: &[String]) -> String {
    clip(&redact(text, secrets))
}

/// The userinfo of a proxy URL (`user:password`) and its parts, so a log line that echoes only a part is still masked.
pub fn userinfo_parts(url: &str) -> Vec<String> {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let Some((userinfo, _)) = authority.rsplit_once('@') else {
        return Vec::new();
    };
    let mut parts = vec![userinfo.to_string()];
    if let Some((user, password)) = userinfo.split_once(':') {
        parts.push(user.to_string());
        parts.push(password.to_string());
    }
    parts
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secrets_are_masked_longest_first_and_the_result_is_clipped() {
        let secrets = vec!["tok".to_string(), "tok_SENTINEL".to_string()];
        assert_eq!(redact("a tok_SENTINEL b", &secrets), "a [redacted] b");
        let long = "x".repeat(500);
        assert_eq!(scrub(&long, &[]).chars().count(), MAX_ERROR_CHARS);
        assert_eq!(clip("a\nb\u{7f}c"), "a b c");
    }

    #[test]
    fn userinfo_is_split() {
        assert_eq!(
            userinfo_parts("http://u:p@proxy:3128"),
            vec!["u:p".to_string(), "u".to_string(), "p".to_string()]
        );
        assert!(userinfo_parts("http://proxy").is_empty());
    }
}
