//! JavaScript text semantics the configuration checks depend on: `String.prototype.trim`, `.length` in UTF-16 code
//! units, the `\p{Cc}\p{Cf}\p{Zl}\p{Zp}` test and the credential shapes, written by hand so no regex engine is linked.

// Unicode 16.0 (Node v24.6.0)
const FORMAT: &[(u32, u32)] = &[
    (0xAD, 0xAD),
    (0x600, 0x605),
    (0x61C, 0x61C),
    (0x6DD, 0x6DD),
    (0x70F, 0x70F),
    (0x890, 0x891),
    (0x8E2, 0x8E2),
    (0x180E, 0x180E),
    (0x200B, 0x200F),
    (0x202A, 0x202E),
    (0x2060, 0x2064),
    (0x2066, 0x206F),
    (0xFEFF, 0xFEFF),
    (0xFFF9, 0xFFFB),
    (0x110BD, 0x110BD),
    (0x110CD, 0x110CD),
    (0x13430, 0x1343F),
    (0x1BCA0, 0x1BCA3),
    (0x1D173, 0x1D17A),
    (0xE0001, 0xE0001),
    (0xE0020, 0xE007F),
];

/// What `\s` and `String.prototype.trim` treat as white space (WhiteSpace and LineTerminator of ECMAScript).
pub fn is_js_space(c: char) -> bool {
    matches!(
        c,
        '\t' | '\n' | '\u{b}' | '\u{c}' | '\r' | ' ' | '\u{a0}' | '\u{1680}' | '\u{2000}'
            ..='\u{200a}'
                | '\u{2028}'
                | '\u{2029}'
                | '\u{202f}'
                | '\u{205f}'
                | '\u{3000}'
                | '\u{feff}'
    )
}

/// `text.trim()`.
pub fn trim(text: &str) -> &str {
    text.trim_matches(is_js_space)
}

/// `text.length`.
pub fn len16(text: &str) -> usize {
    text.chars().map(char::len_utf16).sum()
}

/// `/[\p{Cc}\p{Cf}\p{Zl}\p{Zp}]/u` on one code point.
pub fn is_unsafe_char(c: char) -> bool {
    let code = c as u32;
    if code <= 0x1f || (0x7f..=0x9f).contains(&code) || code == 0x2028 || code == 0x2029 {
        return true;
    }
    if code < 0xad {
        return false;
    }
    FORMAT
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

fn is_word(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

/// Whether `/\bPREFIX(CLASS){min,}/` matches somewhere in `text`.
fn word_prefix_run(text: &[u8], prefix: &[u8], min: usize, class: impl Fn(u8) -> bool) -> bool {
    let mut from = 0;
    while let Some(found) = find(text, prefix, from) {
        let boundary = found == 0 || !is_word(text[found - 1]);
        if boundary {
            let run = text[found + prefix.len()..]
                .iter()
                .take_while(|byte| class(**byte))
                .count();
            if run >= min {
                return true;
            }
        }
        from = found + 1;
    }
    false
}

fn find(text: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    if needle.is_empty() || from + needle.len() > text.len() {
        return None;
    }
    (from..=text.len() - needle.len()).find(|&at| &text[at..at + needle.len()] == needle)
}

/// Whether any of `CREDENTIAL_SHAPES` of `src/config/primitives.ts` matches `text`.
pub fn looks_like_credential(text: &str) -> bool {
    let bytes = text.as_bytes();
    let token = |b: u8| b.is_ascii_alphanumeric() || b == b'_' || b == b'-';
    if word_prefix_run(bytes, b"sk-", 8, token) {
        return true;
    }
    for prefix in [b"ghp_", b"gho_", b"ghu_", b"ghs_", b"ghr_"] {
        if word_prefix_run(bytes, prefix, 8, |b| b.is_ascii_alphanumeric()) {
            return true;
        }
    }
    if word_prefix_run(bytes, b"AKIA", 8, |b| {
        b.is_ascii_uppercase() || b.is_ascii_digit()
    }) {
        return true;
    }
    if bearer(bytes) {
        return true;
    }
    private_key(bytes)
}

/// `/\bBearer[ \t]+(?=[A-Za-z0-9._~+/=-]*\d)[A-Za-z0-9._~+/=-]{16,}/`.
fn bearer(text: &[u8]) -> bool {
    let class = |b: u8| b.is_ascii_alphanumeric() || b"._~+/=-".contains(&b);
    let mut from = 0;
    while let Some(found) = find(text, b"Bearer", from) {
        from = found + 1;
        if found > 0 && is_word(text[found - 1]) {
            continue;
        }
        let rest = &text[found + 6..];
        let spaces = rest
            .iter()
            .take_while(|b| **b == b' ' || **b == b'\t')
            .count();
        if spaces == 0 {
            continue;
        }
        let run: Vec<u8> = rest[spaces..]
            .iter()
            .copied()
            .take_while(|b| class(*b))
            .collect();
        if run.len() >= 16 && run.iter().any(u8::is_ascii_digit) {
            return true;
        }
    }
    false
}

/// `/-----BEGIN [A-Z ]*PRIVATE KEY-----/`.
fn private_key(text: &[u8]) -> bool {
    const HEAD: &[u8] = b"-----BEGIN ";
    const TAIL: &[u8] = b"PRIVATE KEY-----";
    let mut from = 0;
    while let Some(found) = find(text, HEAD, from) {
        from = found + 1;
        let start = found + HEAD.len();
        let run = text[start..]
            .iter()
            .take_while(|b| b.is_ascii_uppercase() || **b == b' ')
            .count();
        let end = start + run;
        // The class holds every letter of "PRIVATE KEY", so the tail starts 11 characters before the dashes.
        if run >= 11 && text[end - 11..end] == TAIL[..11] && text[end..].starts_with(b"-----") {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trim_follows_javascript() {
        assert_eq!(trim("\u{feff}\u{a0} a b \u{3000}\n"), "a b");
        // U+0085 is white space to Rust's `trim` but not to JavaScript's.
        assert_eq!(trim("\u{85}a\u{85}"), "\u{85}a\u{85}");
        assert_eq!(trim("\u{180e}a"), "\u{180e}a");
    }

    #[test]
    fn length_counts_utf16_units() {
        assert_eq!(len16("a\u{1F600}b"), 4);
    }

    #[test]
    fn unsafe_characters_are_control_format_and_separators() {
        for unsafe_char in [
            '\u{0}',
            '\u{1f}',
            '\u{7f}',
            '\u{9f}',
            '\u{ad}',
            '\u{200b}',
            '\u{202e}',
            '\u{2028}',
            '\u{2029}',
            '\u{feff}',
            '\u{e0001}',
        ] {
            assert!(is_unsafe_char(unsafe_char), "{unsafe_char:?}");
        }
        for safe in [' ', 'a', '\u{a0}', '\u{a1}', '\u{3000}', '\u{1F600}', 'é'] {
            assert!(!is_unsafe_char(safe), "{safe:?}");
        }
    }

    #[test]
    fn credential_shapes_match_the_node_patterns() {
        for secret in [
            "sk-abcdefgh",
            "x sk-abcdefgh_-1",
            "ghp_abcdefgh1",
            "AKIAABCDEFGH",
            "Bearer \tabcdefghijklmno1",
            "-----BEGIN RSA PRIVATE KEY-----",
            "-----BEGIN PRIVATE KEY-----",
        ] {
            assert!(looks_like_credential(secret), "{secret}");
        }
        for plain in [
            "sk-abcdefg",
            "task-abcdefghij",
            "ghp_abcdefg",
            "AKIAABCDEFG",
            "Bearer abcdefghijklmnop",
            "Bearer abcdefghijklmn1",
            "xBearer abcdefghijklmno1",
            "-----BEGIN rsa PRIVATE KEY-----",
            "-----BEGIN KEY-----",
        ] {
            assert!(!looks_like_credential(plain), "{plain}");
        }
    }
}
