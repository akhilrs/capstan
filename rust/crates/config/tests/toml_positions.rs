//! Where `smol-toml` refuses a text, and that it refuses exactly the texts the Rust port says it does. With
//! `CAPSTAN_CONFIG_POSITIONS=<file>` (JSON lines of `{index, kind, line, column, base64}`, written by running `smol-toml`'s
//! `parse` over the texts of `node dist/test/config-parity-export.js --differential`) every text is checked: `ok` must have
//! no refusal, `toml` must be refused at that line and column. Without the file only the hand-written cases below run.
mod common;

use capstan_config::toml_refusal;
use capstan_wire::js;
use common::*;

/// The text the loader hands the parser: the bytes as UTF-8 less one byte order mark, with CRLF as LF.
fn loader_source(bytes: &[u8]) -> Option<String> {
    let text = std::str::from_utf8(bytes).ok()?;
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    Some(text.replace("\r\n", "\n"))
}

fn refused(text: &str) -> Option<(usize, usize)> {
    toml_refusal(text).map(|p| (p.line, p.column))
}

#[test]
fn the_cases_node_refused_are_refused_at_the_same_place() {
    // Each position was read from smol-toml 1.8.0 itself.
    let cases: &[(&str, Option<(usize, usize)>)] = &[
        ("a = 1\n", None),
        ("a = \n", Some((1, 5))),
        ("a = 1\na = 2\n", Some((2, 1))),
        ("[a]\n[a]\n", Some((2, 2))),
        ("a = 'x\n", Some((1, 7))),
        ("a = \"x\ny\"\n", Some((1, 7))),
        ("a = tru\n", Some((1, 5))),
        ("a = 01\n", Some((1, 5))),
        ("a = 1__0\n", Some((1, 5))),
        ("a = [1, 2\n", Some((1, 9))),
        ("a = {b = 1, b = 2}\n", Some((1, 13))),
        ("a.b = 1\na = 2\n", Some((2, 1))),
        ("\u{feff}a = 1\n", Some((1, 1))),
        ("a = 1979-13-27\n", Some((1, 5))),
        ("a = \"\\q\"\n", Some((1, 7))),
        ("# a\u{7}\n", Some((1, 4))),
        ("a = \"\u{1f600}\" x\n", Some((1, 10))),
    ];
    for (text, want) in cases {
        assert_eq!(refused(text), *want, "{text:?}");
    }
}

#[test]
fn every_text_of_the_positions_file_is_refused_where_node_refused_it() {
    let Ok(file) = std::env::var("CAPSTAN_CONFIG_POSITIONS") else {
        eprintln!("CAPSTAN_CONFIG_POSITIONS is not set: the positions run is skipped");
        return;
    };
    let (mut total, mut mismatches) = (0, Vec::new());
    for line in std::fs::read_to_string(file).unwrap().lines() {
        let case = js::parse(line).unwrap();
        let index = number(&case, "index").unwrap() as usize;
        let kind = text(&case, "kind").unwrap();
        if kind == "utf8" {
            continue;
        }
        let source = loader_source(&base64(&text(&case, "base64").unwrap())).unwrap();
        total += 1;
        let got = refused(&source);
        let want = match kind.as_str() {
            "ok" => None,
            "toml" => Some((
                number(&case, "line").unwrap() as usize,
                number(&case, "column").unwrap() as usize,
            )),
            other => panic!("kind {other}"),
        };
        if got != want {
            mismatches.push(format!("#{index}: rust {got:?}, node {want:?}"));
        }
    }
    eprintln!("positions: {total} texts, {} differ", mismatches.len());
    for example in mismatches.iter().take(30) {
        eprintln!("  {example}");
    }
    assert!(mismatches.is_empty(), "{} texts differ", mismatches.len());
}
