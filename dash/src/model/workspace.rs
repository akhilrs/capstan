//! Workspace projects: integration sets, per-repo commits and the worktrees an agent holds.
//!
//! A project without `[repos]` sends none of these members and none of this code adds a row, a field or a word.
use serde_json::Value;

use super::build::{field, get, list, num, text, text_of, text_or_null};
use super::types::{
    AgentWorktree, MemberVerify, PipelineItem, ReportCommit, SetGate, SetMember, SetRow,
};

/// The worktrees an agent row lists, in the order the status sends them.
pub(super) fn agent_worktrees(agent: &Value) -> Vec<AgentWorktree> {
    list(get(agent, "worktrees"))
        .iter()
        .map(|w| AgentWorktree {
            repo: text_of(w, "repo"),
            path: text_of(w, "path"),
            branch: text_of(w, "branch"),
        })
        .collect()
}

/// The commit of each repo a report names; empty unless it names more than one repo.
pub(super) fn report_commits(report: &Value) -> Vec<ReportCommit> {
    let commits: Vec<ReportCommit> = list(get(report, "commits"))
        .iter()
        .map(|c| ReportCommit {
            repo: text_of(c, "repo"),
            commit_sha: text_of(c, "commitSha"),
            branch: text_of(c, "branch"),
        })
        .collect();
    if commits.len() > 1 {
        commits
    } else {
        Vec::new()
    }
}

/// `developer-4 3 repos 9f3c2a1`: the agent, the repo count and the first commit.
pub(super) fn label_multi_report(agent: &str, commits: &[ReportCommit]) -> String {
    let first = commits
        .first()
        .map(|c| c.commit_sha.chars().take(7).collect::<String>())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "-".to_string());
    format!("{agent} {} repos {first}", commits.len())
}

fn verify_of(value: Option<&Value>) -> Option<MemberVerify> {
    let v = value.filter(|v| v.is_object())?;
    // A queued run is a run that has not finished: it reads as running.
    let state = match text_of(v, "state").as_str() {
        "passed" => "passed",
        "failed" | "interrupted" => "failed",
        _ => "running",
    };
    Some(MemberVerify {
        state: state.to_string(),
        commit_sha: text_or_null(get(v, "commitSha")),
        last_passed: text_or_null(get(v, "lastPassed")),
    })
}

/// One set of `status.integrationSets`, or `None` when it has no id.
pub(super) fn set_row(set: &Value) -> Option<SetRow> {
    let set_id = text_of(set, "setId");
    if set_id.is_empty() {
        return None;
    }
    let set_state = text_of(set, "state");
    let mut has_integration: Vec<(i64, bool)> = list(get(set, "members"))
        .iter()
        .map(|m| {
            (
                num(get(m, "position")) as i64,
                field(m, "integrationId").is_some(),
            )
        })
        .collect();
    has_integration.sort_by_key(|(p, _)| *p);
    let has_integration: Vec<bool> = has_integration.into_iter().map(|(_, h)| h).collect();
    let mut members: Vec<SetMember> = list(get(set, "members"))
        .iter()
        .map(|m| SetMember {
            repo: text_of(m, "repo"),
            position: num(get(m, "position")) as i64,
            attempt: num(get(m, "attempt")) as i64,
            state: Some(text_of(m, "state"))
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "running".to_string()),
            head_sha: text_or_null(get(m, "headSha")).filter(|s| !s.is_empty()),
            dropped: get(m, "dropped") == Some(&Value::Bool(true)),
            next: false,
            review_subject: false,
            verify: verify_of(get(m, "verify")),
        })
        .collect();
    members.sort_by_key(|m| m.position);
    let live_of = |m: &SetMember| !m.dropped && m.state != "discarded";
    let live = members.iter().filter(|m| live_of(m)).count() as i64;
    let in_main = members
        .iter()
        .filter(|m| live_of(m) && m.state == "confirmed")
        .count() as i64;
    let built = members
        .iter()
        .filter(|m| live_of(m) && (m.state == "merged" || m.state == "confirmed"))
        .count() as i64;
    let first = |state: &str| {
        members
            .iter()
            .find(|m| live_of(m) && m.state == state)
            .map(|m| m.repo.clone())
    };
    let discarded = set_state == "discarded" || live == 0;
    let (word, summary) = if discarded {
        ("discarded", "discarded".to_string())
    } else if let Some(repo) = first("conflicted") {
        ("conflicted", format!("conflicted: {repo}"))
    } else if let Some(repo) = first("failed") {
        ("failed", format!("failed: {repo}"))
    } else if in_main == live {
        ("in main", format!("in main {live}/{live}"))
    } else if in_main > 0 {
        ("partly in main", format!("{in_main}/{live} in main"))
    } else if built == live {
        ("built", format!("built {live}/{live}"))
    } else {
        ("building", format!("building {built}/{live} built"))
    };
    // The host merges one member at a time: the first live one that is not in main, once the set is merged.
    let decidable = matches!(word, "built" | "partly in main");
    let next = members
        .iter()
        .position(|m| live_of(m) && m.state != "confirmed")
        .filter(|_| decidable);
    // The review subject is the first live member that has an integration (the host reads the same).
    let subject = members
        .iter()
        .zip(&has_integration)
        .position(|(m, has)| live_of(m) && *has);
    if let Some(i) = next {
        members[i].next = true;
    }
    if let Some(i) = subject {
        members[i].review_subject = true;
    }
    let gate = field(set, "gate").and_then(|g| {
        let state = text_of(g, "state");
        (!state.is_empty()).then(|| SetGate {
            state,
            repos: get(g, "repos")
                .and_then(Value::as_array)
                .map(|a| a.iter().map(|r| text(Some(r))).collect())
                .unwrap_or_default(),
        })
    });
    Some(SetRow {
        set_id,
        branch: text_of(set, "branch"),
        word: word.to_string(),
        summary,
        live,
        members,
        gate,
    })
}

/// The item of a set: one pipeline row, its state the set word.
pub(super) fn set_item(set: SetRow, created_at: String) -> PipelineItem {
    let label = format!(
        "{} {}",
        set.set_id.chars().take(8).collect::<String>(),
        set.summary
    );
    // Every displayed field (gate, member verify, head, attempt, review subject, next) is part of the row.
    let shown = serde_json::to_string(&set).unwrap_or_default();
    PipelineItem {
        id: format!("set:{}", set.set_id),
        stage: "set".to_string(),
        fingerprint: format!("{}|{label}|{shown}", set.word),
        state: set.word.clone(),
        label,
        created_at,
        set: Some(set),
        commits: Vec::new(),
    }
}
