//! `cstan design guide [<topic>]`: the built-in designer guidance, printed from the binary. It needs no daemon, project or
//! agent token and writes nothing. The texts live in `capstan-config` (`src/prompts/design_guidance/`) and are embedded
//! with `include_str!` through a path that leaves this crate, so `cstan-front` is built only inside this workspace; a build
//! of the crate on its own is unsupported.

/// (topic, one-line description, text), in the order the list prints them.
pub const GUIDANCE_TOPICS: &[(&str, &str, &str)] = &[
    (
        "main",
        "How a designer works here, and which topic to read when",
        include_str!("../../config/src/prompts/design_guidance/main.md"),
    ),
    (
        "rules",
        "The rules every design must follow",
        include_str!("../../config/src/prompts/design_guidance/rules.md"),
    ),
    (
        "directions-product",
        "Visual directions for product and application interfaces",
        include_str!("../../config/src/prompts/design_guidance/directions-product.md"),
    ),
    (
        "directions-brand",
        "Visual directions for brand and marketing pages",
        include_str!("../../config/src/prompts/design_guidance/directions-brand.md"),
    ),
    (
        "redesign",
        "How to improve an existing interface without starting over",
        include_str!("../../config/src/prompts/design_guidance/redesign.md"),
    ),
    (
        "proposal",
        "What a design proposal contains before it is built",
        include_str!("../../config/src/prompts/design_guidance/proposal.md"),
    ),
    (
        "record",
        "How to record the decisions of a finished design",
        include_str!("../../config/src/prompts/design_guidance/record.md"),
    ),
];

/// One line per topic: `<topic>  <description>`.
pub fn topic_list() -> String {
    let mut out = String::new();
    for (topic, about, _) in GUIDANCE_TOPICS {
        out.push_str(&format!("{topic}  {about}\n"));
    }
    out
}

/// The text of one topic, if it exists.
pub fn topic_text(topic: &str) -> Option<&'static str> {
    GUIDANCE_TOPICS
        .iter()
        .find(|(name, _, _)| *name == topic)
        .map(|(_, _, text)| *text)
}
