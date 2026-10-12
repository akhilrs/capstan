//! Id prefixes in command arguments: `dispatch` resolves the record ids of a command's arguments here, once, before the
//! handler runs, so no handler knows about prefixes. A unique prefix of at least four characters becomes the full id
//! (see `capstan_kernel::areas::id_prefix`); an ambiguous or too short one is refused; any other argument goes on as it
//! is. Agent ids, plan ids and package ids are not resolved: they keep exact matching.

use capstan_kernel::areas::id_prefix::{refusal, resolve_id, IdColumn, Resolved};

use super::shared::{fail, map_kernel_error, CommandEnv, CommandResponse, ErrorCode};

const MESSAGES: &[IdColumn] = &[("messages", "message_id")];
const REPORTS: &[IdColumn] = &[("agent_reports", "report_id")];
const INTEGRATIONS: &[IdColumn] = &[("integrations", "integration_id")];
const REPORT_OR_INTEGRATION: &[IdColumn] = &[
    ("agent_reports", "report_id"),
    ("integrations", "integration_id"),
];
const FINDINGS: &[IdColumn] = &[("agent_findings", "finding_id")];
const RELAYS: &[IdColumn] = &[("prompt_relays", "relay_id")];
const PROPOSALS: &[IdColumn] = &[("operator_proposals", "proposal_id")];
const GRANTS: &[IdColumn] = &[("operator_grants", "grant_id")];
const CLEANUPS: &[IdColumn] = &[("cleanups", "cleanup_id")];
const RUNS: &[IdColumn] = &[("verify_runs", "run_id")];

/// The arguments of `command` that hold record ids, with the tables each may name.
fn id_arguments(command: &str, args: &[String]) -> Vec<(usize, &'static [IdColumn])> {
    let sub = args.first().map(String::as_str);
    match (command, sub) {
        ("ack", _) => (0..args.len()).map(|i| (i, MESSAGES)).collect(),
        ("resolve" | "cancel", _) => vec![(0, MESSAGES)],
        ("request-review", _) => vec![(0, REPORT_OR_INTEGRATION)],
        ("integrate", Some("confirm" | "discard")) => vec![(1, INTEGRATIONS)],
        ("integrate", _) => (0..args.len()).map(|i| (i, REPORTS)).collect(),
        ("finding", Some("check")) => vec![(1, FINDINGS)],
        ("prompt", Some("answer")) => vec![(1, RELAYS)],
        ("op", Some("decide" | "show" | "cancel")) => vec![(1, PROPOSALS)],
        ("op", Some("revoke")) => vec![(1, GRANTS)],
        ("cleanup", Some("apply" | "decline")) => vec![(1, CLEANUPS)],
        ("plan", Some("signoff")) => vec![(2, INTEGRATIONS)],
        ("verify", Some("show" | "cancel")) => vec![(1, RUNS)],
        ("verify", Some("list")) => vec![(1, REPORT_OR_INTEGRATION)],
        ("verify", Some(_)) => vec![(0, REPORT_OR_INTEGRATION)],
        _ => Vec::new(),
    }
}

/// Replaces each id argument of `command` that is a unique prefix with the full id. `Err` holds the refusal to answer with.
pub fn resolve_arguments(
    env: &CommandEnv<'_>,
    command: &str,
    args: &mut [String],
) -> Result<(), CommandResponse> {
    for (index, targets) in id_arguments(command, args) {
        let Some(id) = args.get(index).cloned() else {
            continue;
        };
        if id.starts_with("--") {
            continue;
        }
        let resolved = env
            .deps
            .kernel
            .run(move |core| resolve_id(core.kernel(), targets, &id).map(|r| (id, r)))
            .map_err(|error| map_kernel_error(&error))?;
        match resolved {
            (_, Resolved::Unchanged) => {}
            (_, Resolved::Full(full)) => args[index] = full,
            (id, other) => return Err(fail(ErrorCode::InvalidRequest, refusal(&id, &other))),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(text: &str) -> Vec<String> {
        text.split(' ').map(str::to_string).collect()
    }

    #[test]
    fn id_positions_follow_each_commands_syntax() {
        let at = |c: &str, a: &str| {
            id_arguments(c, &words(a))
                .into_iter()
                .map(|(i, _)| i)
                .collect::<Vec<_>>()
        };
        assert_eq!(at("ack", "m1"), [0]);
        assert_eq!(at("ack", "m1 m2"), [0, 1]);
        assert_eq!(at("integrate", "r1 r2"), [0, 1]);
        assert_eq!(at("integrate", "confirm i1"), [1]);
        assert_eq!(at("op", "decide p1 approve"), [1]);
        assert_eq!(at("op", "grants"), Vec::<usize>::new());
        assert_eq!(at("plan", "signoff p1 i1 done"), [2]);
        assert_eq!(at("plan", "show p1"), Vec::<usize>::new());
        assert_eq!(at("send", "a1 hi"), Vec::<usize>::new());
        assert_eq!(at("verify", "r1"), [0]);
        assert_eq!(at("verify", "show run1"), [1]);
        assert_eq!(at("verify", "cancel run1"), [1]);
        assert_eq!(at("verify", "list r1"), [1]);
    }
}
