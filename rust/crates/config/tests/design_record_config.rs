//! The design record and guidance keys of `[design]`: defaults, accepted values, exact refusals and warnings, the
//! confinement of paths to the project, the printed JSON and the prompts the keys must not move.
use capstan_config::starter::STARTER_CONFIG;
use capstan_config::{
    parse_config, parse_config_with_home, sha256, ConfigError, Design, GuidanceMode, GuidanceRules,
    RecordMode, RoleConfig,
};
use std::path::Path;

const BASE: &str = r#"schema_version = 1
[nexora]
track = "never"
[hosts.claude]
kind = "claude"
[roles.pm]
kind = "PM"
host = "claude"
[roles.designer]
kind = "Developer"
host = "claude"
"#;

fn load_in(root: &Path, home: Option<&str>, extra: &str) -> Result<RoleConfig, String> {
    parse_config_with_home(format!("{BASE}{extra}").as_bytes(), root, home).map_err(|error| {
        match error {
            ConfigError::Invalid(message) => message,
            other => format!("{other:?}"),
        }
    })
}

fn load(extra: &str) -> Result<RoleConfig, String> {
    let dir = tempfile::tempdir().unwrap();
    load_in(dir.path(), Some("/home/someone"), extra)
}

fn design(extra: &str) -> Design {
    load(extra).unwrap().design
}

fn refusal(extra: &str) -> String {
    load(extra).unwrap_err()
}

#[test]
fn no_table_and_an_empty_table_give_the_defaults() {
    for extra in ["", "[design]\n"] {
        let resolved = design(extra);
        assert_eq!(resolved.configured, !extra.is_empty());
        assert!(!resolved.customised);
        assert_eq!(resolved.design_record, ["DESIGN.md"]);
        assert_eq!(resolved.design_record_mode, RecordMode::Maintain);
        assert_eq!(resolved.guidance, GuidanceMode::Builtin);
        assert_eq!(resolved.guidance_rules, GuidanceRules::Extend);
        assert_eq!(
            resolved.guidance_extra,
            ["docs/design/directions.md", "docs/design/rules.md"]
        );
        assert_eq!((resolved.directions_min, resolved.directions_max), (2, 3));
        assert_eq!(resolved.reference_collection, None);
        assert!(!resolved.block_keys_customised() && !resolved.record_customised());
    }
}

#[test]
fn each_key_takes_a_valid_value() {
    let resolved = design(
        "[design]\ndesign_record = [\"a.md\", \"b/c.md\"]\ndesign_record_mode = \"read\"\nguidance = \"on\"\nguidance_rules = \"override\"\nguidance_extra = [\"x.md\"]\ndirections_min = 4\ndirections_max = 6\nreference_collection = \"/srv/refs\"\n",
    );
    assert_eq!(resolved.design_record, ["a.md", "b/c.md"]);
    assert_eq!(resolved.design_record_mode, RecordMode::Read);
    assert_eq!(resolved.design_record_mode.as_str(), "read");
    assert_eq!(resolved.guidance, GuidanceMode::On);
    assert_eq!(resolved.guidance.as_str(), "on");
    assert_eq!(resolved.guidance_rules, GuidanceRules::Override);
    assert_eq!(resolved.guidance_rules.as_str(), "override");
    assert_eq!(resolved.guidance_extra, ["x.md"]);
    assert_eq!((resolved.directions_min, resolved.directions_max), (4, 6));
    assert_eq!(resolved.reference_collection.as_deref(), Some("/srv/refs"));
    assert!(resolved.customised && resolved.block_keys_customised());
    for (value, mode) in [("maintain", RecordMode::Maintain), ("off", RecordMode::Off)] {
        let resolved = design(&format!("[design]\ndesign_record_mode = \"{value}\"\n"));
        assert_eq!(resolved.design_record_mode, mode);
        assert_eq!(resolved.design_record_mode.as_str(), value);
    }
    for (value, mode) in [
        ("builtin", GuidanceMode::Builtin),
        ("off", GuidanceMode::Off),
    ] {
        assert_eq!(
            design(&format!("[design]\nguidance = \"{value}\"\n")).guidance,
            mode
        );
    }
    assert_eq!(GuidanceMode::Off.as_str(), "off");
    assert_eq!(GuidanceRules::Extend.as_str(), "extend");
    assert_eq!(
        design("[design]\nguidance_extra = []\n")
            .guidance_extra
            .len(),
        0
    );
}

#[test]
fn design_record_is_a_string_or_a_list() {
    assert_eq!(
        design("[design]\ndesign_record = \"docs/D.md\"\n").design_record,
        ["docs/D.md"]
    );
    assert_eq!(
        design("[design]\ndesign_record = [\"docs/D.md\"]\n").design_record,
        ["docs/D.md"]
    );
    assert!(design("[design]\ndesign_record = \"docs/D.md\"\n").record_customised());
    assert!(design("[design]\ndesign_record_mode = \"off\"\n").record_customised());
    assert!(!design("[design]\nguidance = \"on\"\n").record_customised());
    assert!(!design("[design]\nguidance = \"on\"\n").block_keys_customised());
    assert!(design("[design]\nguidance = \"on\"\n").customised);
}

#[test]
fn a_key_written_with_its_default_changes_nothing() {
    for body in [
        "design_record = \"DESIGN.md\"",
        "design_record = [\"DESIGN.md\"]",
        "design_record = \"./DESIGN.md\"",
        "design_record_mode = \"maintain\"",
        "guidance = \"builtin\"",
        "guidance_rules = \"extend\"",
        "guidance_extra = [\"./docs/design/directions.md\", \"docs//design/rules.md\"]",
        "directions_min = 2",
        "directions_max = 3",
        "reference_collection = \"\"",
    ] {
        let resolved = design(&format!("[design]\n{body}\n"));
        assert!(!resolved.customised, "{body}");
        assert_eq!(resolved, design("[design]\n"), "{body}");
    }
    assert!(
        design("[design]\nguidance_extra = [\"docs/design/rules.md\", \"docs/design/directions.md\"]\n")
            .customised
    );
}

#[test]
fn directions_are_checked_in_order() {
    assert_eq!(
        refusal("[design]\ndirections_min = 1\n"),
        "design.directions_min must be between 2 and 5"
    );
    assert_eq!(
        refusal("[design]\ndirections_min = 6\n"),
        "design.directions_min must be between 2 and 5"
    );
    assert_eq!(
        refusal("[design]\ndirections_max = 1\n"),
        "design.directions_max must be between 2 and 6"
    );
    assert_eq!(
        refusal("[design]\ndirections_max = 7\n"),
        "design.directions_max must be between 2 and 6"
    );
    assert_eq!(
        refusal("[design]\ndirections_min = 9\ndirections_max = 4\n"),
        "design.directions_min must be between 2 and 5"
    );
    assert_eq!(
        refusal("[design]\ndirections_min = 4\n"),
        "design.directions_min (4) is greater than design.directions_max (3); set both"
    );
    assert_eq!(
        refusal("[design]\ndirections_min = 5\ndirections_max = 4\n"),
        "design.directions_min (5) is greater than design.directions_max (4); set both"
    );
    assert_eq!(
        refusal("[design]\ndirections_min = \"2\"\n"),
        "design.directions_min must be an integer"
    );
    let resolved = design("[design]\ndirections_min = 4\ndirections_max = 4\n");
    assert_eq!((resolved.directions_min, resolved.directions_max), (4, 4));
}

#[test]
fn types_and_unknown_keys_are_refused() {
    assert_eq!(
        refusal("[design]\nguidance = \"sometimes\"\n"),
        "design.guidance must be one of builtin, on, off"
    );
    assert_eq!(
        refusal("[design]\ndesign_record_mode = 1\n"),
        "design.design_record_mode must be one of maintain, read, off"
    );
    assert_eq!(
        refusal("[design]\nguidance_rules = \"x\"\n"),
        "design.guidance_rules must be one of extend, override"
    );
    assert_eq!(
        refusal("[design]\nguidance_extra = \"a.md\"\n"),
        "design.guidance_extra must be an array of strings"
    );
    assert_eq!(
        refusal("[design]\ndesign_record = 3\n"),
        "design.design_record must be a non-empty string"
    );
    assert_eq!(
        refusal("[design]\ndesign_record = [3]\n"),
        "design.design_record[0] must be a non-empty string"
    );
    assert_eq!(
        refusal("[design]\nreference_collection = 3\n"),
        "design.reference_collection must be a non-empty string"
    );
    assert!(refusal("[design]\nother = 1\n")
        .starts_with("design has 1 unknown key(s); allowed: require_approval, design_record,"));
}

#[test]
fn path_refusals_have_exact_text() {
    for key in ["design_record", "guidance_extra"] {
        let at = |index: Option<usize>| match index {
            Some(i) => format!("design.{key}[{i}]"),
            None => format!("design.{key}"),
        };
        let cases: Vec<(&str, &str)> = vec![
            ("", "must not be empty"),
            (".", "must not be empty"),
            ("./", "must not be empty"),
            ("/", "must be relative to the project"),
            ("/etc/x", "must be relative to the project"),
            ("~x", "must be relative to the project"),
            ("~/x", "must be relative to the project"),
            ("C:x", "must be relative to the project"),
            ("../x", "must not contain .."),
            ("a/../x", "must not contain .."),
            ("a\\\\b", "must not contain a backslash"),
            ("a*b", "must not contain a glob character (* ? [ ])"),
            ("a?b", "must not contain a glob character (* ? [ ])"),
            ("a[1]", "must not contain a glob character (* ? [ ])"),
            ("dir/", "must name a file, not end with /"),
            (" a", "must not have leading or trailing whitespace"),
            (
                "a\\nb",
                "contains control, format or line-separator characters",
            ),
            (
                "a\\tb",
                "contains control, format or line-separator characters",
            ),
            (
                "a\\u0000b",
                "contains control, format or line-separator characters",
            ),
            (
                "a\\u2028b",
                "contains control, format or line-separator characters",
            ),
            (
                "a\\u200bb",
                "contains control, format or line-separator characters",
            ),
            (
                "a\\u202eb",
                "contains control, format or line-separator characters",
            ),
        ];
        for (entry, message) in cases {
            let single = format!("[design]\n{key} = [\"{entry}\"]\n");
            assert_eq!(
                refusal(&single),
                format!("{} {message}", at(Some(0))),
                "{key} {entry:?}"
            );
        }
        let long = format!("{}.md", "a".repeat(98));
        assert_eq!(long.len(), 101);
        assert_eq!(
            refusal(&format!("[design]\n{key} = [\"{long}\"]\n")),
            format!("{} must be at most 100 bytes", at(Some(0)))
        );
        let fits = format!("{}.md", "a".repeat(97));
        assert!(design(&format!("[design]\n{key} = [\"{fits}\"]\n")).customised);
        assert!(load(&format!("[design]\n{key} = [\"a..b\"]\n")).is_ok());
        assert!(load(&format!("[design]\n{key} = [\"./a//b.md\"]\n")).is_ok());
        assert_eq!(
            refusal(&format!("[design]\n{key} = [\"a.md\", \"./a.md\"]\n")),
            format!("{} repeats an earlier entry", at(Some(1)))
        );
        // Case-sensitive.
        assert!(load(&format!("[design]\n{key} = [\"a.md\", \"A.md\"]\n")).is_ok());
    }
    assert_eq!(
        refusal("[design]\ndesign_record = \"\"\n"),
        "design.design_record must not be empty"
    );
    assert_eq!(
        refusal("[design]\ndesign_record = \"../x\"\n"),
        "design.design_record must not contain .."
    );
    assert_eq!(
        design("[design]\ndesign_record = \"a..b\"\n").design_record,
        ["a..b"]
    );
}

#[test]
fn list_sizes_and_cross_list_repeats_are_refused() {
    let nine: Vec<String> = (0..9).map(|i| format!("\"f{i}.md\"")).collect();
    let eight: Vec<String> = (0..8).map(|i| format!("\"f{i}.md\"")).collect();
    assert_eq!(
        refusal(&format!(
            "[design]\ndesign_record = [{}]\n",
            nine.join(", ")
        )),
        "design.design_record must have 1 to 8 entries"
    );
    assert_eq!(
        refusal("[design]\ndesign_record = []\n"),
        "design.design_record must have 1 to 8 entries"
    );
    assert_eq!(
        refusal(&format!(
            "[design]\nguidance_extra = [{}]\n",
            nine.join(", ")
        )),
        "design.guidance_extra must have at most 8 entries"
    );
    assert_eq!(
        design(&format!(
            "[design]\ndesign_record = [{}]\n",
            eight.join(", ")
        ))
        .design_record
        .len(),
        8
    );
    assert_eq!(
        refusal("[design]\ndesign_record = [\"x.md\"]\nguidance_extra = [\"y.md\", \"./x.md\"]\n"),
        "design.guidance_extra[1] repeats a design_record entry"
    );
    assert_eq!(
        refusal("[design]\nguidance_extra = [\"DESIGN.md\"]\n"),
        "design.guidance_extra[0] repeats a design_record entry"
    );
}

#[test]
fn reference_collection_is_checked_lexically() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("proj");
    std::fs::create_dir(&project).unwrap();
    let outside = dir.path().join("refs");
    std::fs::create_dir(&outside).unwrap();
    let text = |value: &str| format!("[design]\nreference_collection = \"{value}\"\n");
    let go = |home: Option<&str>, value: &str| load_in(&project, home, &text(value));

    let resolved = go(None, outside.to_str().unwrap()).unwrap();
    assert_eq!(
        resolved.design.reference_collection.as_deref(),
        outside.to_str()
    );
    assert!(resolved
        .warnings
        .iter()
        .all(|w| !w.contains("reference_collection")));
    assert!(resolved.design.customised);

    let home = dir.path().to_str().unwrap();
    let resolved = go(Some(home), "~/refs").unwrap();
    assert_eq!(
        resolved.design.reference_collection.as_deref(),
        Some("~/refs")
    );
    assert!(resolved
        .warnings
        .iter()
        .all(|w| !w.contains("reference_collection")));
    assert_eq!(
        go(None, "~/refs").unwrap_err(),
        "design.reference_collection: ~ needs HOME"
    );
    assert_eq!(
        go(Some(""), "~/refs").unwrap_err(),
        "design.reference_collection: ~ needs HOME"
    );

    assert_eq!(
        go(None, "refs").unwrap_err(),
        "design.reference_collection must be an absolute path or start with ~/"
    );
    assert_eq!(
        go(None, "~").unwrap_err(),
        "design.reference_collection must be an absolute path or start with ~/"
    );
    assert_eq!(
        go(None, "/a/../b").unwrap_err(),
        "design.reference_collection must not contain .."
    );
    assert_eq!(
        go(None, project.to_str().unwrap()).unwrap_err(),
        "design.reference_collection must be outside the project"
    );
    assert_eq!(
        go(None, &format!("{}/sub/x", project.display())).unwrap_err(),
        "design.reference_collection must be outside the project"
    );
    assert_eq!(
        go(None, dir.path().to_str().unwrap()).unwrap_err(),
        "design.reference_collection must not contain the project"
    );
    assert_eq!(
        go(None, "/").unwrap_err(),
        "design.reference_collection must not contain the project"
    );
    let long = format!("/{}", "a".repeat(300));
    assert_eq!(
        go(None, &long).unwrap_err(),
        "design.reference_collection must be at most 300 bytes"
    );
    for bad in [
        "/a\\nb",
        "/a\\tb",
        "/a\\u0000b",
        "/a\\u2028b",
        "/a\\u200bb",
        "/a\\u202eb",
    ] {
        assert_eq!(
            go(None, bad).unwrap_err(),
            "design.reference_collection contains control, format or line-separator characters",
            "{bad:?}"
        );
    }
    assert_eq!(
        go(None, " /x").unwrap_err(),
        "design.reference_collection must not have leading or trailing whitespace"
    );

    let missing = dir.path().join("nowhere");
    let resolved = go(None, missing.to_str().unwrap()).unwrap();
    assert_eq!(
        resolved.warnings.last().unwrap(),
        &format!(
            "design.reference_collection {} does not exist",
            missing.display()
        )
    );

    // A project root with a trailing slash or a `.` segment is the same project.
    for root in [
        format!("{}/", project.display()),
        format!("{}/.", project.display()),
    ] {
        let result = load_in(Path::new(&root), None, &text(project.to_str().unwrap()));
        assert_eq!(
            result.unwrap_err(),
            "design.reference_collection must be outside the project",
            "{root}"
        );
        assert!(load_in(Path::new(&root), None, &text(outside.to_str().unwrap())).is_ok());
    }

    // An empty value is no collection.
    assert_eq!(go(None, "").unwrap().design.reference_collection, None);
}

#[cfg(unix)]
#[test]
fn symlinks_may_not_leave_the_project() {
    use std::os::unix::fs::symlink;
    for key in ["design_record", "guidance_extra"] {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("proj");
        let outside = dir.path().join("out");
        std::fs::create_dir_all(project.join("docs")).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("secret.md"), "x").unwrap();
        std::fs::write(project.join("real.md"), "x").unwrap();
        symlink(outside.join("secret.md"), project.join("file-link.md")).unwrap();
        symlink(&outside, project.join("dir-link")).unwrap();
        symlink(project.join("real.md"), project.join("inside-link.md")).unwrap();
        symlink(project.join("docs"), project.join("docs-link")).unwrap();
        symlink(project.join("gone"), project.join("dangling.md")).unwrap();
        let go = |entry: &str| {
            load_in(
                &project,
                None,
                &format!("[design]\n{key} = [\"{entry}\"]\n"),
            )
        };
        let stay = format!("design.{key}[0] must stay inside the project");
        for refused in [
            "file-link.md",
            "dir-link/secret.md",
            "dir-link/not-yet.md",
            "dir-link/sub/not-yet.md",
            "dangling.md",
        ] {
            assert_eq!(go(refused).unwrap_err(), stay, "{refused}");
        }
        for accepted in [
            "inside-link.md",
            "docs-link/new.md",
            "real.md",
            "docs/new/deeper.md",
            "nothing/at/all.md",
        ] {
            assert!(go(accepted).is_ok(), "{accepted}");
        }
    }
}

#[test]
fn the_design_object_prints_by_the_rules() {
    let json = |extra: &str| load(extra).unwrap().to_json();
    assert!(!json("").contains("\"design\""));
    let old_true =
        "\"design\": {\n    \"configured\": true,\n    \"requireApproval\": true\n  },\n  \"env\"";
    let old_false =
        "\"design\": {\n    \"configured\": true,\n    \"requireApproval\": false\n  },\n  \"env\"";
    assert!(json("[design]\n").contains(old_true));
    assert!(json("[design]\nrequire_approval = false\n").contains(old_false));
    let defaults = "[design]\ndesign_record = \"DESIGN.md\"\ndesign_record_mode = \"maintain\"\nguidance = \"builtin\"\nguidance_rules = \"extend\"\nguidance_extra = [\"docs/design/directions.md\", \"docs/design/rules.md\"]\ndirections_min = 2\ndirections_max = 3\nreference_collection = \"\"\n";
    assert!(json(defaults).contains(old_true));

    let printed = json("[design]\ndirections_max = 4\n");
    let at = printed.find("\"design\"").unwrap();
    let tail = &printed[at..];
    let end = tail.find("\n  },").unwrap();
    assert_eq!(
        &tail[..end],
        "\"design\": {\n    \"configured\": true,\n    \"requireApproval\": true,\n    \"designRecord\": [\n      \"DESIGN.md\"\n    ],\n    \"designRecordMode\": \"maintain\",\n    \"guidance\": \"builtin\",\n    \"guidanceRules\": \"extend\",\n    \"guidanceExtra\": [\n      \"docs/design/directions.md\",\n      \"docs/design/rules.md\"\n    ],\n    \"directionsMin\": 2,\n    \"directionsMax\": 4,\n    \"referenceCollection\": null"
    );
    let printed = json("[design]\nreference_collection = \"/srv/r\"\n");
    assert!(printed.contains("\"referenceCollection\": \"/srv/r\""));
}

#[test]
fn a_customised_table_without_a_designer_warns() {
    let no_designer = "schema_version = 1\n[nexora]\ntrack = \"never\"\n[hosts.claude]\nkind = \"claude\"\n[roles.pm]\nkind = \"PM\"\nhost = \"claude\"\n[roles.dev]\nkind = \"Developer\"\nhost = \"claude\"\n";
    let dir = tempfile::tempdir().unwrap();
    let warn = "design: no Developer role named designer, so the guidance and record keys change no prompt";
    let run = |extra: &str| {
        parse_config_with_home(format!("{no_designer}{extra}").as_bytes(), dir.path(), None)
            .unwrap()
    };
    assert!(run("[design]\nguidance = \"on\"\n")
        .warnings
        .iter()
        .any(|w| w == warn));
    assert!(!run("[design]\n").warnings.iter().any(|w| w == warn));
    assert!(!run("").warnings.iter().any(|w| w == warn));
    assert!(!load("[design]\nguidance = \"on\"\n")
        .unwrap()
        .warnings
        .iter()
        .any(|w| w == warn));
}

#[test]
fn the_starter_prompts_and_hashes_are_those_before_the_keys_existed() {
    // Measured on the commit before these keys were added.
    let expected = [
        (
            "pm",
            None,
            "fa75ece7d08c796c3bd82e9ec5db150dfa69c6579bc3a30cf1796a5f2c2342d5",
        ),
        (
            "developer",
            Some("32dd2ac95826bfaf889612db30f0ff0e7319138c4dedac43a57fea790c6236d7"),
            "11a4c7b68ca1be685d345ef04f655cbc5518416d62ff8109dd569aeb0313a4e7",
        ),
        (
            "designer",
            Some("bac7bf19b78e08c0a8f5eff13236aeb77f5ca96a778025ce7fd41b067a8b305a"),
            "7eeba52f4ea5abcb1dbc88a25b385748d62eac0c197029cbe90831925bafd9e5",
        ),
        (
            "reviewer",
            Some("1bff45f53bb84eb3f3abaad3237161212ebe9e219e4f6810a514d7386c219703"),
            "38f0ddac02bbd6c8607447f540bb9cf0619fbc3baeb7995faec7554bece8281b",
        ),
        (
            "tester",
            Some("2efb6688b3d6bc9442d40747908cfef48295261c8058c733457fe04f0cede593"),
            "0670d98f0f155ade00f559010c0c40e46018d6a5b2441dfd9de24ac8cd78d7cb",
        ),
        (
            "supervisor",
            Some("bb79a8706ff01587c155cee091faba272d33081f6f0df65a3f21ca85dfba3666"),
            "739977d313b2056f8bbc3b3d7c8abd1ab5c9c7d4e864a09a61a94d87615301c9",
        ),
    ];
    let config = parse_config(STARTER_CONFIG.as_bytes(), Path::new("/nonexistent")).unwrap();
    assert_eq!(config.roles.len(), expected.len());
    for (role, (name, prompt_hash, config_hash)) in config.roles.iter().zip(expected) {
        assert_eq!(role.name, name);
        assert_eq!(
            role.prompt.hash.as_deref(),
            prompt_hash,
            "{name} prompt.hash"
        );
        assert_eq!(role.config_hash, config_hash, "{name} configHash");
        assert_eq!(
            role.prompt_text.as_deref().map(sha256).as_deref(),
            prompt_hash,
            "{name} prompt_text"
        );
    }
    // The prompts and hashes the keys do change are pinned in builtin_prompt.rs and design_project_block.rs.
}
