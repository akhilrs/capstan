use capstan_parity_overlay::load;
use serde_json::json;
use std::path::Path;

fn cases_of(file: &Path) -> Result<Vec<String>, String> {
    let text = std::fs::read_to_string(file).map_err(|e| e.to_string())?;
    let value: serde_json::Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    Ok(value["cases"]
        .as_array()
        .ok_or("no cases")?
        .iter()
        .filter_map(|c| c.as_str().map(str::to_string))
        .collect())
}

struct Setup {
    root: tempfile::TempDir,
}

impl Setup {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("fixtures/nested")).unwrap();
        std::fs::create_dir_all(root.path().join("overlays")).unwrap();
        std::fs::write(
            root.path().join("fixtures/a.json"),
            r#"{"cases": ["one", "two"]}"#,
        )
        .unwrap();
        std::fs::write(
            root.path().join("fixtures/nested/b.json"),
            r#"{"cases": ["three"]}"#,
        )
        .unwrap();
        Self { root }
    }

    fn overlay(&self, name: &str, value: serde_json::Value) {
        std::fs::write(
            self.root.path().join("overlays").join(name),
            serde_json::to_string(&value).unwrap(),
        )
        .unwrap();
    }

    fn load(&self) -> Result<capstan_parity_overlay::Overlays, String> {
        load(
            &self.root.path().join("overlays"),
            &self.root.path().join("fixtures"),
            &cases_of,
        )
        .map_err(|e| e.to_string())
    }
}

fn good(fixture: &str, case: &str) -> serde_json::Value {
    json!({"fixture": fixture, "case": case, "expected": {"out": 1}, "reason": "Rust runs the cstan"})
}

#[test]
fn overlays_are_found_by_fixture_and_case() {
    let setup = Setup::new();
    setup.overlay("x.json", good("a.json", "two"));
    setup.overlay("y.json", good("nested/b.json", "three"));
    let overlays = setup.load().unwrap();
    assert_eq!(overlays.len(), 2);
    assert_eq!(overlays.expected("a.json", "two"), Some(&json!({"out": 1})));
    assert_eq!(overlays.expected("a.json", "one"), None);
    assert_eq!(
        overlays.get("nested/b.json", "three").unwrap().reason,
        "Rust runs the cstan"
    );
}

#[test]
fn a_missing_directory_is_no_overlays() {
    let setup = Setup::new();
    let none = load(
        &setup.root.path().join("absent"),
        &setup.root.path().join("fixtures"),
        &cases_of,
    )
    .unwrap();
    assert!(none.is_empty());
}

#[test]
fn an_overlay_naming_a_missing_fixture_or_case_is_rejected() {
    let setup = Setup::new();
    setup.overlay("x.json", good("gone.json", "one"));
    let message = setup.load().unwrap_err();
    assert!(
        message.contains("x.json") && message.contains("fixture gone.json which does not exist"),
        "{message}"
    );
    setup.overlay("x.json", good("a.json", "nine"));
    let message = setup.load().unwrap_err();
    assert!(message.contains("has no case \"nine\""), "{message}");
    setup.overlay("x.json", good("../fixtures/a.json", "one"));
    assert!(setup.load().unwrap_err().contains("must be a path inside"));
}

#[test]
fn an_overlay_with_no_reason_is_rejected() {
    let setup = Setup::new();
    for reason in [json!(""), json!("  \n")] {
        let mut overlay = good("a.json", "one");
        overlay["reason"] = reason;
        setup.overlay("x.json", overlay);
        assert!(setup.load().unwrap_err().contains("has no reason"));
    }
    let mut overlay = good("a.json", "one");
    overlay.as_object_mut().unwrap().remove("reason");
    setup.overlay("x.json", overlay);
    assert!(setup.load().unwrap_err().contains("missing field `reason`"));
}

#[test]
fn two_overlays_for_one_case_are_rejected() {
    let setup = Setup::new();
    setup.overlay("x.json", good("a.json", "one"));
    setup.overlay("y.json", good("a.json", "one"));
    let message = setup.load().unwrap_err();
    assert!(
        message.contains("y.json")
            && message.contains("already replaces")
            && message.contains("x.json"),
        "{message}"
    );
}

#[test]
fn an_unknown_field_is_rejected() {
    let setup = Setup::new();
    let mut overlay = good("a.json", "one");
    overlay["extra"] = json!(1);
    setup.overlay("x.json", overlay);
    assert!(setup.load().unwrap_err().contains("unknown field `extra`"));
}
