//! The `### This project` block the `[design]` keys add to the designer's prompt, and the architect's note: exact
//! texts, the sizes they stay within, and their agreement with the built-in guidance they refer to.
use capstan_config::design_guidance::{architect_note, project_block, record_block};
use capstan_config::prompts::{DESIGNER_BUILTIN_PROMPT, DESIGNER_PROMPT, DESIGN_GUIDANCE};
use capstan_config::{parse_config_with_home, ConfigError, Design};

const BASE: &str = "schema_version = 1\n[nexora]\ntrack = \"never\"\n[hosts.claude]\nkind = \"claude\"\n[roles.pm]\nkind = \"PM\"\nhost = \"claude\"\n[roles.designer]\nkind = \"Developer\"\nhost = \"claude\"\nprompt = \"mine\"\n";

fn design(extra: &str) -> Design {
    let dir = tempfile::tempdir().unwrap();
    parse_config_with_home(
        format!("{BASE}{extra}").as_bytes(),
        dir.path(),
        Some("/home/someone"),
    )
    .unwrap_or_else(|error| match error {
        ConfigError::Invalid(message) => panic!("{message}"),
        other => panic!("{other:?}"),
    })
    .design
}

const HEAD: &str = "### This project\nThese values replace the defaults in the guidance above.";
const DIRECTIONS: &str = "- Directions: offer 2 to 3; always one safe and one stretch.";
const EXTEND: &str = "- Rules: extend (the guidance adds to the Avoid/Require list of your prompt, if it has one, which wins where they differ).";
const OVERRIDE: &str = "- Rules: override (where the guidance or the project rule files differ from the Avoid/Require list of your prompt, they win; this replaces the rule above that the list wins).";
const FILES: &str =
    "- Project files: docs/design/directions.md, docs/design/rules.md (read those that exist).";
const RECORD: &str = "- Design record: DESIGN.md; mode maintain. Wherever the guidance says DESIGN.md, it means these paths (one per surface).";
const MAINTAIN: &str =
    "A design package that owns a record path writes or updates it, in the commit it reports for approval.";
const READ: &str = "Read it; no package owns it, so never change it.";
const OFF: &str = "Ignore it: skip every step of your prompt and of the guidance that reads or writes DESIGN.md or these paths.";
const NO_COLLECTION: &str =
    "- Reference collection: none configured; read one only when the PM names its path.";
const PATHS: &str = "- Paths: before you read or write a path above, check that it resolves inside the repository (for example with realpath); if it does not, leave it and tell the PM.";

fn block(lines: &[&str]) -> Option<String> {
    Some(lines.join("\n"))
}

#[test]
fn every_default_written_adds_no_block() {
    let written = design(
        "[design]\nrequire_approval = true\ndesign_record = \"DESIGN.md\"\ndesign_record_mode = \"maintain\"\nguidance = \"builtin\"\nguidance_rules = \"extend\"\nguidance_extra = [\"docs/design/directions.md\", \"docs/design/rules.md\"]\ndirections_min = 2\ndirections_max = 3\n",
    );
    assert_eq!(project_block(&written), None);
    assert_eq!(record_block(&written), None);
    assert_eq!(project_block(&Design::default()), None);
}

#[test]
fn guidance_on_or_off_alone_adds_no_block() {
    for mode in ["on", "off"] {
        let resolved = design(&format!("[design]\nguidance = \"{mode}\"\n"));
        assert_eq!(project_block(&resolved), None, "{mode}");
        assert_eq!(record_block(&resolved), None, "{mode}");
    }
}

#[test]
fn directions_max_four_changes_one_line() {
    let resolved = design("[design]\ndirections_max = 4\n");
    assert_eq!(
        project_block(&resolved),
        block(&[
            HEAD,
            "- Directions: offer 2 to 4; always one safe and one stretch.",
            EXTEND,
            FILES,
            RECORD,
            MAINTAIN,
            NO_COLLECTION,
            PATHS,
        ])
    );
    assert_eq!(record_block(&resolved), None);
}

#[test]
fn rules_override_replaces_the_rules_line() {
    let resolved = design("[design]\nguidance_rules = \"override\"\n");
    assert_eq!(
        project_block(&resolved),
        block(&[
            HEAD,
            DIRECTIONS,
            OVERRIDE,
            FILES,
            RECORD,
            MAINTAIN,
            NO_COLLECTION,
            PATHS
        ])
    );
}

#[test]
fn guidance_extra_lists_the_written_files() {
    let resolved = design("[design]\nguidance_extra = [\"docs/a.md\"]\n");
    let text = project_block(&resolved).unwrap();
    assert!(text.contains("\n- Project files: docs/a.md (read those that exist).\n"));
    let none = design("[design]\nguidance_extra = []\n");
    assert!(project_block(&none)
        .unwrap()
        .contains("\n- Project files: none.\n"));
}

#[test]
fn mode_read_and_mode_off_state_who_owns_the_record() {
    let read = design("[design]\ndesign_record_mode = \"read\"\n");
    assert_eq!(
        project_block(&read),
        block(&[
            HEAD,
            DIRECTIONS,
            EXTEND,
            FILES,
            "- Design record: DESIGN.md; mode read. Wherever the guidance says DESIGN.md, it means these paths (one per surface).",
            READ,
            NO_COLLECTION,
            PATHS,
        ])
    );
    let off = design("[design]\ndesign_record_mode = \"off\"\n");
    assert_eq!(
        project_block(&off),
        block(&[
            HEAD,
            DIRECTIONS,
            EXTEND,
            FILES,
            "- Design record: DESIGN.md; mode off. Wherever the guidance says DESIGN.md, it means these paths (one per surface).",
            OFF,
            NO_COLLECTION,
            PATHS,
        ])
    );
}

#[test]
fn two_record_paths_are_listed_in_order() {
    let resolved =
        design("[design]\ndesign_record = [\"docs/web/DESIGN.md\", \"docs/app/DESIGN.md\"]\n");
    assert!(project_block(&resolved).unwrap().contains(
        "\n- Design record: docs/web/DESIGN.md, docs/app/DESIGN.md; mode maintain. Wherever"
    ));
}

#[test]
fn guidance_off_keeps_only_the_record_lines() {
    let read =
        design("[design]\nguidance = \"off\"\ndesign_record_mode = \"read\"\ndirections_max = 5\n");
    let expected = block(&[
        "### This project",
        "- Design record: DESIGN.md; mode read. Wherever your prompt or the guidance says DESIGN.md, it means these paths (one per surface).",
        READ,
        PATHS,
    ]);
    assert_eq!(project_block(&read), expected);
    assert_eq!(record_block(&read), expected);
    let maintain = design("[design]\nguidance = \"off\"\ndesign_record = \"docs/DESIGN.md\"\n");
    assert_eq!(
        record_block(&maintain),
        block(&[
            "### This project",
            "- Design record: docs/DESIGN.md; mode maintain. Wherever your prompt or the guidance says DESIGN.md, it means these paths (one per surface).",
            MAINTAIN,
            PATHS,
        ])
    );
    // Only a record key moves the record block; a block key alone leaves nothing to say.
    let other = design("[design]\nguidance = \"off\"\ndirections_max = 5\n");
    assert_eq!(project_block(&other), None);
}

#[test]
fn guidance_off_with_mode_off_tells_the_designer_to_ignore_the_record() {
    let resolved = design("[design]\nguidance = \"off\"\ndesign_record_mode = \"off\"\n");
    let expected = block(&[
        "### This project",
        &format!("- Design record: DESIGN.md; mode off. {OFF}"),
    ]);
    assert_eq!(project_block(&resolved), expected);
    assert_eq!(record_block(&resolved), expected);
}

#[test]
fn the_reference_collection_line_has_three_forms() {
    let absolute = design("[design]\nreference_collection = \"/opt/refs/ui\"\n");
    assert!(project_block(&absolute).unwrap().contains(
        "\n- Reference collection: /opt/refs/ui (traits only, never values or tokens). This path counts as named by the PM. Read it with your file tools; if reading is denied, tell the PM.\n"
    ));
    let home = design("[design]\nreference_collection = \"~/refs/ui\"\n");
    assert!(project_block(&home).unwrap().contains(
        "\n- Reference collection: ~/refs/ui (traits only, never values or tokens). This path counts as named by the PM. Read it with your file tools; if reading is denied, tell the PM. ~ is your home directory.\n"
    ));
    let none = design("[design]\ndirections_max = 4\n");
    assert!(project_block(&none)
        .unwrap()
        .contains(&format!("\n{NO_COLLECTION}\n")));
}

fn maximal() -> Design {
    let record: Vec<String> = (0..8)
        .map(|index| format!("{:x<100}", format!("rec{index}/")))
        .collect();
    let extra: Vec<String> = (0..8)
        .map(|index| format!("{:y<100}", format!("ext{index}/")))
        .collect();
    let list = |paths: &[String]| {
        paths
            .iter()
            .map(|path| format!("\"{path}\""))
            .collect::<Vec<_>>()
            .join(", ")
    };
    design(&format!(
        "[design]\ndesign_record = [{}]\nguidance_extra = [{}]\ndesign_record_mode = \"read\"\nguidance_rules = \"override\"\nreference_collection = \"~/{}\"\n",
        list(&record),
        list(&extra),
        "c".repeat(298)
    ))
}

#[test]
fn the_largest_block_stays_under_the_cap_and_the_prompts_within_their_bounds() {
    let resolved = maximal();
    assert!(resolved.design_record.iter().all(|path| path.len() == 100));
    assert_eq!(resolved.reference_collection.as_ref().unwrap().len(), 300);
    let text = project_block(&resolved).unwrap();
    assert!(text.len() < 3072, "{} bytes", text.len());
    assert!(text.ends_with(PATHS));
    let built_in = format!("{DESIGNER_BUILTIN_PROMPT}\n{text}");
    assert!(built_in.len() <= 16384, "{} bytes", built_in.len());
    let appended = format!("\n{DESIGN_GUIDANCE}\n{text}");
    assert!(appended.len() <= 10240, "{} bytes", appended.len());
}

#[test]
fn every_block_with_a_path_to_read_ends_with_the_path_check() {
    for extra in [
        "[design]\ndirections_max = 4\n",
        "[design]\nguidance = \"off\"\ndesign_record_mode = \"read\"\n",
        "[design]\nguidance = \"off\"\ndesign_record = \"docs/DESIGN.md\"\n",
    ] {
        let resolved = design(extra);
        assert!(
            project_block(&resolved).unwrap().ends_with(PATHS),
            "{extra}"
        );
    }
    let off = design("[design]\ndesign_record_mode = \"off\"\n");
    assert!(!record_block(&off).unwrap().contains("- Paths:"));
}

#[test]
fn the_built_in_text_is_the_designer_text_and_the_guidance() {
    assert_eq!(
        DESIGNER_BUILTIN_PROMPT,
        format!("{DESIGNER_PROMPT}\n{DESIGN_GUIDANCE}")
    );
    assert_eq!(DESIGNER_PROMPT.len() + DESIGN_GUIDANCE.len(), 12108);
}

#[test]
fn the_guidance_still_says_what_the_block_replaces() {
    let phrases = [
        "Offer 2 or 3",
        "Always one safe and one stretch",
        "the Avoid/Require list wins",
        "`docs/design/directions.md`",
        "`docs/design/rules.md`",
        "`cstan design guide <topic>`",
        "DESIGN.md at the repository root",
        "Use the DESIGN.md tokens",
        "Write or update DESIGN.md only when the package owns it",
        "read only when the PM names its path",
    ];
    for phrase in phrases {
        assert!(DESIGN_GUIDANCE.contains(phrase), "main.md lost: {phrase}");
    }
    let resolved = design("[design]\ndirections_max = 4\nreference_collection = \"/opt/refs\"\n");
    let text = project_block(&resolved).unwrap();
    assert!(text.contains("Wherever the guidance says DESIGN.md, it means these paths"));
    assert!(text.contains("This path counts as named by the PM"));
    // The defaults the block repeats are the ones the guidance states.
    let base = Design::default();
    assert_eq!((base.directions_min, base.directions_max), (2, 3));
    assert_eq!(
        base.guidance_extra,
        ["docs/design/directions.md", "docs/design/rules.md"]
    );
    assert_eq!(base.design_record, ["DESIGN.md"]);
}

#[test]
fn the_architect_note_follows_the_record_keys() {
    assert_eq!(architect_note(&Design::default()), None);
    assert_eq!(
        architect_note(&design("[design]\ndirections_max = 4\n")),
        None
    );
    assert_eq!(
        architect_note(&design(
            "[design]\ndesign_record = [\"docs/web/DESIGN.md\", \"docs/app/DESIGN.md\"]\n"
        )),
        Some("In this project the design record is docs/web/DESIGN.md, docs/app/DESIGN.md (mode maintain). The exception above applies to each of these paths: exactly one design package of the plan owns each path it changes.".to_string())
    );
    assert_eq!(
        architect_note(&design("[design]\ndesign_record_mode = \"read\"\n")),
        Some("In this project the design record is DESIGN.md (mode read). Design packages read the record and never change it, so no package owns a record path.".to_string())
    );
    assert_eq!(
        architect_note(&design("[design]\ndesign_record_mode = \"off\"\n")),
        Some("In this project the design record is DESIGN.md (mode off). Designers ignore the design record, so no package owns a record path.".to_string())
    );
}
