//! The one table of `cstan` commands, and everything printed from it: `cstan help`, `cstan help <command>`, the short hint
//! after a usage error and the `cstan.1` man page (`cstan help --man`). Each entry is a group, the command word it
//! belongs to, its syntax and a description, so the three outputs cannot drift apart. Add a command here and it shows up
//! in all of them.

/// One command line of the CLI.
pub struct Entry {
    pub group: &'static str,
    /// The command word the entry belongs to (`cstan help <word>` and the usage hint list every entry with it).
    pub command: &'static str,
    /// What follows `cstan`, as typed.
    pub syntax: &'static str,
    pub about: &'static str,
}

const fn e(
    group: &'static str,
    command: &'static str,
    syntax: &'static str,
    about: &'static str,
) -> Entry {
    Entry {
        group,
        command,
        syntax,
        about,
    }
}

const SETUP: &str = "Setup & daemon";
const RUN: &str = "Run control";
const STATUS: &str = "Status & inspection";
const MESSAGING: &str = "Messaging";
const WORKERS: &str = "Workers";
const REVIEW: &str = "Review & integration";
const PLANS: &str = "Plans";
const LINKS: &str = "Nexora links";
const SUPERVISION: &str = "Supervision";
const RELAY: &str = "Prompt relay";
const OPERATOR: &str = "Operator";
const CACHE: &str = "Tool caches";

/// Every command, in the order `cstan help` lists them. The groups run together.
pub const ENTRIES: &[Entry] = &[
    e(SETUP, "init", "init [--git] [--track-config]", "Create .capstan/, a starter capstan.toml and the capstan block of .gitignore."),
    e(SETUP, "init", "init --gitignore [--track-config]", "Only add or update the capstan block of .gitignore (also in an initialized project)."),
    e(SETUP, "start", "start", "Start the controller daemon and launch the PM."),
    e(SETUP, "stop", "stop", "Stop the controller daemon."),
    e(SETUP, "ping", "ping", "Check that the controller answers."),
    e(SETUP, "config", "config check", "Validate capstan.toml."),
    e(SETUP, "config", "config sync", "Write the role definitions of capstan.toml into the ledger."),
    e(SETUP, "herdr-config", "herdr-config", "Print an optional Herdr config.toml snippet for the sidebar."),
    e(SETUP, "pm", "pm restart", "Replace the PM session, seeded from the ledger."),
    e(RUN, "pause", "pause [<agent-id>] --reason <text> [--interrupt]", "Hold one agent, or the whole run when no agent is named. --interrupt also stops the agent's current turn."),
    e(RUN, "resume", "resume [<agent-id>] --reason <text>", "Release a pause."),
    e(STATUS, "status", "status [--json] [--all]", "Show the project state: the active agents only, and no empty sections; --all adds the agents that ended."),
    e(STATUS, "status", "status --watch [--interval <seconds>]", "Redraw the state every 1 to 60 seconds (default 2)."),
    e(STATUS, "dash", "dash [--interval <seconds>] [--no-color] [--reduced-motion]", "Open the read-only terminal dashboard (cstan-dash)."),
    e(STATUS, "inspect", "inspect <id> [--json]", "Show one ledger record."),
    e(STATUS, "cancel", "cancel <id> [--json]", "Cancel a queued message."),
    e(STATUS, "observe", "observe <agent-id> [lines]", "Show the recent screen of another agent (PM and Supervisor)."),
    e(MESSAGING, "inbox", "inbox [<agent-id>]", "Read your pending messages; the operator names the agent whose mailbox to read."),
    e(MESSAGING, "ack", "ack <message-id>...|--all-read", "Acknowledge messages you read; --all-read acknowledges every message already shown to you."),
    e(MESSAGING, "wait", "wait", "Block until a new message arrives (agents)."),
    e(MESSAGING, "wait", "wait --until <duration>", "Keep waiting past the host's limit until mail arrives or the duration (1s to 10m, e.g. 5m) passes; prints \"no new messages\" and exits 0 on timeout."),
    e(MESSAGING, "send", "send [--info|--action] <agent-id>|@pm <text>", "Queue a message. --action needs an action and wakes the PM; --info is for awareness and is acknowledged by the next inbox. Without a flag the [messages] send_default decides."),
    e(MESSAGING, "report", "report <commit> <summary>", "Report a finished commit (full 40-character id) and a one-line summary (workers)."),
    e(MESSAGING, "ask", "ask", "Reserved. The controller answers not_implemented."),
    e(MESSAGING, "assign", "assign", "Reserved. The controller answers not_implemented."),
    e(MESSAGING, "resolve", "resolve <message-id> retry|skip|cancel [<note>]", "Settle a blocked message."),
    e(WORKERS, "spawn", "spawn <role> [--task <plan-id>/<package-id> | --task <requirement-ref-id>] [--type <type>] [--title <text>]", "Start a worker of a role; --task names its branch after the task."),
    e(WORKERS, "release", "release <agent-id>", "End a worker and free its pane and worktree."),
    e(WORKERS, "replace", "replace <agent-id>", "Replace a lost or stuck worker with a new one."),
    e(REVIEW, "request-review", "request-review <report-or-integration-id> [role]", "Start an independent review of a report or an integration."),
    e(REVIEW, "review", "review pass|findings <text>", "Reviewer: answer the review request once."),
    e(REVIEW, "integrate", "integrate <report-id>...", "Squash reviewed reports onto an integration branch."),
    e(REVIEW, "integrate", "integrate confirm|discard <integration-id>", "Keep a reviewed integration, or throw it away."),
    e(PLANS, "plan", "plan open normal|high-risk <title> [<superseded-plan-id>]", "Open a plan, optionally superseding an earlier one."),
    e(PLANS, "plan", "plan submit <plan-id> <json>", "Submit the plan's packages as JSON."),
    e(PLANS, "plan", "plan amend <plan-id> <json>", "Amend the plan after a review whose findings were all mechanical: add owned paths and dependency edges as JSON (reason, packages of id/add_owns/add_depends_on, integration_order); no new review round."),
    e(REVIEW, "cleanup", "cleanup --dry-run", "List the local branches in three groups (landed, unlanded, active or protected) with the commit, worktree and reason of each. Changes nothing (PM and operator)."),
    e(REVIEW, "cleanup", "cleanup", "Offer the landed branches (and the unlanded ones when [cleanup] include_unlanded is set) for deletion and print the cleanup id. Nothing is deleted yet."),
    e(REVIEW, "cleanup", "cleanup apply <cleanup-id> [<branch>...]", "Delete the landed branches of an offer, or exactly the named ones; prints each deleted branch with its commit and the git branch command that restores it."),
    e(REVIEW, "cleanup", "cleanup decline <cleanup-id>", "Decline an offer; nothing is deleted."),
    e(PLANS, "plan", "plan show [<plan-id>] [--brief]", "Show a plan, or the plans of the project; --brief prints one line per plan, or per package of one plan."),
    e(PLANS, "plan", "plan assign <plan-id> <package-id> <agent-id>", "Assign a package to a worker."),
    e(PLANS, "plan", "plan signoff <plan-id> <integration-id> <summary>", "Sign the plan off on a reviewed integration."),
    e(PLANS, "plan", "plan cancel <plan-id> [<package-id>]", "Cancel a plan, or one of its packages."),
    e(LINKS, "link", "link requirement|plan|package <ref-id> <nexora-id> [<state>]", "Link a requirement, plan or package to a Nexora item."),
    e(LINKS, "link", "link bind <requirement-ref-id> <agent-id>", "Bind a requirement to the agent working on it."),
    e(SUPERVISION, "finding", "finding <agent-id> <severity> <evidence> <correction> <done-when>", "Raise a finding against an agent (Supervisor)."),
    e(SUPERVISION, "finding", "finding check <finding-id> resolved|unresolved <evidence>", "Record whether a finding was resolved (Supervisor)."),
    e(RELAY, "prompt", "prompt show <agent-id>", "Show the prompt an agent is stuck on."),
    e(RELAY, "prompt", "prompt answer <relay-id> --hash <hash12> option <n> | esc | text <text> [--session auto=60|auto=session|auto=<n>]", "Answer the prompt; --session opens an auto-approve window with an allow-once option."),
    e(OPERATOR, "op", "op propose <command> <reason>", "Propose a shell command for approval."),
    e(OPERATOR, "op", "op propose --restart [--force] <reason>", "Propose restarting the controller."),
    e(OPERATOR, "op", "op decide <proposal-id> approve --hash <hash12> [--session exact|--session prefix=<words>] [--session auto=60|auto=session|auto=<n>]", "Approve a proposal, optionally for the rest of the session or an auto-approve window."),
    e(OPERATOR, "op", "op decide <proposal-id> deny [<note>]", "Deny a proposal."),
    e(OPERATOR, "op", "op show [<proposal-id>]", "Show a proposal, or the pending ones."),
    e(OPERATOR, "op", "op cancel <proposal-id>", "Withdraw a proposal."),
    e(OPERATOR, "op", "op grants", "List the session grants and any auto-approve window."),
    e(OPERATOR, "op", "op revoke <grant-id>", "Revoke a session grant or end an auto-approve window."),
    e(OPERATOR, "op", "op full-auto on [<minutes>] --asked-user <text>", "Let the operator run commands without asking, for a while."),
    e(OPERATOR, "op", "op full-auto off", "End full-auto mode."),
    e(OPERATOR, "op", "op full-auto status", "Show whether full-auto mode is on."),
    e(REVIEW, "verify", "verify <report-or-integration-id>", "Queue a gate run at the commit of a report or integration and print the run id."),
    e(REVIEW, "verify", "verify show <run-id>", "Show a run: state, commit, commands, exit code, failed step, duration, output tail and log path."),
    e(REVIEW, "verify", "verify list [<subject-id>]", "List the runs of a report or integration, or the latest runs."),
    e(REVIEW, "verify", "verify cancel <run-id>", "Cancel a queued or running gate run."),
    e(REVIEW, "design", "design approve <plan-id>/<package-id> --link <url-or-path> --asked-user <text>", "Record the user's approval of a design package's latest report, so packages that depend on it can be assigned (PM and operator)."),
    e(REVIEW, "design", "design changes <plan-id>/<package-id> --asked-user <text> [--link <url>]", "Record that the user asked for changes; the designer is told what to revise and report again (PM and operator)."),
    e(REVIEW, "design", "design show <plan-id>[/<package-id>]", "Show the design packages of a plan with their decisions and whether the gate is open."),
    e(CACHE, "cache", "cache status [--json]", "List the tool caches of [cache] with state, size, files, last use, root and scope. Measured now; works without the daemon."),
    e(CACHE, "cache", "cache prune [<entry>...] [--dry-run]", "Run the tools' prune commands and the age and size limits; entries are removed only while no agent worktree exists. --dry-run lists what would happen and changes nothing (PM and operator)."),
    e(CACHE, "cache", "cache clear <entry>...|--all [--force]", "Remove cache entries (setup/<key> names one setup snapshot). Refused while agents have worktrees, and for scope user, unless --force (PM and operator)."),
];

/// What `cstan herdr-config` prints.
pub const HERDR_CONFIG_SNIPPET: &str = "# Paste into Herdr's config.toml (Capstan never edits it), then run: herdr server reload-config\n# Capstan reports a $project token on every pane and workspace it creates.\n[ui.sidebar.agents]\nrows = [[\"state_icon\", \"$project\", \"workspace\"], [\"agent\"]]\n\n[ui.sidebar.spaces]\nrows = [[\"state_icon\", \"workspace\"], [\"$project\", \"branch\", \"git_status\"]]\n";

/// The columns of `cstan help`.
pub const WIDTH: usize = 80;
const INDENT: usize = 2;
/// A syntax up to this long shares its line with the description; a longer one gets a line of its own.
const SYNTAX_COLUMN: usize = 28;

const ID_PREFIX_NOTE_MAN: &str = ".PP\nWhere a command takes the id of a report, review, integration, message, finding, relay, proposal, grant or cleanup, a unique prefix of at least four characters is enough, as in git. An ambiguous prefix is refused with the candidates listed. Agent, plan and package ids must be given in full.\n";

const NAME_LINE: &str = "cstan - run a team of coding agents in a git repository";
const SYNOPSIS_LINE: &str = "usage: cstan <command> [args]";

/// Words of `text` as lines of at most `width` characters; a word longer than that stays whole on its own line.
fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for word in text.split_whitespace() {
        match lines.last_mut() {
            Some(line) if line.len() + 1 + word.len() <= width => {
                line.push(' ');
                line.push_str(word);
            }
            _ => lines.push(word.to_string()),
        }
    }
    lines
}

/// `lines` with `pad` spaces before each.
fn indented(lines: &[String], pad: usize) -> String {
    lines.iter().map(|l| format!("{:pad$}{l}\n", "")).collect()
}

/// One entry in the grouped listing: the syntax and the description side by side when the syntax is short, else the
/// syntax (wrapped at spaces) and the description under it.
fn listing_entry(entry: &Entry) -> String {
    let description_at = INDENT + SYNTAX_COLUMN + 2;
    let about = wrap(entry.about, WIDTH - description_at);
    if entry.syntax.len() <= SYNTAX_COLUMN {
        let mut out = String::new();
        for (at, line) in about.iter().enumerate() {
            if at == 0 {
                out.push_str(&format!(
                    "{:INDENT$}{:SYNTAX_COLUMN$}  {line}\n",
                    "", entry.syntax
                ));
            } else {
                out.push_str(&format!("{:description_at$}{line}\n", ""));
            }
        }
        return out;
    }
    let syntax = wrap(entry.syntax, WIDTH - INDENT - 4);
    let mut out = String::new();
    for (at, line) in syntax.iter().enumerate() {
        let pad = if at == 0 { INDENT } else { INDENT + 4 };
        out.push_str(&format!("{:pad$}{line}\n", ""));
    }
    out.push_str(&indented(&about, description_at));
    out
}

fn groups() -> Vec<&'static str> {
    let mut seen: Vec<&'static str> = Vec::new();
    for entry in ENTRIES {
        if !seen.contains(&entry.group) {
            seen.push(entry.group);
        }
    }
    seen
}

/// The text of `cstan help`, `cstan --help` and `cstan -h`.
pub fn help_text() -> String {
    let mut out = format!("{NAME_LINE}\n{SYNOPSIS_LINE}\n");
    for group in groups() {
        out.push_str(&format!("\n{group}\n"));
        for entry in ENTRIES.iter().filter(|e| e.group == group) {
            out.push_str(&listing_entry(entry));
        }
    }
    out.push_str("\nRecord ids may be given as a unique prefix of 4 or more characters.\nRun `cstan help <command>` for one command, `cstan --version` for the version,\nand `man cstan` for the full manual.\n");
    out
}

/// The entries of the command word `command`.
pub fn entries_of(command: &str) -> Vec<&'static Entry> {
    ENTRIES.iter().filter(|e| e.command == command).collect()
}

/// `cstan help <command>`: the syntax and the description of each entry of the command word, or `None` when it is not
/// one.
pub fn command_help(command: &str) -> Option<String> {
    let entries = entries_of(command);
    if entries.is_empty() {
        return None;
    }
    let mut out = String::new();
    for (at, entry) in entries.iter().enumerate() {
        if at > 0 {
            out.push('\n');
        }
        out.push_str(&syntax_lines("usage: cstan ", entry.syntax));
        out.push_str(&indented(&wrap(entry.about, WIDTH - INDENT), INDENT));
    }
    Some(out)
}

/// `prefix` and `syntax` as lines of at most [`WIDTH`] columns; a continuation line lines up with the first word after the
/// prefix, indented by four more spaces.
fn syntax_lines(prefix: &str, syntax: &str) -> String {
    let wrapped = wrap(syntax, WIDTH - prefix.len());
    let mut out = String::new();
    for (at, line) in wrapped.iter().enumerate() {
        let lead = if at == 0 {
            prefix.to_string()
        } else {
            " ".repeat(prefix.len() + 4)
        };
        out.push_str(&format!("{lead}{line}\n"));
    }
    out
}

/// What follows a usage error: the syntax of the command the arguments start with, and where to read more. The whole
/// command list is never printed here.
pub fn hint(args: &[String]) -> String {
    let Some(first) = args.first() else {
        return format!("{SYNOPSIS_LINE}\nrun `cstan help` for the list of commands\n");
    };
    if matches!(first.as_str(), "--version" | "-V" | "version") {
        return "usage: cstan --version\nrun `cstan help` for the list of commands\n".to_string();
    }
    let entries = entries_of(first);
    if entries.is_empty() {
        let what = if first.starts_with('-') {
            "option"
        } else {
            "command"
        };
        return format!(
            "cstan: unknown {what}: {first}\nrun `cstan help` for the list of commands\n"
        );
    }
    let mut out = String::new();
    for (at, entry) in entries.iter().enumerate() {
        out.push_str(&syntax_lines(
            if at == 0 {
                "usage: cstan "
            } else {
                "       cstan "
            },
            entry.syntax,
        ));
    }
    out.push_str(&format!(
        "run `cstan help {first}` for details, or `cstan help` for all commands\n"
    ));
    out
}

/// What `cstan help <words>` prints: the help of one command word, the man page for `--man`, else an error hint.
pub fn help_for(words: &[String], version: &str) -> Result<String, String> {
    match words {
        [] => Ok(help_text()),
        [word] if word == "--man" => Ok(man_page(version)),
        [word] => command_help(word).ok_or_else(|| {
            format!("cstan: unknown command: {word}\nrun `cstan help` for the list of commands\n")
        }),
        _ => Err(
            "usage: cstan help [<command>]\nrun `cstan help` for the list of commands\n"
                .to_string(),
        ),
    }
}

/// `text` as roff: backslashes and hyphens escaped, and no line starting with a control character.
fn roff(text: &str) -> String {
    let escaped = text.replace('\\', "\\e").replace('-', "\\-");
    match escaped.chars().next() {
        Some('.' | '\'') => format!("\\&{escaped}"),
        _ => escaped,
    }
}

/// The roff source of `cstan.1`, with every command of [`ENTRIES`] under COMMANDS.
pub fn man_page(version: &str) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        ".\\\" Generated by `cstan help --man` from the command table of cstan {version}; do not edit.\n"
    ));
    out.push_str(&format!(
        ".TH CSTAN 1 \"\" \"cstan {}\" \"Capstan\"\n",
        roff(version)
    ));
    out.push_str(".SH NAME\ncstan \\- run a team of coding agents in a git repository\n");
    out.push_str(".SH SYNOPSIS\n.B cstan\n.I command\n.RI [ args ]\n.br\n.B cstan help\n.RI [ command ]\n.br\n.B cstan\n.BR \\-\\-version\n");
    out.push_str(".SH DESCRIPTION\n");
    out.push_str("Capstan runs a controller daemon for one git repository. The controller starts a project manager (PM), workers, reviewers and a supervisor as agents in Herdr panes, hands each worker its own git worktree and branch, delivers messages between them and keeps every report, review, plan and integration in a ledger.\n");
    out.push_str(".PP\nYou run the setup, status and operator commands from the project directory. Agents run the messaging and delivery commands from their own panes; they reach the controller through the socket and token in their environment. Every command that talks to the controller accepts\n.B \\-\\-json\nto print the raw result.\n");
    out.push_str(
        ".PP\n.B cstan help\nlists the commands;\n.B cstan help\n.I command\nshows one of them.\n",
    );
    out.push_str(ID_PREFIX_NOTE_MAN);
    out.push_str(".SH COMMANDS\n");
    for group in groups() {
        out.push_str(&format!(".SS {}\n", roff(group).replace('&', "\\&&")));
        for entry in ENTRIES.iter().filter(|e| e.group == group) {
            out.push_str(&format!(
                ".TP\n.B cstan {}\n{}\n",
                roff(entry.syntax),
                roff(entry.about)
            ));
        }
    }
    out.push_str(".SH FILES\n.TP\n.I capstan.toml\nThe project configuration: the agent hosts, the roles and the run settings. Checked by\n.B cstan config check\nand created by\n.BR \"cstan init\" .\n");
    out.push_str(".TP\n.I .capstan/\nThe project state, created by\n.BR \"cstan init\" :\nthe ledger, the controller's control socket under\n.IR state/ ,\nthe operator key and the logs. It is kept out of git by the managed block of\n.IR .gitignore ;\ndo not commit it.\n");
    out.push_str(".SH ENVIRONMENT\n");
    for (name, text) in [
        ("CAPSTAN_TOKEN", "The credential of an agent. Set by the controller in the agent's pane."),
        ("CAPSTAN_SOCKET", "The controller socket an agent talks to. A socket that belongs to another project is refused unless CAPSTAN_ALLOW_FOREIGN_SOCKET is 1."),
        ("CAPSTAN_AGENT_ID", "The id of the agent. Set by the controller in the agent's pane."),
        ("CAPSTAN_ALLOW_FOREIGN_SOCKET", "Set to 1 to use a CAPSTAN_SOCKET that belongs to another project."),
        ("CSTAN_DASH_BIN", "Absolute path of the cstan-dash binary that cstan dash runs."),
    ] {
        out.push_str(&format!(".TP\n.B {}\n{}\n", roff(name), roff(text)));
    }
    out.push_str(".SH EXIT STATUS\n");
    for (code, text) in [
        ("0", "Success."),
        ("2", "Usage error: unknown command or wrong arguments. A one-line hint names the syntax."),
        ("3", "The request was invalid or refused."),
        ("4", "The request was blocked by the state of the project, for example a controller that is not running."),
        ("5", "A runtime error, such as a failed connection."),
    ] {
        out.push_str(&format!(".TP\n.B {code}\n{}\n", roff(text)));
    }
    out.push_str(".SH SEE ALSO\n.BR git (1)\n");
    out
}
