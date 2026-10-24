//! The design guidance texts appended to the built-in designer prompt and read by `cstan design guide`: their shape,
//! so an edit cannot drop a direction, a rule or a pointer the prompt relies on.
use std::path::PathBuf;

fn read(name: &str) -> String {
    let path: PathBuf = [
        env!("CARGO_MANIFEST_DIR"),
        "src",
        "prompts",
        "design_guidance",
        name,
    ]
    .iter()
    .collect();
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn headings(text: &str) -> Vec<&str> {
    text.lines().filter_map(|l| l.strip_prefix("## ")).collect()
}

const PRODUCT: [&str; 8] = [
    "Ledger",
    "Control Room",
    "Workbench",
    "Instrument",
    "Briefing",
    "Wayfinder",
    "Conversation",
    "Handbook",
];
const BRAND: [&str; 8] = [
    "Poster",
    "Quiet Luxury",
    "Storefront",
    "Playground",
    "Zine",
    "Field Notes",
    "Plain Speech",
    "Throwback",
];

#[test]
fn each_directions_file_has_its_eight_entries() {
    assert_eq!(headings(&read("directions-product.md")), PRODUCT);
    assert_eq!(headings(&read("directions-brand.md")), BRAND);
}

#[test]
fn each_entry_has_its_fields_in_order() {
    for (file, names) in [
        ("directions-product.md", PRODUCT),
        ("directions-brand.md", BRAND),
    ] {
        let text = read(file);
        let mut entries: Vec<&str> = text.split("\n## ").skip(1).collect();
        assert_eq!(entries.len(), names.len());
        for (name, entry) in names.iter().zip(entries.drain(..)) {
            let mut at = 0;
            for field in 1..=15 {
                let tag = format!("\nF{field}:");
                let found = entry[at..]
                    .find(&tag)
                    .unwrap_or_else(|| panic!("{name}: F{field} missing or out of order"));
                at += found + tag.len();
            }
            let has_16 = entry.contains("\nF16:");
            assert_eq!(has_16, *name == "Throwback", "{name}: F16");
        }
    }
}

#[test]
fn rules_holds_each_of_the_62_ids_once() {
    let text = read("rules.md");
    let mut ids = Vec::new();
    for (prefix, count) in [
        ('T', 8),
        ('C', 7),
        ('L', 9),
        ('I', 6),
        ('P', 6),
        ('K', 7),
        ('M', 5),
        ('R', 7),
    ] {
        ids.extend((1..=count).map(|n| format!("{prefix}{n}")));
    }
    ids.extend(["A1", "A2", "A3", "A4", "A5", "A5b", "A6"].map(String::from));
    assert_eq!(ids.len(), 62);
    for id in &ids {
        let line = format!("\n{id}. ");
        assert_eq!(text.matches(&line).count(), 1, "rule {id}");
    }
    let rule_lines = text
        .lines()
        .filter(|l| {
            l.split_once(". ").is_some_and(|(id, _)| {
                id.len() >= 2
                    && id.chars().next().unwrap().is_ascii_uppercase()
                    && id[1..].chars().all(|c| c.is_ascii_alphanumeric())
                    && id[1..].starts_with(|c: char| c.is_ascii_digit())
            })
        })
        .count();
    assert_eq!(rule_lines, 62);
}

#[test]
fn the_files_stay_within_their_size_limits() {
    for (file, limit) in [
        ("main.md", 14 * 1024),
        ("directions-product.md", 16 * 1024),
        ("directions-brand.md", 16 * 1024),
        ("rules.md", 12 * 1024),
        ("proposal.md", 4 * 1024),
        ("record.md", 4 * 1024),
        ("redesign.md", 4 * 1024),
    ] {
        let size = read(file).len();
        assert!(size <= limit, "{file}: {size} bytes over {limit}");
    }
}

#[test]
fn main_names_the_design_note_headings_and_deviations() {
    let main = read("main.md");
    for heading in [
        "## Rule deviations",
        "## Design record changes",
        "## Kept, moved, changed, removed",
    ] {
        assert!(main.contains(heading), "{heading}");
    }
    assert!(main.contains("rule deviation"));
}

#[test]
fn main_points_at_the_project_files_and_the_command() {
    let main = read("main.md");
    for needle in [
        "docs/design/directions.md",
        "docs/design/rules.md",
        "cstan design guide",
        "Avoid/Require",
    ] {
        assert!(main.contains(needle), "{needle}");
    }
    assert!(
        main.contains("the Avoid/Require list in your prompt differ, the Avoid/Require list wins"),
        "the precedence sentence"
    );
}

#[test]
fn main_indexes_every_direction_and_every_rule_group() {
    let main = read("main.md");
    for name in PRODUCT.iter().chain(BRAND.iter()) {
        let line = format!("\n- {name}: ");
        assert_eq!(main.matches(&line).count(), 1, "catalog line for {name}");
    }
    for group in [
        "Typography",
        "Colour",
        "Layout",
        "Imagery",
        "Copy",
        "Motion",
        "Components",
        "Record",
        "Accessibility",
    ] {
        let line = format!("\n{group}: ");
        assert_eq!(
            main.matches(&line).count(),
            1,
            "rule index line for {group}"
        );
    }
    assert!(main.len() <= 14 * 1024, "{} bytes", main.len());
}
