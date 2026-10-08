//! The Researcher role's tool-rule checks (`src/researcher-policy.ts`): which rules it may allow and which it must deny.
use crate::types::{Researcher, Role};

/// Tool rules the Researcher role must deny: subagents, every way to push or rewrite history, and the curl flags that send, save or read local files.
const REQUIRED_DENY: [&str; 74] = [
    "Agent",
    "Task",
    "NotebookEdit",
    "Bash(git push)",
    "Bash(git push *)",
    "Bash(git merge *)",
    "Bash(git rebase *)",
    "Bash(git reset *)",
    "Bash(git remote *)",
    "Bash(git config *)",
    "Bash(git checkout *)",
    "Bash(git switch *)",
    "Bash(curl * -d*)",
    "Bash(curl * --data*)",
    "Bash(curl * -F*)",
    "Bash(curl * --form*)",
    "Bash(curl * -T*)",
    "Bash(curl * --upload-file*)",
    "Bash(curl * -X*)",
    "Bash(curl * --request*)",
    "Bash(curl * --json*)",
    "Bash(curl * -o*)",
    "Bash(curl * --output*)",
    "Bash(curl * -O*)",
    "Bash(curl * --remote-name*)",
    "Bash(curl * -K*)",
    "Bash(curl * --config*)",
    "Bash(curl * -u*)",
    "Bash(curl * --user*)",
    "Bash(curl * -c*)",
    "Bash(curl * --cookie-jar*)",
    "Bash(curl * -D*)",
    "Bash(curl * --dump-header*)",
    "Bash(curl * --trace*)",
    "Bash(curl * --stderr*)",
    "Bash(curl * --create-dirs*)",
    "Bash(curl * --libcurl*)",
    "Bash(curl * --hsts*)",
    "Bash(curl * --alt-svc*)",
    "Bash(curl * --etag-save*)",
    "Bash(curl * file:*)",
    "Bash(curl * @*)",
    "Bash(curl -d*)",
    "Bash(curl --data*)",
    "Bash(curl -F*)",
    "Bash(curl --form*)",
    "Bash(curl -T*)",
    "Bash(curl --upload-file*)",
    "Bash(curl -X*)",
    "Bash(curl --request*)",
    "Bash(curl --json*)",
    "Bash(curl -o*)",
    "Bash(curl --output*)",
    "Bash(curl -O*)",
    "Bash(curl --remote-name*)",
    "Bash(curl -K*)",
    "Bash(curl --config*)",
    "Bash(curl -u*)",
    "Bash(curl --user*)",
    "Bash(curl -c*)",
    "Bash(curl --cookie-jar*)",
    "Bash(curl -D*)",
    "Bash(curl --dump-header*)",
    "Bash(curl --trace*)",
    "Bash(curl --stderr*)",
    "Bash(curl --create-dirs*)",
    "Bash(curl --libcurl*)",
    "Bash(curl --hsts*)",
    "Bash(curl --alt-svc*)",
    "Bash(curl --etag-save*)",
    "Bash(curl *%output{*)",
    "Bash(curl file:*)",
    "Bash(curl @*)",
    "Bash(git * --output*)",
];

const FIXED_ALLOW: [&str; 12] = [
    "WebSearch",
    "WebFetch",
    "Bash(jq *)",
    "Bash(date*)",
    "Bash(git status*)",
    "Bash(git diff*)",
    "Bash(git log*)",
    "Bash(git show *)",
    "Bash(git rev-parse *)",
    "Bash(git add *)",
    "Bash(git commit *)",
    "Bash(cstan *)",
];

/// `/^WebFetch\(domain:[A-Za-z0-9][A-Za-z0-9.-]*\)$/`.
fn web_fetch_domain(rule: &str) -> bool {
    let Some(host) = rule
        .strip_prefix("WebFetch(domain:")
        .and_then(|rest| rest.strip_suffix(')'))
    else {
        return false;
    };
    match host.as_bytes().split_first() {
        Some((head, tail)) => {
            head.is_ascii_alphanumeric()
                && tail
                    .iter()
                    .all(|b| b.is_ascii_alphanumeric() || *b == b'.' || *b == b'-')
        }
        None => false,
    }
}

/// ``/^Bash\(curl [^`$;&|<>\\()\n]*\)$/``.
fn curl_rule(rule: &str) -> bool {
    rule.strip_prefix("Bash(curl ")
        .and_then(|rest| rest.strip_suffix(')'))
        .is_some_and(|middle| {
            !middle.chars().any(|c| {
                matches!(
                    c,
                    '`' | '$' | ';' | '&' | '|' | '<' | '>' | '\\' | '(' | ')' | '\n'
                )
            })
        })
}

fn mcp_class(b: u8, upper: bool) -> bool {
    b.is_ascii_lowercase()
        || b.is_ascii_digit()
        || b == b'_'
        || b == b'-'
        || (upper && b.is_ascii_uppercase())
}

/// `/^mcp__([a-z][a-z0-9_-]*)__([A-Za-z0-9_-]+)$/`: the server and the tool.
fn mcp_rule(rule: &str) -> Option<(&str, &str)> {
    let rest = rule.strip_prefix("mcp__")?;
    let bytes = rest.as_bytes();
    if !bytes.first()?.is_ascii_lowercase() {
        return None;
    }
    let reach = 1 + bytes[1..]
        .iter()
        .take_while(|b| mcp_class(**b, false))
        .count();
    // The server is greedy and gives characters back until "__" and a non-empty tool follow.
    for end in (1..=reach).rev() {
        let tail = &rest[end..];
        if let Some(tool) = tail.strip_prefix("__") {
            if !tool.is_empty() && tool.bytes().all(|b| mcp_class(b, true)) {
                return Some((&rest[..end], tool));
            }
        }
    }
    None
}

/// `/^mcp__[a-z][a-z0-9_-]*$/`.
fn mcp_bare(rule: &str) -> bool {
    rule.strip_prefix("mcp__").is_some_and(|rest| {
        rest.as_bytes().split_first().is_some_and(|(head, tail)| {
            head.is_ascii_lowercase() && tail.iter().all(|b| mcp_class(*b, false))
        })
    })
}

/// `/upload|run_code|evaluate|install|file/i`.
fn forbidden_mcp_tool(tool: &str) -> bool {
    let lower = tool.to_ascii_lowercase();
    ["upload", "run_code", "evaluate", "install", "file"]
        .iter()
        .any(|word| lower.contains(word))
}

fn allow_problem(rule: &str, output_dir: &str, servers: &[&str]) -> Option<String> {
    if FIXED_ALLOW.contains(&rule)
        || web_fetch_domain(rule)
        || curl_rule(rule)
        || rule == format!("Write({output_dir}/**)")
        || rule == format!("Edit({output_dir}/**)")
    {
        return None;
    }
    if let Some((server, tool)) = mcp_rule(rule) {
        if !servers.contains(&server) {
            return Some(format!(
                "names MCP server {server}, which is not in this role's mcp list"
            ));
        }
        if forbidden_mcp_tool(tool) {
            return Some(format!(
                "must not allow the MCP tool {tool}: tools that upload, run code, evaluate scripts, install or touch files are refused"
            ));
        }
        return None;
    }
    if mcp_bare(rule) {
        return Some(format!(
            "must name one tool, not a whole server; the researcher role may not allow {rule}"
        ));
    }
    Some(format!(
        "is not on the researcher allowlist; the researcher role may not allow {rule}"
    ))
}

/// Readable problems with a role used as the Researcher; empty when it passes.
pub fn researcher_rule_problems(role: &Role, researcher: &Researcher) -> Vec<String> {
    let at = format!("roles.{}", role.name);
    let mut problems = Vec::new();
    if role.kind != "Developer" {
        problems.push(format!(
            "researcher.role \"{}\" must be a Developer role; {at}.kind is {}",
            role.name, role.kind
        ));
    }
    if role.permission_mode != "default" {
        problems.push(format!(
            "{at}.permission_mode must be \"default\" for the researcher role; it is \"{}\"",
            role.permission_mode
        ));
    }
    let servers: Vec<&str> = role.mcp.iter().map(|server| server.name.as_str()).collect();
    for (index, rule) in role.allow.iter().enumerate() {
        if let Some(problem) = allow_problem(rule, &researcher.output_dir, &servers) {
            problems.push(format!("{at}.allow[{index}] {problem}"));
        }
    }
    for required in REQUIRED_DENY {
        if !role.deny.iter().any(|rule| rule == required) {
            problems.push(format!(
                "{at}.deny must include {required}: the researcher role may not start subagents, push, or send or save data with curl"
            ));
        }
    }
    problems
}
