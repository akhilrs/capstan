//! The design record ownership rule and the PM's handling of design-record problems are in the built prompts and in
//! docs/reference/design-approval.md.
use capstan_config::prompts::*;

const DESIGN_SENTENCE: &str = "implementation packages depend on it.";
const OWNERSHIP_SENTENCE: &str = "The one exception to owning only those files: when a design needs the project's design record (`DESIGN.md` at the repository root), exactly one design package of the plan owns the record path; any other design package that needs record changes lists them under `## Design record changes` in its own design note and never edits the record, the owning package depends on those packages and applies their changes before it reports, no implementation package owns a record path, and an implementation package that finds a problem in the record reports it to the PM.";
const S1: &str = "When a design package reports, also show the user its `## Rule deviations` and `## Design record changes` sections: a rule deviation never blocks approval, the user decides.";
const S2: &str = "When the designer reports that the design record and the design system disagree, ask the user which one wins and send the answer to the designer.";
const S3: &str = "When an implementation developer reports a problem in the design record, do not record Changes needed on your own: ask the user whether the design should change, and only if they want a change record their words with `cstan design changes <plan-id>/<package-id> --asked-user` on the design package that owns the record; otherwise send the problem to the architect to investigate or to plan a new design package. While a design is being revised, tell the developers of already assigned packages that depend on it to stop building the affected parts until the revised design is approved.";

fn worker_roles() -> Vec<WorkerRole> {
    vec![
        WorkerRole {
            name: "developer".into(),
            kind: "Developer".into(),
        },
        WorkerRole {
            name: "reviewer".into(),
            kind: "Verifier".into(),
        },
    ]
}

fn architect_input() -> ArchitectInput {
    ArchitectInput {
        role: "architect".into(),
        high_risk_triggers: vec!["schema or migrations".into()],
        design_note: None,
    }
}

fn base(kind: Kind, id: &str) -> PromptInput {
    let mut input = PromptInput::new("x", kind, id, 90.0);
    input.role_prompt = Some("Be brief.".into());
    input.worker_roles = Some(worker_roles());
    input
}

fn architect_prompt() -> String {
    let mut input = base(Kind::Developer, "architect-1");
    input.is_architect = true;
    input.architect = Some(architect_input());
    build_role_prompt(&input).unwrap()
}

fn pm_prompt() -> String {
    let mut input = base(Kind::Pm, "pm-1");
    input.architect = Some(architect_input());
    build_role_prompt(&input).unwrap()
}

#[test]
fn the_architect_prompt_carries_the_ownership_sentence_once_after_the_design_sentence() {
    let prompt = architect_prompt();
    assert_eq!(prompt.matches(OWNERSHIP_SENTENCE).count(), 1);
    assert!(prompt.contains(&format!("{DESIGN_SENTENCE} {OWNERSHIP_SENTENCE}")));
    assert!(OWNERSHIP_SENTENCE.contains("`DESIGN.md`"));
    assert!(!OWNERSHIP_SENTENCE.contains("design_record"));
    assert!(prompt.len() <= MAX_PROMPT_BYTES);
}

#[test]
fn no_other_role_carries_the_ownership_sentence() {
    let mut operator = base(Kind::Developer, "operator-1");
    operator.is_operator = true;
    operator.operator = Some(OperatorInput {
        role: "operator".into(),
        auto_approve: vec!["ls -l".into()],
    });
    let mut researcher = base(Kind::Developer, "researcher-1");
    researcher.is_researcher = true;
    researcher.researcher = Some(ResearcherInput {
        role: "researcher".into(),
        output_dir: "research".into(),
        user_agent: "agent".into(),
    });
    let mut developer = base(Kind::Developer, "developer-1");
    developer.architect = Some(architect_input());
    for input in [
        developer,
        base(Kind::Verifier, "reviewer-1"),
        base(Kind::Supervisor, "supervisor-1"),
        operator,
        researcher,
    ] {
        let prompt = build_role_prompt(&input).unwrap();
        assert!(
            !prompt.contains("exactly one design package of the plan owns the record path"),
            "{}",
            input.agent_id
        );
    }
}

#[test]
fn the_pm_prompt_carries_the_three_design_record_sentences_in_order() {
    let prompt = pm_prompt();
    let mut at = 0;
    for sentence in [S1, S2, S3] {
        assert_eq!(prompt.matches(sentence).count(), 1, "{sentence}");
        let found = prompt.find(sentence).unwrap();
        assert!(found >= at, "out of order: {sentence}");
        at = found;
    }
    assert!(prompt.len() <= MAX_PROMPT_BYTES);
}

#[test]
fn the_architect_overlays_were_read_and_hold_the_full_prompt() {
    let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/divergences");
    let mut seen = 0;
    for entry in std::fs::read_dir(directory).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        if !name.starts_with("prompt-") || !name.ends_with(".json") {
            continue;
        }
        let overlay: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let text = overlay["expected"]["text"].as_str().unwrap();
        assert_eq!(text.matches(OWNERSHIP_SENTENCE).count(), 1, "{name}");
        assert_eq!(
            overlay["expected"]["bytes"].as_u64(),
            Some(text.len() as u64)
        );
        assert_eq!(overlay["fixture"], "prompts-expected.json");
        seen += 1;
    }
    assert!(seen >= 1, "no prompt overlays");
}

#[test]
fn the_design_approval_page_describes_the_flow() {
    let page = std::fs::read_to_string(format!(
        "{}/../../../docs/reference/design-approval.md",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap();
    for topic in [
        "main",
        "rules",
        "directions-product",
        "directions-brand",
        "redesign",
        "proposal",
        "record",
    ] {
        let command = format!("cstan design guide {topic}");
        assert!(page.contains(&command), "{command}");
    }
    for phrase in [
        "DESIGN.md",
        "rule deviation",
        "token references resolve",
        "contrast pairs pass AA by a computed check",
        "no unused tokens",
        "## Design record ownership",
        "owning only `docs/design/<plan-id>-<package-id>*` (one exception: the design record, see Design record ownership)",
        "exactly one design package of the plan owns the record path",
        "never edits the record",
        "Implementation packages never edit DESIGN.md",
        "A rule deviation never blocks the report",
        "traits only, never values or tokens",
        "Changes needed is always the user's words",
    ] {
        assert!(page.contains(phrase), "{phrase}");
    }
    // The page names the keys once, in the ownership section, and links the reference.
    assert_eq!(page.matches("`design_record_mode`").count(), 1);
    assert!(page.contains("(configuration.md#design)"));
}

const NOTE: &str = "In this project the design record is docs/web/DESIGN.md, docs/app/DESIGN.md (mode read). Design packages read the record and never change it, so no package owns a record path.";

fn with_note(mut input: PromptInput) -> String {
    input.architect.as_mut().unwrap().design_note = Some(NOTE.into());
    build_role_prompt(&input).unwrap()
}

#[test]
fn a_design_note_follows_the_architect_reference_once() {
    let mut input = base(Kind::Developer, "architect-1");
    input.is_architect = true;
    input.architect = Some(architect_input());
    let without = build_role_prompt(&input).unwrap();
    let prompt = with_note(input);
    assert_eq!(prompt.matches(NOTE).count(), 1);
    // Taking the note out gives the prompt without it, byte for byte.
    assert_eq!(prompt.replacen(&format!("{NOTE}\n\n"), "", 1), without);
    let at = prompt.find(NOTE).unwrap();
    assert!(prompt[..at].contains("cstan plan amend <plan-id>"));
    assert!(
        at < prompt.find("Be brief.").unwrap(),
        "before the role prompt"
    );
}

#[test]
fn the_design_note_reaches_no_other_prompt() {
    let mut note = architect_input();
    note.design_note = Some(NOTE.into());
    let mut pm = base(Kind::Pm, "pm-1");
    pm.architect = Some(note.clone());
    let mut developer = base(Kind::Developer, "developer-1");
    developer.architect = Some(note.clone());
    let mut reviewer = base(Kind::Verifier, "reviewer-1");
    reviewer.architect = Some(note);
    for input in [pm, developer, reviewer] {
        let prompt = build_role_prompt(&input).unwrap();
        assert!(!prompt.contains(NOTE), "{}", input.agent_id);
    }
}

#[test]
fn without_a_design_note_the_architect_prompt_is_unchanged() {
    let prompt = architect_prompt();
    assert!(!prompt.contains("In this project the design record is"));
}
