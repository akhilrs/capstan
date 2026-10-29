//! The decisions section of `docs/remote-protocol.md` and the CLI: its synopsis line is the usage entry of `cstan decide`
//! and it names every code the command gives itself.

use cstan_front::decide::CLI_ERROR_CODES;
use cstan_front::usage::ENTRIES;

const DOC: &str = include_str!("../../../../docs/remote-protocol.md");

/// The text between the backticks of the line of section 11 that starts with `Synopsis: `.
fn synopsis(doc: &str) -> Result<String, String> {
    let start = doc
        .find("\n## 11. Decisions\n")
        .ok_or("the document has no section 11")?;
    doc[start..]
        .lines()
        .find_map(|line| line.strip_prefix("Synopsis: `")?.strip_suffix('`'))
        .map(str::to_string)
        .ok_or_else(|| "section 11 has no synopsis line".to_string())
}

fn usage_of_decide() -> &'static str {
    let mut entries = ENTRIES.iter().filter(|entry| entry.command == "decide");
    let entry = entries.next().expect("the table has a decide entry");
    assert!(entries.next().is_none(), "one decide entry");
    entry.syntax
}

fn check_synopsis(doc: &str) -> Result<(), String> {
    let listed = synopsis(doc)?;
    if listed == usage_of_decide() {
        Ok(())
    } else {
        Err(format!(
            "synopsis `{listed}`, usage `{}`",
            usage_of_decide()
        ))
    }
}

#[test]
fn the_synopsis_is_the_usage_entry_of_decide() {
    check_synopsis(DOC).unwrap();
}

#[test]
fn a_changed_synopsis_fails() {
    let changed = DOC.replacen("[--client <label>]", "[--client <name>]", 1);
    assert_ne!(changed, DOC);
    assert!(check_synopsis(&changed).is_err());
    let missing = DOC.replacen("Synopsis: `", "Usage: `", 1);
    assert!(check_synopsis(&missing).is_err());
}

#[test]
fn section_11_names_every_code_of_the_command() {
    let section = &DOC[DOC.find("\n## 11. Decisions\n").expect("section 11")..];
    for code in CLI_ERROR_CODES {
        assert!(section.contains(&format!("`{code}`")), "code {code}");
    }
}
