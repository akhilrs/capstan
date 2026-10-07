//! Replays the fixtures `test/wire-parity-export.ts` computed with Node, byte for byte.
use std::fs;
use std::path::PathBuf;

use capstan_wire::js::{self, JsStr, Value};
use capstan_wire::{frame, response, Response};

fn fixture(name: &str) -> Vec<Value> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("parity")
        .join(name);
    let text = fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    match js::parse(&text).expect("fixture is JSON") {
        Value::Array(cases) => cases,
        _ => panic!("{name} is not an array"),
    }
}

fn field<'a>(case: &'a Value, key: &str) -> &'a Value {
    case.get(key).unwrap_or_else(|| panic!("case has no {key}"))
}

fn text(value: &Value) -> JsStr {
    match value {
        Value::String(s) => s.clone(),
        other => panic!("not a string: {other:?}"),
    }
}

fn string(case: &Value, key: &str) -> String {
    text(field(case, key)).to_utf8().expect("well-formed text")
}

/// A string, or `{head, text, count, tail}` standing for head + text repeated count times + tail.
fn expand(value: &Value) -> String {
    match value {
        Value::String(s) => s.to_utf8().expect("well-formed text"),
        object => {
            let count = match field(object, "count") {
                Value::Number(n) => *n as usize,
                _ => panic!("count"),
            };
            format!(
                "{}{}{}",
                string(object, "head"),
                string(object, "text").repeat(count),
                string(object, "tail")
            )
        }
    }
}

#[test]
fn numbers_print_as_string_and_json_do() {
    let cases = fixture("numbers.json");
    assert!(cases.len() > 500);
    for case in cases {
        let hex = string(&case, "bits");
        let n = f64::from_bits(u64::from_str_radix(&hex, 16).unwrap());
        assert_eq!(
            js::number_to_string(n),
            string(&case, "string"),
            "String({hex})"
        );
        assert_eq!(
            js::stringify(&Value::Number(n), 0).to_utf8().unwrap(),
            string(&case, "json"),
            "JSON.stringify({hex})"
        );
    }
}

#[test]
fn parse_stringify_and_render_match_node() {
    for case in fixture("parse.json") {
        let input = string(&case, "text");
        let parsed = js::parse(&input);
        if case.get("error").is_some() {
            assert!(parsed.is_err(), "JSON.parse refuses {input:?}");
            continue;
        }
        let value = parsed.unwrap_or_else(|e| panic!("JSON.parse accepts {input:?}: {e}"));
        assert_eq!(
            js::stringify(&value, 0),
            text(field(&case, "compact")),
            "{input:?}"
        );
        if let Some(pretty) = case.get("pretty") {
            assert_eq!(js::stringify(&value, 2), text(pretty), "{input:?} indented");
        }
        assert_eq!(
            js::render(&value),
            text(field(&case, "render")),
            "{input:?}"
        );
    }
}

#[test]
fn frames_match_client_ts() {
    for case in fixture("frames.json") {
        let args: Vec<String> = match field(&case, "args") {
            Value::Array(items) => items.iter().map(expand).collect(),
            _ => panic!("args"),
        };
        let got = frame(
            &string(&case, "credential"),
            &string(&case, "command"),
            &args,
        );
        match field(&case, "frame") {
            Value::Null => assert!(got.is_err(), "an over-long frame is refused"),
            expected => {
                let expected = expand(expected);
                let got = got.expect("the frame fits");
                assert_eq!(got.len(), expected.len());
                assert!(got == expected.as_bytes(), "frame bytes differ");
            }
        }
    }
}

#[test]
fn responses_are_decided_as_client_ts_does() {
    for case in fixture("responses.json") {
        let hex = string(&case, "hex");
        let line: Vec<u8> = (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
            .collect();
        let expect = field(&case, "expect");
        let got = response(&line);
        let label = String::from_utf8_lossy(&line).into_owned();
        match string(expect, "kind").as_str() {
            "malformed" => assert_eq!(got, Response::Malformed, "{label:?}"),
            "ok" => {
                let Response::Ok { result } = got else {
                    panic!("{label:?} is not ok: {got:?}");
                };
                match field(expect, "result") {
                    Value::Null => assert_eq!(result, None, "{label:?}"),
                    expected => assert_eq!(
                        js::stringify(&result.expect("a result"), 0),
                        text(expected),
                        "{label:?}"
                    ),
                }
            }
            "refused" => assert_eq!(
                got,
                Response::Refused {
                    code: text(field(expect, "code")),
                    message: text(field(expect, "message")),
                },
                "{label:?}"
            ),
            other => panic!("unknown kind {other}"),
        }
    }
}

#[test]
fn dates_parse_like_date_parse() {
    for case in fixture("dates.json") {
        let input = string(&case, "text");
        let expected = match field(&case, "ms") {
            Value::Null => None,
            Value::Number(n) => Some(*n as i64),
            other => panic!("{other:?}"),
        };
        assert_eq!(js::date_parse_ms(&input), expected, "Date.parse({input:?})");
    }
}
