//! The text the `[design]` keys add to the designer's prompt and to the architect's: a `### This project` block that
//! replaces the defaults of the built-in guidance, and a note for the architect about who owns the design record.
use crate::design::{Design, GuidanceMode, GuidanceRules, RecordMode};

const HEADING: &str = "### This project";
const PATHS_LINE: &str = "- Paths: before you read or write a path above, check that it resolves inside the repository (for example with realpath); if it does not, leave it and tell the PM.";

/// The `### This project` block for a designer that takes the guidance: None unless a key other than `guidance` is not
/// at its default. With `guidance = "off"` only the design record lines remain.
pub fn project_block(design: &Design) -> Option<String> {
    if design.guidance == GuidanceMode::Off {
        return record_block(design);
    }
    if !design.block_keys_customised() {
        return None;
    }
    let rules = match design.guidance_rules {
        GuidanceRules::Extend => "- Rules: extend (the guidance adds to the Avoid/Require list of your prompt, if it has one, which wins where they differ).",
        GuidanceRules::Override => "- Rules: override (where the guidance or the project rule files differ from the Avoid/Require list of your prompt, they win; this replaces the rule above that the list wins).",
    };
    let extra = if design.guidance_extra.is_empty() {
        "- Project files: none.".to_string()
    } else {
        format!(
            "- Project files: {} (read those that exist).",
            design.guidance_extra.join(", ")
        )
    };
    let mut lines = vec![
        HEADING.to_string(),
        "These values replace the defaults in the guidance above.".to_string(),
        format!(
            "- Directions: offer {} to {}; always one safe and one stretch.",
            design.directions_min, design.directions_max
        ),
        rules.to_string(),
        extra,
        format!(
            "- Design record: {}; mode {}. Wherever the guidance says DESIGN.md, it means these paths (one per surface).",
            design.design_record.join(", "),
            design.design_record_mode.as_str()
        ),
        mode_line(design.design_record_mode).to_string(),
        collection_line(design),
        PATHS_LINE.to_string(),
    ];
    lines.retain(|line| !line.is_empty());
    Some(lines.join("\n"))
}

/// The block for a designer that does not take the guidance (its own prompt with `guidance` builtin or off, or the
/// built-in prompt with `guidance = "off"`): the design record lines only. None while the record keys are at their
/// defaults.
pub fn record_block(design: &Design) -> Option<String> {
    if !design.record_customised() {
        return None;
    }
    let paths = design.design_record.join(", ");
    let mode = design.design_record_mode;
    if mode == RecordMode::Off {
        return Some(format!(
            "{HEADING}\n- Design record: {paths}; mode off. {}",
            OFF_SENTENCE
        ));
    }
    Some(format!(
        "{HEADING}\n- Design record: {paths}; mode {}. Wherever your prompt or the guidance says DESIGN.md, it means these paths (one per surface).\n{}\n{PATHS_LINE}",
        mode.as_str(),
        mode_line(mode)
    ))
}

const OFF_SENTENCE: &str = "Ignore it: skip every step of your prompt and of the guidance that reads or writes DESIGN.md or these paths.";

fn mode_line(mode: RecordMode) -> &'static str {
    match mode {
        RecordMode::Maintain => "A design package that owns a record path writes or updates it, in the commit it reports for approval.",
        RecordMode::Read => "Read it; no package owns it, so never change it.",
        RecordMode::Off => OFF_SENTENCE,
    }
}

fn collection_line(design: &Design) -> String {
    match design.reference_collection.as_deref() {
        Some(path) => format!(
            "- Reference collection: {path} (traits only, never values or tokens). This path counts as named by the PM. Read it with your file tools; if reading is denied, tell the PM.{}",
            if path.starts_with("~/") {
                " ~ is your home directory."
            } else {
                ""
            }
        ),
        None => "- Reference collection: none configured; read one only when the PM names its path."
            .to_string(),
    }
}

/// What the architect needs to know about the design record: None while the record keys are at their defaults, which
/// the architect's own reference text already describes.
pub fn architect_note(design: &Design) -> Option<String> {
    if !design.record_customised() {
        return None;
    }
    let rule = match design.design_record_mode {
        RecordMode::Maintain => "The exception above applies to each of these paths: exactly one design package of the plan owns each path it changes.",
        RecordMode::Read => "Design packages read the record and never change it, so no package owns a record path.",
        RecordMode::Off => "Designers ignore the design record, so no package owns a record path.",
    };
    Some(format!(
        "In this project the design record is {} (mode {}). {rule}",
        design.design_record.join(", "),
        design.design_record_mode.as_str()
    ))
}
