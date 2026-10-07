//! The few JavaScript operations the CLI's output code relies on, over `capstan_wire::js::Value`: string and number
//! coercion, strict equality and the `TypeError` texts V8 gives for a property read on `null` or `undefined`.
use capstan_wire::js::{number_to_string, JsStr, Value};

/// An error the Node CLI would print as `cstan: <message>` with this exit code.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JsError {
    pub exit: i32,
    pub message: JsStr,
}

pub const EXIT_INVALID: i32 = 3;
pub const EXIT_BLOCKED: i32 = 4;
pub const EXIT_RUNTIME: i32 = 5;

impl JsError {
    /// A `TypeError` (exit 3 in the CLI).
    pub fn type_error(message: impl Into<JsStr>) -> JsError {
        JsError {
            exit: EXIT_INVALID,
            message: message.into(),
        }
    }

    pub fn runtime(message: impl Into<JsStr>) -> JsError {
        JsError {
            exit: EXIT_RUNTIME,
            message: message.into(),
        }
    }
}

/// `Cannot read properties of null (reading 'x')` / `... of undefined ...`.
pub fn cannot_read(of: Option<&Value>, key: &str) -> JsError {
    let what = match of {
        None => "undefined",
        Some(_) => "null",
    };
    JsError::type_error(format!(
        "Cannot read properties of {what} (reading '{key}')"
    ))
}

/// `value.key` where `value` is known not to be `null`/`undefined`; a primitive has no members.
pub fn member<'a>(value: &'a Value, key: &str) -> Option<&'a Value> {
    value.get(key)
}

/// `value.key` for a value that may be `undefined` (`None`) or `null`: those throw.
pub fn read<'a>(value: Option<&'a Value>, key: &str) -> Result<Option<&'a Value>, JsError> {
    match value {
        None => Err(cannot_read(None, key)),
        Some(Value::Null) => Err(cannot_read(Some(&Value::Null), key)),
        Some(v) => Ok(member(v, key)),
    }
}

/// `value?.key`.
pub fn read_opt<'a>(value: Option<&'a Value>, key: &str) -> Option<&'a Value> {
    match value {
        None | Some(Value::Null) => None,
        Some(v) => member(v, key),
    }
}

/// `value ?? fallback` is "not nullish": `None` and `null` are nullish.
pub fn nullish(value: Option<&Value>) -> bool {
    matches!(value, None | Some(Value::Null))
}

/// `String(value)` / the conversion a template literal applies.
pub fn to_js_string(value: &Value) -> JsStr {
    match value {
        Value::Null => "null".into(),
        Value::Bool(b) => if *b { "true" } else { "false" }.into(),
        Value::Number(n) => number_to_string(*n).into(),
        Value::String(s) => s.clone(),
        Value::Array(items) => {
            let mut out = JsStr::default();
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push_str(",");
                }
                if !matches!(item, Value::Null) {
                    out = join(out, to_js_string(item));
                }
            }
            out
        }
        Value::Object(_) => "[object Object]".into(),
    }
}

fn join(mut left: JsStr, right: JsStr) -> JsStr {
    let mut units = left.as_units().to_vec();
    units.extend_from_slice(right.as_units());
    left = JsStr::from(units);
    left
}

/// A template-literal substitution `${value}` of a member that may be absent (`undefined`).
pub fn template(value: Option<&Value>) -> JsStr {
    match value {
        None => "undefined".into(),
        Some(v) => to_js_string(v),
    }
}

/// `${value ?? ""}`.
pub fn template_or_empty(value: Option<&Value>) -> JsStr {
    if nullish(value) {
        JsStr::default()
    } else {
        template(value)
    }
}

/// Appends text to a `JsStr`.
pub fn concat(parts: &[&JsStr]) -> JsStr {
    let mut units = Vec::new();
    for part in parts {
        units.extend_from_slice(part.as_units());
    }
    JsStr::from(units)
}

fn is_js_space(c: char) -> bool {
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

/// Whether `/[\s\p{Cc}]/u` matches somewhere in the text.
pub fn has_space_or_control(text: &str) -> bool {
    text.chars().any(|c| is_js_space(c) || c.is_control())
}

/// `Number(text)` for a string.
pub fn string_to_number(text: &str) -> f64 {
    let trimmed = text.trim_matches(is_js_space);
    if trimmed.is_empty() {
        return 0.0;
    }
    for (prefix, radix) in [
        ("0x", 16),
        ("0X", 16),
        ("0o", 8),
        ("0O", 8),
        ("0b", 2),
        ("0B", 2),
    ] {
        if let Some(digits) = trimmed.strip_prefix(prefix) {
            if digits.is_empty() || !digits.chars().all(|c| c.is_digit(radix)) {
                return f64::NAN;
            }
            return digits.chars().fold(0.0, |acc, c| {
                acc * f64::from(radix) + f64::from(c.to_digit(radix).unwrap())
            });
        }
    }
    let (sign, body) = match trimmed.as_bytes()[0] {
        b'+' => (1.0, &trimmed[1..]),
        b'-' => (-1.0, &trimmed[1..]),
        _ => (1.0, trimmed),
    };
    if body == "Infinity" {
        return sign * f64::INFINITY;
    }
    let bytes = body.as_bytes();
    let mut at = 0;
    let digits = |at: &mut usize| {
        let start = *at;
        while bytes.get(*at).is_some_and(u8::is_ascii_digit) {
            *at += 1;
        }
        *at - start
    };
    let whole = digits(&mut at);
    let mut fraction = 0;
    if bytes.get(at) == Some(&b'.') {
        at += 1;
        fraction = digits(&mut at);
    }
    if whole + fraction == 0 {
        return f64::NAN;
    }
    if matches!(bytes.get(at), Some(b'e' | b'E')) {
        at += 1;
        if matches!(bytes.get(at), Some(b'+' | b'-')) {
            at += 1;
        }
        if digits(&mut at) == 0 {
            return f64::NAN;
        }
    }
    if at != bytes.len() {
        return f64::NAN;
    }
    body.parse::<f64>().map_or(f64::NAN, |n| sign * n)
}

/// `Number(value)`; `None` is `undefined` (NaN).
pub fn to_number(value: Option<&Value>) -> f64 {
    match value {
        None => f64::NAN,
        Some(Value::Null) => 0.0,
        Some(Value::Bool(b)) => f64::from(u8::from(*b)),
        Some(Value::Number(n)) => *n,
        Some(Value::String(s)) => string_to_number(&s.to_utf8_lossy()),
        Some(v @ Value::Array(_)) => string_to_number(&to_js_string(v).to_utf8_lossy()),
        Some(Value::Object(_)) => f64::NAN,
    }
}

/// `a === b` for values read from JSON (objects and arrays are never identical).
pub fn strict_eq(a: Option<&Value>, b: Option<&Value>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(Value::Null), Some(Value::Null)) => true,
        (Some(Value::Bool(x)), Some(Value::Bool(y))) => x == y,
        (Some(Value::Number(x)), Some(Value::Number(y))) => x == y,
        (Some(Value::String(x)), Some(Value::String(y))) => x == y,
        _ => false,
    }
}

/// `value === true`.
pub fn is_true(value: Option<&Value>) -> bool {
    matches!(value, Some(Value::Bool(true)))
}

/// `typeof value === "number"` and its number.
pub fn as_number(value: Option<&Value>) -> Option<f64> {
    match value {
        Some(Value::Number(n)) => Some(*n),
        _ => None,
    }
}
