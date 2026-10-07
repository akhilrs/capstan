//! A JSON value with JavaScript semantics: strings are UTF-16 (lone surrogates survive), object keys keep JavaScript's
//! property order, numbers print as `String(n)` does. `parse`, `stringify`, `render` and `date_parse_ms` match
//! `JSON.parse`, `JSON.stringify`, the CLI's `render()` and `Date.parse` (ISO strings, offset-less times read as UTC).
use std::collections::HashMap;
use std::fmt;

/// A JavaScript string: UTF-16 code units, not necessarily well formed.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct JsStr(Vec<u16>);

impl JsStr {
    pub fn as_units(&self) -> &[u16] {
        &self.0
    }

    pub fn push_str(&mut self, text: &str) {
        self.0.extend(text.encode_utf16());
    }

    /// The text with every lone surrogate replaced by U+FFFD.
    pub fn to_utf8_lossy(&self) -> String {
        String::from_utf16_lossy(&self.0)
    }

    /// `Some` when the string has no lone surrogate.
    pub fn to_utf8(&self) -> Option<String> {
        String::from_utf16(&self.0).ok()
    }

    pub fn len_units(&self) -> usize {
        self.0.len()
    }
}

impl From<&str> for JsStr {
    fn from(text: &str) -> Self {
        JsStr(text.encode_utf16().collect())
    }
}

impl From<String> for JsStr {
    fn from(text: String) -> Self {
        JsStr::from(text.as_str())
    }
}

impl From<Vec<u16>> for JsStr {
    fn from(units: Vec<u16>) -> Self {
        JsStr(units)
    }
}

impl fmt::Display for JsStr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_utf8_lossy())
    }
}

/// A JSON value. Object members are in JavaScript property order: array-index keys ascending, then the rest as inserted.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Number(f64),
    String(JsStr),
    Array(Vec<Value>),
    Object(Vec<(JsStr, Value)>),
}

impl Value {
    /// JavaScript truthiness.
    pub fn truthy(&self) -> bool {
        match self {
            Value::Null => false,
            Value::Bool(b) => *b,
            Value::Number(n) => !(*n == 0.0 || n.is_nan()),
            Value::String(s) => s.len_units() > 0,
            Value::Array(_) | Value::Object(_) => true,
        }
    }

    /// The member `key` of an object.
    pub fn get(&self, key: &str) -> Option<&Value> {
        let key = JsStr::from(key);
        match self {
            Value::Object(members) => members.iter().find(|(k, _)| *k == key).map(|(_, v)| v),
            _ => None,
        }
    }
}

/// Why a text is not JSON.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseError {
    pub at: usize,
    pub message: &'static str,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} at byte {}", self.message, self.at)
    }
}

impl std::error::Error for ParseError {}

/// Nesting beyond this is refused rather than risking the stack (JSON.parse also has a limit, far higher).
const MAX_DEPTH: usize = 2048;

/// `JSON.parse(text)`. A BOM is not whitespace, so a text that starts with one is an error. Duplicate keys keep the
/// last value in the first position.
pub fn parse(text: &str) -> Result<Value, ParseError> {
    let mut parser = Parser {
        bytes: text.as_bytes(),
        text,
        at: 0,
    };
    parser.skip_space();
    let value = parser.value(0)?;
    parser.skip_space();
    if parser.at != parser.bytes.len() {
        return Err(parser.error("unexpected data after the value"));
    }
    Ok(value)
}

struct Parser<'a> {
    bytes: &'a [u8],
    text: &'a str,
    at: usize,
}

impl Parser<'_> {
    fn error(&self, message: &'static str) -> ParseError {
        ParseError {
            at: self.at,
            message,
        }
    }

    fn skip_space(&mut self) {
        while matches!(self.bytes.get(self.at), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.at += 1;
        }
    }

    fn eat(&mut self, byte: u8) -> bool {
        if self.bytes.get(self.at) == Some(&byte) {
            self.at += 1;
            true
        } else {
            false
        }
    }

    fn literal(&mut self, word: &str, value: Value) -> Result<Value, ParseError> {
        if self.bytes[self.at..].starts_with(word.as_bytes()) {
            self.at += word.len();
            Ok(value)
        } else {
            Err(self.error("unexpected token"))
        }
    }

    fn value(&mut self, depth: usize) -> Result<Value, ParseError> {
        if depth > MAX_DEPTH {
            return Err(self.error("nested too deeply"));
        }
        match self.bytes.get(self.at) {
            None => Err(self.error("unexpected end")),
            Some(b'n') => self.literal("null", Value::Null),
            Some(b't') => self.literal("true", Value::Bool(true)),
            Some(b'f') => self.literal("false", Value::Bool(false)),
            Some(b'"') => Ok(Value::String(self.string()?)),
            Some(b'[') => {
                self.at += 1;
                let mut items = Vec::new();
                self.skip_space();
                if self.eat(b']') {
                    return Ok(Value::Array(items));
                }
                loop {
                    self.skip_space();
                    items.push(self.value(depth + 1)?);
                    self.skip_space();
                    if self.eat(b',') {
                        continue;
                    }
                    if self.eat(b']') {
                        return Ok(Value::Array(items));
                    }
                    return Err(self.error("expected , or ]"));
                }
            }
            Some(b'{') => {
                self.at += 1;
                let mut members: Vec<(JsStr, Value)> = Vec::new();
                let mut index: HashMap<JsStr, usize> = HashMap::new();
                self.skip_space();
                if self.eat(b'}') {
                    return Ok(Value::Object(members));
                }
                loop {
                    self.skip_space();
                    if self.bytes.get(self.at) != Some(&b'"') {
                        return Err(self.error("expected a key"));
                    }
                    let key = self.string()?;
                    self.skip_space();
                    if !self.eat(b':') {
                        return Err(self.error("expected :"));
                    }
                    self.skip_space();
                    let value = self.value(depth + 1)?;
                    match index.get(&key) {
                        Some(&position) => members[position].1 = value,
                        None => {
                            index.insert(key.clone(), members.len());
                            members.push((key, value));
                        }
                    }
                    self.skip_space();
                    if self.eat(b',') {
                        continue;
                    }
                    if self.eat(b'}') {
                        order_members(&mut members);
                        return Ok(Value::Object(members));
                    }
                    return Err(self.error("expected , or }"));
                }
            }
            Some(b'-' | b'0'..=b'9') => self.number(),
            Some(_) => Err(self.error("unexpected token")),
        }
    }

    fn digits(&mut self) -> usize {
        let start = self.at;
        while self.bytes.get(self.at).is_some_and(u8::is_ascii_digit) {
            self.at += 1;
        }
        self.at - start
    }

    fn number(&mut self) -> Result<Value, ParseError> {
        let start = self.at;
        self.eat(b'-');
        match self.bytes.get(self.at) {
            Some(b'0') => self.at += 1,
            Some(b'1'..=b'9') => {
                self.digits();
            }
            _ => return Err(self.error("bad number")),
        }
        if self.eat(b'.') && self.digits() == 0 {
            return Err(self.error("bad number"));
        }
        if matches!(self.bytes.get(self.at), Some(b'e' | b'E')) {
            self.at += 1;
            if !self.eat(b'+') {
                self.eat(b'-');
            }
            if self.digits() == 0 {
                return Err(self.error("bad number"));
            }
        }
        // The slice is ASCII, and Rust rounds a decimal string to the nearest double as JavaScript does.
        let number = self.text[start..self.at]
            .parse::<f64>()
            .map_err(|_| self.error("bad number"))?;
        Ok(Value::Number(number))
    }

    fn hex4(&mut self) -> Result<u16, ParseError> {
        let digits = self
            .bytes
            .get(self.at..self.at + 4)
            .ok_or_else(|| self.error("bad escape"))?;
        let mut unit = 0u16;
        for byte in digits {
            let digit = (*byte as char)
                .to_digit(16)
                .ok_or_else(|| self.error("bad escape"))?;
            unit = unit << 4 | digit as u16;
        }
        self.at += 4;
        Ok(unit)
    }

    fn string(&mut self) -> Result<JsStr, ParseError> {
        self.at += 1;
        let mut out: Vec<u16> = Vec::new();
        loop {
            let Some(&byte) = self.bytes.get(self.at) else {
                return Err(self.error("unterminated string"));
            };
            match byte {
                b'"' => {
                    self.at += 1;
                    return Ok(JsStr(out));
                }
                0..=0x1f => return Err(self.error("control character in string")),
                b'\\' => {
                    self.at += 1;
                    let Some(&escape) = self.bytes.get(self.at) else {
                        return Err(self.error("unterminated string"));
                    };
                    self.at += 1;
                    out.push(match escape {
                        b'"' => 0x22,
                        b'\\' => 0x5c,
                        b'/' => 0x2f,
                        b'b' => 0x08,
                        b'f' => 0x0c,
                        b'n' => 0x0a,
                        b'r' => 0x0d,
                        b't' => 0x09,
                        b'u' => self.hex4()?,
                        _ => return Err(self.error("bad escape")),
                    });
                }
                0x20..=0x7f => {
                    out.push(u16::from(byte));
                    self.at += 1;
                }
                _ => {
                    let c = self.text[self.at..]
                        .chars()
                        .next()
                        .ok_or_else(|| self.error("bad text"))?;
                    let mut buffer = [0u16; 2];
                    out.extend_from_slice(c.encode_utf16(&mut buffer));
                    self.at += c.len_utf8();
                }
            }
        }
    }
}

/// The array index a key names (a canonical decimal below 2^32 - 1), if it names one.
fn array_index(key: &JsStr) -> Option<u32> {
    let units = key.as_units();
    if units.is_empty() || units.len() > 10 || (units.len() > 1 && units[0] == u16::from(b'0')) {
        return None;
    }
    let mut value: u64 = 0;
    for unit in units {
        if !(u16::from(b'0')..=u16::from(b'9')).contains(unit) {
            return None;
        }
        value = value * 10 + u64::from(unit - u16::from(b'0'));
    }
    u32::try_from(value).ok().filter(|v| *v != u32::MAX)
}

/// JavaScript lists array-index properties first, ascending, then the others in insertion order.
fn order_members(members: &mut [(JsStr, Value)]) {
    if members.iter().any(|(k, _)| array_index(k).is_some()) {
        // A stable sort keeps insertion order within the non-index keys.
        members.sort_by_key(|(k, _)| array_index(k).map_or((1u8, 0u32), |i| (0, i)));
    }
}

/// `String(n)`.
pub fn number_to_string(n: f64) -> String {
    if n.is_nan() {
        return "NaN".into();
    }
    if n == 0.0 {
        return "0".into();
    }
    if n.is_infinite() {
        return if n > 0.0 { "Infinity" } else { "-Infinity" }.into();
    }
    let sign = if n < 0.0 { "-" } else { "" };
    // `{:e}` prints the shortest digits that round-trip: `d.ddde<exp>`.
    let scientific = format!("{:e}", n.abs());
    let (mantissa, exponent) = scientific.split_once('e').expect("{:e} has an exponent");
    let digits: String = mantissa.chars().filter(|c| *c != '.').collect();
    let k = digits.len() as i32;
    let n = exponent.parse::<i32>().expect("exponent is a number") + 1;
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

fn quote(text: &JsStr, out: &mut Vec<u16>) {
    let push_ascii = |out: &mut Vec<u16>, s: &str| out.extend(s.bytes().map(u16::from));
    out.push(0x22);
    let units = text.as_units();
    let mut i = 0;
    while i < units.len() {
        let unit = units[i];
        match unit {
            0x22 => push_ascii(out, "\\\""),
            0x5c => push_ascii(out, "\\\\"),
            0x08 => push_ascii(out, "\\b"),
            0x0c => push_ascii(out, "\\f"),
            0x0a => push_ascii(out, "\\n"),
            0x0d => push_ascii(out, "\\r"),
            0x09 => push_ascii(out, "\\t"),
            0..=0x1f => push_ascii(out, &format!("\\u{unit:04x}")),
            0xd800..=0xdbff if matches!(units.get(i + 1), Some(0xdc00..=0xdfff)) => {
                out.push(unit);
                out.push(units[i + 1]);
                i += 1;
            }
            0xd800..=0xdfff => push_ascii(out, &format!("\\u{unit:04x}")),
            _ => out.push(unit),
        }
        i += 1;
    }
    out.push(0x22);
}

fn write_value(value: &Value, indent: &[u16], level: usize, out: &mut Vec<u16>) {
    let ascii = |out: &mut Vec<u16>, s: &str| out.extend(s.bytes().map(u16::from));
    let newline = |out: &mut Vec<u16>, level: usize| {
        if !indent.is_empty() {
            out.push(0x0a);
            for _ in 0..level {
                out.extend_from_slice(indent);
            }
        }
    };
    match value {
        Value::Null => ascii(out, "null"),
        Value::Bool(true) => ascii(out, "true"),
        Value::Bool(false) => ascii(out, "false"),
        Value::Number(n) if n.is_finite() => ascii(out, &number_to_string(*n)),
        Value::Number(_) => ascii(out, "null"),
        Value::String(s) => quote(s, out),
        Value::Array(items) if items.is_empty() => ascii(out, "[]"),
        Value::Object(members) if members.is_empty() => ascii(out, "{}"),
        Value::Array(items) => {
            out.push(u16::from(b'['));
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(u16::from(b','));
                }
                newline(out, level + 1);
                write_value(item, indent, level + 1, out);
            }
            newline(out, level);
            out.push(u16::from(b']'));
        }
        Value::Object(members) => {
            out.push(u16::from(b'{'));
            for (i, (key, item)) in members.iter().enumerate() {
                if i > 0 {
                    out.push(u16::from(b','));
                }
                newline(out, level + 1);
                quote(key, out);
                out.push(u16::from(b':'));
                if !indent.is_empty() {
                    out.push(u16::from(b' '));
                }
                write_value(item, indent, level + 1, out);
            }
            newline(out, level);
            out.push(u16::from(b'}'));
        }
    }
}

/// `JSON.stringify(value, null, indent)`: `indent` spaces per level, clamped to 10; 0 is compact. Non-finite numbers
/// print as `null`, lone surrogates as `\udXXX`.
pub fn stringify(value: &Value, indent: usize) -> JsStr {
    let indent = vec![u16::from(b' '); indent.min(10)];
    let mut out = Vec::new();
    write_value(value, &indent, 0, &mut out);
    JsStr(out)
}

/// The CLI's `render()`: strings as they are, arrays one item per line, objects as `key: value` lines, anything else as
/// `String(value)`.
pub fn render(value: &Value) -> JsStr {
    match value {
        Value::String(s) => s.clone(),
        Value::Array(items) => join_lines(items.iter().map(render)),
        Value::Object(members) => join_lines(members.iter().map(|(key, item)| {
            let mut line = key.clone();
            line.push_str(": ");
            line.0.extend(render(item).0);
            line
        })),
        Value::Null => JsStr::from("null"),
        Value::Bool(b) => JsStr::from(if *b { "true" } else { "false" }),
        Value::Number(n) => JsStr::from(number_to_string(*n)),
    }
}

fn join_lines(lines: impl Iterator<Item = JsStr>) -> JsStr {
    let mut out = JsStr::default();
    for (i, line) in lines.enumerate() {
        if i > 0 {
            out.0.push(0x0a);
        }
        out.0.extend(line.0);
    }
    out
}

fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    // Howard Hinnant's algorithm; month 1..=12, any day (it may overflow the month like JavaScript's MakeDay).
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// `Date.parse(text)` for ISO strings: `YYYY`, `YYYY-MM`, `YYYY-MM-DD`, optionally with `THH:mm[:ss[.fff]]` and `Z` or
/// `±HH:mm`, and the `±YYYYYY` form. As V8 does, it accepts a lower-case `t`/`z`, `±HHmm`, more than three fraction
/// digits (cut to milliseconds) and a day up to 31 in any month (it rolls over). Times with no offset are read as UTC.
/// `None` is `NaN`: not ISO (the other formats V8 accepts are not handled) or outside the range of a date.
pub fn date_parse_ms(text: &str) -> Option<i64> {
    let b = text.as_bytes();
    let mut at = 0usize;
    let number = |at: &mut usize, count: usize| -> Option<i64> {
        let digits = b.get(*at..*at + count)?;
        if !digits.iter().all(u8::is_ascii_digit) {
            return None;
        }
        *at += count;
        Some(digits.iter().fold(0, |v, d| v * 10 + i64::from(d - b'0')))
    };
    let year = match b.first()? {
        sign @ (b'+' | b'-') => {
            at = 1;
            let y = number(&mut at, 6)?;
            if *sign == b'-' {
                if y == 0 {
                    return None;
                }
                -y
            } else {
                y
            }
        }
        _ => number(&mut at, 4)?,
    };
    let (mut month, mut day) = (1, 1);
    if b.get(at) == Some(&b'-') {
        at += 1;
        month = number(&mut at, 2)?;
        if b.get(at) == Some(&b'-') {
            at += 1;
            day = number(&mut at, 2)?;
        }
    }
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let (mut hour, mut minute, mut second, mut milli) = (0, 0, 0, 0);
    let mut offset_minutes = 0;
    if matches!(b.get(at), Some(b'T' | b't')) {
        at += 1;
        hour = number(&mut at, 2)?;
        if b.get(at) != Some(&b':') {
            return None;
        }
        at += 1;
        minute = number(&mut at, 2)?;
        if b.get(at) == Some(&b':') {
            at += 1;
            second = number(&mut at, 2)?;
            if b.get(at) == Some(&b'.') {
                at += 1;
                let start = at;
                while b.get(at).is_some_and(u8::is_ascii_digit) {
                    at += 1;
                }
                if at == start {
                    return None;
                }
                let mut fraction = b[start..at.min(start + 3)]
                    .iter()
                    .fold(0, |v, d| v * 10 + i64::from(d - b'0'));
                for _ in at.min(start + 3) - start..3 {
                    fraction *= 10;
                }
                milli = fraction;
            }
        }
        if minute > 59
            || second > 59
            || hour > 24
            || (hour == 24 && (minute, second, milli) != (0, 0, 0))
        {
            return None;
        }
        match b.get(at) {
            Some(b'Z' | b'z') => at += 1,
            Some(sign @ (b'+' | b'-')) => {
                let negative = *sign == b'-';
                at += 1;
                let oh = number(&mut at, 2)?;
                if b.get(at) == Some(&b':') {
                    at += 1;
                }
                let om = number(&mut at, 2)?;
                if oh > 23 || om > 59 {
                    return None;
                }
                offset_minutes = (oh * 60 + om) * if negative { -1 } else { 1 };
            }
            _ => {}
        }
    } else if b.get(at).is_some_and(|c| matches!(c, b'Z' | b'z')) {
        at += 1;
    }
    if at != b.len() {
        return None;
    }
    let days = days_from_civil(year, month, day);
    let ms = ((days * 24 + hour) * 60 + minute - offset_minutes) * 60_000 + second * 1000 + milli;
    (ms.abs() <= 8_640_000_000_000_000).then_some(ms)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_print_like_javascript() {
        for (n, text) in [
            (0.0, "0"),
            (-0.0, "0"),
            (1.0, "1"),
            (5e-7, "5e-7"),
            (1e21, "1e+21"),
            (1e20, "100000000000000000000"),
            (0.000001, "0.000001"),
            (123.456, "123.456"),
            (-1.5e-9, "-1.5e-9"),
            (f64::MAX, "1.7976931348623157e+308"),
        ] {
            assert_eq!(number_to_string(n), text);
        }
    }

    #[test]
    fn lone_surrogates_survive() {
        let value = parse(r#"["\ud800", "a\udc00b"]"#).unwrap();
        assert_eq!(
            stringify(&value, 0).to_utf8_lossy(),
            r#"["\ud800","a\udc00b"]"#
        );
    }

    #[test]
    fn duplicate_and_index_keys_order_like_javascript() {
        let value = parse(r#"{"b":1,"2":0,"a":2,"b":3,"1":9}"#).unwrap();
        assert_eq!(
            stringify(&value, 0).to_utf8_lossy(),
            r#"{"1":9,"2":0,"b":3,"a":2}"#
        );
    }

    #[test]
    fn a_bom_is_not_json() {
        assert!(parse("\u{feff}{}").is_err());
    }

    #[test]
    fn dates() {
        assert_eq!(
            date_parse_ms("2020-01-01T10:00:00.5+01:00"),
            Some(1_577_869_200_500)
        );
        assert_eq!(date_parse_ms("2020-13-01"), None);
    }
}
