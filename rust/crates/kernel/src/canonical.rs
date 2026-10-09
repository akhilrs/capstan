//! Canonical JSON and the digests over it: src/controller/canonical.ts.
//!
//! `JSON.stringify` of an object whose keys were sorted with `Array.prototype.sort` and inserted in that order. Two
//! consequences are reproduced here: keys compare by UTF-16 code units (not by UTF-8 bytes), and keys that are array
//! indices ("0", "17", never "01") come first in numeric order whatever the sort said.

use serde_json::{Number, Value};
use sha2::{Digest, Sha256};
use std::cmp::Ordering;

/// Lower-case hex of a SHA-256 digest of the UTF-8 bytes of `value`.
pub fn sha256(value: &str) -> String {
    hex(&Sha256::digest(value.as_bytes()))
}

pub fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

pub fn digest_json(value: &Value) -> String {
    sha256(&canonical_json(value))
}

pub fn canonical_json(value: &Value) -> String {
    let mut out = String::new();
    write_value(&mut out, value);
    out
}

fn array_index(key: &str) -> Option<u64> {
    let bytes = key.as_bytes();
    if bytes.is_empty() || bytes.len() > 10 || !bytes.iter().all(u8::is_ascii_digit) {
        return None;
    }
    if bytes.len() > 1 && bytes[0] == b'0' {
        return None;
    }
    let n: u64 = key.parse().ok()?;
    (n < 0xffff_ffff).then_some(n)
}

fn compare_utf16(a: &str, b: &str) -> Ordering {
    a.encode_utf16().cmp(b.encode_utf16())
}

fn write_string(out: &mut String, text: &str) {
    out.push_str(&serde_json::to_string(text).expect("a string always serializes"));
}

fn write_value(out: &mut String, value: &Value) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(true) => out.push_str("true"),
        Value::Bool(false) => out.push_str("false"),
        Value::Number(n) => out.push_str(&js_number(n)),
        Value::String(s) => write_string(out, s),
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_value(out, item);
            }
            out.push(']');
        }
        Value::Object(map) => {
            let mut indexed: Vec<(u64, &String, &Value)> = Vec::new();
            let mut named: Vec<(&String, &Value)> = Vec::new();
            for (key, item) in map {
                match array_index(key) {
                    Some(n) => indexed.push((n, key, item)),
                    None => named.push((key, item)),
                }
            }
            indexed.sort_by_key(|(n, _, _)| *n);
            named.sort_by(|a, b| compare_utf16(a.0, b.0));
            out.push('{');
            let mut first = true;
            let entries = indexed.into_iter().map(|(_, k, v)| (k, v)).chain(named);
            for (key, item) in entries {
                if !first {
                    out.push(',');
                }
                first = false;
                write_string(out, key);
                out.push(':');
                write_value(out, item);
            }
            out.push('}');
        }
    }
}

/// `String(number)` for a JSON number, as `JSON.stringify` prints it.
pub fn js_number(n: &Number) -> String {
    if let Some(i) = n.as_i64() {
        return i.to_string();
    }
    if let Some(u) = n.as_u64() {
        return u.to_string();
    }
    js_f64(n.as_f64().unwrap_or(0.0))
}

/// The ECMAScript Number-to-string conversion (shortest round-trip digits, exponent rules of 6.1.6.1.20).
pub fn js_f64(f: f64) -> String {
    if f == 0.0 {
        return "0".to_string();
    }
    if !f.is_finite() {
        return "null".to_string();
    }
    let sign = if f < 0.0 { "-" } else { "" };
    // Rust's `{:e}` prints the shortest digits that round-trip: "d.ddde[-]x".
    let scientific = format!("{:e}", f.abs());
    let (mantissa, exponent) = scientific
        .split_once('e')
        .expect("LowerExp has an exponent");
    let exponent: i32 = exponent.parse().expect("the exponent is an integer");
    let digits: String = mantissa.chars().filter(|c| *c != '.').collect();
    let k = digits.len() as i32;
    let n = exponent + 1;
    let body = if k <= n && n <= 21 {
        format!("{digits}{}", "0".repeat((n - k) as usize))
    } else if 0 < n && n <= 21 {
        format!("{}.{}", &digits[..n as usize], &digits[n as usize..])
    } else if -6 < n && n <= 0 {
        format!("0.{}{digits}", "0".repeat((-n) as usize))
    } else {
        let e = n - 1;
        let sign_e = if e < 0 { '-' } else { '+' };
        if k == 1 {
            format!("{digits}e{sign_e}{}", e.abs())
        } else {
            format!("{}.{}e{sign_e}{}", &digits[..1], &digits[1..], e.abs())
        }
    };
    format!("{sign}{body}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn sorts_keys_by_utf16_and_puts_indices_first() {
        let value = json!({"b": 1, "a": 2, "10": 3, "9": 4, "\u{e000}": 5, "\u{10000}": 6});
        assert_eq!(
            canonical_json(&value),
            "{\"9\":4,\"10\":3,\"a\":2,\"b\":1,\"\u{10000}\":6,\"\u{e000}\":5}"
        );
    }

    #[test]
    fn numbers_print_like_javascript() {
        for (value, text) in [
            (1.5, "1.5"),
            (1e21, "1e+21"),
            (1e-7, "1e-7"),
            (0.000001, "0.000001"),
            (123456789012345680000.0, "123456789012345680000"),
            (-0.0, "0"),
            (0.1, "0.1"),
            (5e-324, "5e-324"),
        ] {
            assert_eq!(js_f64(value), text);
        }
    }

    #[test]
    fn digest_is_sha256_of_canonical_text() {
        assert_eq!(digest_json(&json!({"a": 1})), sha256("{\"a\":1}"));
    }
}
