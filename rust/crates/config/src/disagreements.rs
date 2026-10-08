//! Texts the Rust TOML parser (`toml` 1.1, TOML 1.1) reads but `smol-toml` 1.8 refuses, found by the differential run
//! (test/config-parity-export.ts) and the corpus. Accepting one natively would succeed where `cstan config check` in Node
//! fails, so each is a rule that makes the loader return `ConfigError::Parse` instead and leave the command to Node.
//! Rust refusing what Node accepts is harmless (the command also goes to Node) and is not listed.

/// `Some(rule)` when `source` (after the loader's BOM and CRLF handling) is one Node refuses.
pub fn node_refuses(source: &str) -> Option<&'static str> {
    if source.starts_with('\u{feff}') {
        return Some("a second byte order mark");
    }
    if newline_in_inline_table(source) {
        return Some("a line break inside an inline table");
    }
    None
}

/// Whether a line break sits inside an inline table `{ ... }`. TOML 1.1 allows one anywhere there (before a key, around the
/// `=`, before the `}`); `smol-toml` allows some of those places and not others, so every such document is left to Node.
/// The scan skips strings and comments, which is all it needs of the grammar because the text has already parsed.
fn newline_in_inline_table(source: &str) -> bool {
    let bytes = source.as_bytes();
    let (mut at, mut depth) = (0, 0usize);
    while at < bytes.len() {
        match bytes[at] {
            b'#' => {
                while at < bytes.len() && bytes[at] != b'\n' {
                    at += 1;
                }
                continue;
            }
            quote @ (b'"' | b'\'') => {
                let multi = bytes[at..].starts_with(&[quote, quote, quote]);
                at += if multi { 3 } else { 1 };
                while at < bytes.len() {
                    if quote == b'"' && bytes[at] == b'\\' {
                        at += 2;
                        continue;
                    }
                    if multi && bytes[at..].starts_with(&[quote, quote, quote]) {
                        at += 3;
                        break;
                    }
                    if !multi && (bytes[at] == quote || bytes[at] == b'\n') {
                        at += 1;
                        break;
                    }
                    at += 1;
                }
                continue;
            }
            b'{' => depth += 1,
            b'}' => depth = depth.saturating_sub(1),
            b'\n' if depth > 0 => return true,
            _ => {}
        }
        at += 1;
    }
    false
}
