//! The texts the controller and the PM prompt share about Operator proposals decided from a remote client. The
//! kernel depends on this crate, so these constants are the single source of the wording.

/// First line of every picker about an Operator proposal (the main one and the follow-ups).
pub const OPERATOR_PICKER_QUESTION: &str = "Operator proposal <proposal-id> hash <hash12>";
/// Lead of the info message sent to the PM when the user approved a proposal on the phone.
pub const PM_REMOTE_APPROVED_LEAD: &str = "Operator proposal <id> was approved from remote client";
/// Lead of the info message sent to the PM when the user denied a proposal on the phone.
pub const PM_REMOTE_DENIED_LEAD: &str = "Operator proposal <id> was denied from remote client";
/// Lead of the message that forwards an approve-once chosen on the phone.
pub const FORWARD_APPROVE_ONCE_LEAD: &str =
    "The user chose approve once for <requestId> in a remote client";

/// Replaces every occurrence of each placeholder (matched verbatim, for example `<id>`) with its value.
pub fn fill(template: &str, values: &[(&str, &str)]) -> String {
    let mut text = template.to_string();
    for (placeholder, value) in values {
        text = text.replace(placeholder, value);
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fill_replaces_every_occurrence() {
        assert_eq!(
            fill(PM_REMOTE_DENIED_LEAD, &[("<id>", "op-7")]),
            "Operator proposal op-7 was denied from remote client"
        );
        assert_eq!(fill("<a> <a> <b>", &[("<a>", "x"), ("<b>", "y")]), "x x y");
    }
}
