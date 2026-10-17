//! Operator command policy (src/operator-policy.ts): how a shell proposal is normalised and hashed, and which commands
//! may run without a human decision. Auto-approval is an allowlist of tiny read-only commands over a tiny character set;
//! everything else needs a human every time. The denylist is a second layer, not the primary guard.
//!
//! `capstan_config` already ports the subset config loading needs; its rule check (`auto_approve_rule_problem`) is reused
//! here, so a configured rule and a matched rule can never disagree. The rest of the module is private to config, so the
//! word lists below are a second copy that `tests/parity.rs` pins to Node's through the committed fixtures.

use crate::api::{GrantKind, ProposalKind};
use capstan_config::auto_approve_rule_problem;
use sha2::{Digest, Sha256};

pub const MAX_OPERATOR_COMMAND_BYTES: usize = 8192;
pub const MAX_OPERATOR_REASON_BYTES: usize = 1024;
pub const HASH_PREFIX_CHARS: usize = 12;
/// The `auto_rule` of a proposal approved under full auto.
pub const FULL_AUTO_RULE: &str = "full-auto";
const SESSION_RULE_LEAD: &str = "session:";

/// Words that always need a human decision, matched against the normalised token forms.
pub const OPERATOR_ALWAYS_APPROVAL: [&str; 104] = [
    "push",
    "force",
    "force-with-lease",
    "f",
    "hard",
    "delete",
    "d",
    "D",
    "clean",
    "rm",
    "rmdir",
    "unlink",
    "shred",
    "truncate",
    "dd",
    "mkfs",
    "mv",
    "cp",
    "chmod",
    "chown",
    "chgrp",
    "ln",
    "kill",
    "killall",
    "pkill",
    "reboot",
    "shutdown",
    "halt",
    "poweroff",
    "sudo",
    "su",
    "doas",
    "reset",
    "restore",
    "checkout",
    "switch",
    "rebase",
    "merge",
    "cherry-pick",
    "revert",
    "commit",
    "stash",
    "tag",
    "am",
    "apply",
    "gc",
    "prune",
    "filter-branch",
    "update-ref",
    "worktree",
    "submodule",
    "remote",
    "config",
    "fetch",
    "pull",
    "clone",
    "curl",
    "wget",
    "ssh",
    "scp",
    "rsync",
    "nc",
    "ncat",
    "sh",
    "bash",
    "zsh",
    "dash",
    "fish",
    "eval",
    "exec",
    "source",
    "xargs",
    "env",
    "nohup",
    "find",
    "tee",
    "install",
    "patch",
    "tar",
    "unzip",
    "zip",
    "npm",
    "npx",
    "pnpm",
    "yarn",
    "node",
    "python",
    "python3",
    "perl",
    "ruby",
    "make",
    "cmake",
    "cargo",
    "go",
    "pip",
    "docker",
    "kubectl",
    "output",
    "ext-diff",
    "textconv",
    "script-shell",
    "upload-pack",
    "receive-pack",
    "open-files-in-pager",
];

/// Single-dash letters that write, execute or delete: -f -d -D -x -r -R -o -O -e -c.
pub const OPERATOR_DANGEROUS_SHORT_LETTERS: [char; 10] =
    ['f', 'd', 'D', 'x', 'r', 'R', 'o', 'O', 'e', 'c'];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Positionals {
    /// No argument.
    None,
    /// Safe path-like arguments.
    Paths,
    /// Exactly these words.
    Words(&'static [&'static str]),
}

#[derive(Clone, Copy, Debug)]
pub struct AllowlistEntry {
    pub command: &'static str,
    pub subcommand: Option<&'static str>,
    pub options: &'static [&'static str],
    pub positionals: Positionals,
}

/// The only commands that may ever run without a human decision; all are read-only and run no project code.
pub const OPERATOR_AUTO_ALLOWLIST: [AllowlistEntry; 10] = [
    AllowlistEntry {
        command: "ls",
        subcommand: None,
        options: &["-l", "-a", "-la", "-h"],
        positionals: Positionals::Paths,
    },
    AllowlistEntry {
        command: "pwd",
        subcommand: None,
        options: &[],
        positionals: Positionals::None,
    },
    AllowlistEntry {
        command: "whoami",
        subcommand: None,
        options: &[],
        positionals: Positionals::None,
    },
    AllowlistEntry {
        command: "date",
        subcommand: None,
        options: &[],
        positionals: Positionals::None,
    },
    AllowlistEntry {
        command: "uname",
        subcommand: None,
        options: &["-a"],
        positionals: Positionals::None,
    },
    AllowlistEntry {
        command: "df",
        subcommand: None,
        options: &["-h"],
        positionals: Positionals::None,
    },
    AllowlistEntry {
        command: "cstan",
        subcommand: Some("ping"),
        options: &[],
        positionals: Positionals::None,
    },
    AllowlistEntry {
        command: "cstan",
        subcommand: Some("status"),
        options: &[],
        positionals: Positionals::None,
    },
    AllowlistEntry {
        command: "git",
        subcommand: Some("rev-parse"),
        options: &["--abbrev-ref", "--short"],
        positionals: Positionals::Words(&["HEAD"]),
    },
    AllowlistEntry {
        command: "git",
        subcommand: Some("ls-files"),
        options: &[],
        positionals: Positionals::None,
    },
];

// ------------------------------------------------------------------------------------------------ text

/// The refusal of `normalizeCommand` / `normalizeReason`: the code Node returns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextRefusal {
    Empty,
    TooLong,
    NonAscii,
}

impl TextRefusal {
    pub fn code(self) -> &'static str {
        match self {
            Self::Empty => "empty",
            Self::TooLong => "too_long",
            Self::NonAscii => "non_ascii",
        }
    }
}

/// `\s` of a JavaScript regular expression (and what `String.prototype.trim` removes).
pub fn is_js_space(c: char) -> bool {
    matches!(
        c,
        '\t' | '\n' | '\u{0b}' | '\u{0c}' | '\r' | ' ' | '\u{a0}' | '\u{1680}' | '\u{2000}'
            ..='\u{200a}'
                | '\u{2028}'
                | '\u{2029}'
                | '\u{202f}'
                | '\u{205f}'
                | '\u{3000}'
                | '\u{feff}'
    )
}

fn js_trim(text: &str) -> &str {
    text.trim_matches(is_js_space)
}

/// `/^[\x20-\x7e\n\t]*$/`: printable ASCII plus newline and tab; NUL, C0/C1 controls, bidi and zero-width characters
/// and homoglyphs all fall outside it.
fn ascii_text(text: &str) -> bool {
    text.bytes()
        .all(|b| (0x20..=0x7e).contains(&b) || b == b'\n' || b == b'\t')
}

fn check_text(text: &str, max_bytes: usize) -> Result<(), TextRefusal> {
    if !ascii_text(text) {
        return Err(TextRefusal::NonAscii);
    }
    if js_trim(text).is_empty() {
        return Err(TextRefusal::Empty);
    }
    if text.len() > max_bytes {
        return Err(TextRefusal::TooLong);
    }
    Ok(())
}

/// The command exactly as sent, or the reason it is refused. It is never trimmed or rewritten: the hash covers these
/// bytes.
pub fn normalize_command(text: &str) -> Result<&str, TextRefusal> {
    check_text(text, MAX_OPERATOR_COMMAND_BYTES).map(|()| text)
}

pub fn normalize_reason(text: &str) -> Result<&str, TextRefusal> {
    check_text(text, MAX_OPERATOR_REASON_BYTES).map(|()| text)
}

/// `commandHash`: sha256 of `JSON.stringify([kind, command, forceRestart])`, as lowercase hex.
pub fn command_hash(kind: &str, command: &str, force_restart: bool) -> String {
    let text = serde_json::json!([kind, command, force_restart]).to_string();
    let digest = Sha256::digest(text.as_bytes());
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// `hashPrefix`: the first 12 characters.
pub fn hash_prefix(hash: &str) -> &str {
    match hash.char_indices().nth(HASH_PREFIX_CHARS) {
        Some((end, _)) => &hash[..end],
        None => hash,
    }
}

// ------------------------------------------------------------------------------------------------ tokens

/// `/^[A-Za-z0-9 _./:=@%+,-]+$/`.
fn simple_chars(text: &str) -> bool {
    !text.is_empty()
        && text
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b" _./:=@%+,-".contains(&b))
}

/// `/^[A-Za-z0-9_./:@%+,][A-Za-z0-9_./:@%+,-]*$/` and no `=`.
pub fn is_safe_positional(token: &str) -> bool {
    let class = |b: u8| b.is_ascii_alphanumeric() || b"_./:@%+,".contains(&b);
    match token.as_bytes().split_first() {
        Some((head, tail)) => class(*head) && tail.iter().all(|b| class(*b) || *b == b'-'),
        None => false,
    }
}

/// `text.split(/\s+/).filter((token) => token !== "")`.
pub fn words(text: &str) -> Vec<&str> {
    text.split(is_js_space)
        .filter(|token| !token.is_empty())
        .collect()
}

fn basename(token: &str) -> &str {
    let trimmed = token.trim_end_matches('/');
    match trimmed.rfind('/') {
        Some(slash) => &trimmed[slash + 1..],
        None => trimmed,
    }
}

fn add(forms: &mut Vec<String>, form: &str) {
    if !forms.iter().any(|existing| existing == form) {
        forms.push(form.to_string());
    }
}

fn word_forms(token: &str) -> Vec<String> {
    let mut forms: Vec<String> = Vec::new();
    let add_form = |forms: &mut Vec<String>, form: &str| {
        if form.is_empty() {
            return;
        }
        add(forms, form);
        add(forms, basename(form));
        if form.starts_with("--") {
            let bare = form.trim_start_matches('-');
            add(forms, bare);
            add(forms, basename(bare));
        }
    };
    add_form(&mut forms, token);
    for part in token.split('=') {
        add_form(&mut forms, part);
    }
    for form in forms.clone() {
        if form.encode_utf16().count() > 1 && form != form.to_lowercase() {
            add(&mut forms, &form.to_lowercase());
        }
    }
    forms
}

/// The single letters of a combined short flag such as `-fd`; a long flag or a bare word yields none.
fn short_letters(token: &str) -> Vec<char> {
    let flag = token.split('=').next().unwrap_or("");
    match flag.strip_prefix('-') {
        Some(letters)
            if !letters.is_empty() && letters.bytes().all(|b| b.is_ascii_alphabetic()) =>
        {
            letters.chars().collect()
        }
        _ => Vec::new(),
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tokens {
    pub words: Vec<String>,
    pub letters: Vec<char>,
}

pub fn tokenize(text: &str) -> Tokens {
    let mut found: Vec<String> = Vec::new();
    let mut letters: Vec<char> = Vec::new();
    for token in words(text) {
        for form in word_forms(token) {
            add(&mut found, &form);
        }
        for letter in short_letters(token) {
            if !letters.contains(&letter) {
                letters.push(letter);
            }
        }
    }
    Tokens {
        words: found,
        letters,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandClassification {
    pub simple: bool,
    /// The normalised token forms (raw, basename, split on `=`, long flags without dashes).
    pub tokens: Vec<String>,
    /// The denylist words and dangerous short letters found; empty when none.
    pub always_approval: Vec<String>,
}

pub fn classify_command(text: &str) -> CommandClassification {
    let simple = simple_chars(text) && !text.contains('\n');
    let Tokens {
        words: forms,
        letters,
    } = tokenize(text);
    let mut always_approval: Vec<String> = forms
        .iter()
        .filter(|form| OPERATOR_ALWAYS_APPROVAL.contains(&form.as_str()))
        .cloned()
        .collect();
    always_approval.extend(
        letters
            .iter()
            .filter(|letter| OPERATOR_DANGEROUS_SHORT_LETTERS.contains(letter))
            .map(|letter| format!("-{letter}")),
    );
    CommandClassification {
        simple,
        tokens: forms,
        always_approval,
    }
}

fn find_entry<'a>(tokens: &[&'a str]) -> Option<(&'static AllowlistEntry, Vec<&'a str>)> {
    let command = tokens.first().copied();
    let second = tokens.get(1).copied();
    for entry in &OPERATOR_AUTO_ALLOWLIST {
        if Some(entry.command) != command {
            continue;
        }
        match entry.subcommand {
            None => return Some((entry, tokens[1..].to_vec())),
            Some(sub) if Some(sub) == second => return Some((entry, tokens[2..].to_vec())),
            Some(_) => {}
        }
    }
    None
}

/// Why a token list is not a command the allowlist knows; `None` when it is exactly allowed.
fn allowlist_problem(tokens: &[&str]) -> Option<String> {
    let Some((entry, rest)) = find_entry(tokens) else {
        return Some("is not on the read-only allowlist (OPERATOR_AUTO_ALLOWLIST)".into());
    };
    for token in rest {
        if entry.options.contains(&token) {
            continue;
        }
        if token.starts_with('-') || token.contains('=') {
            return Some(format!(
                "uses option \"{token}\", which the allowlist does not accept for {}",
                entry.command
            ));
        }
        match entry.positionals {
            Positionals::Words(list) if list.contains(&token) => continue,
            Positionals::Paths if is_safe_positional(token) => continue,
            _ => {}
        }
        return Some(format!(
            "has argument \"{token}\", which the allowlist does not accept for {}",
            entry.command
        ));
    }
    None
}

// ------------------------------------------------------------------------------------------------ auto approval

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AutoApproveVerdict {
    pub auto: bool,
    pub rule: Option<String>,
    pub reason: String,
}

fn no(reason: impl Into<String>) -> AutoApproveVerdict {
    AutoApproveVerdict {
        auto: false,
        rule: None,
        reason: reason.into(),
    }
}

/// Whether the command may run without a human. Exact rules match the whole command; prefix rules match whole leading
/// tokens and accept only safe positional tokens after them.
pub fn matches_auto_approve(
    text: &str,
    exact_rules: &[String],
    prefix_rules: &[String],
) -> AutoApproveVerdict {
    if exact_rules.is_empty() && prefix_rules.is_empty() {
        return no("no auto-approve rules are configured");
    }
    if normalize_command(text).is_err() {
        return no("the command is not accepted text");
    }
    let classification = classify_command(text);
    if !classification.simple {
        return no("the command is not a simple command line");
    }
    if !classification.always_approval.is_empty() {
        return no(format!(
            "the command contains {}, which always needs a human decision",
            classification.always_approval.join(", ")
        ));
    }
    let tokens = words(text);
    if tokens[0].contains('=') {
        return no("the command starts with an assignment");
    }
    for rule in exact_rules {
        if text != rule {
            continue;
        }
        if auto_approve_rule_problem(rule).is_none() {
            return AutoApproveVerdict {
                auto: true,
                rule: Some(rule.clone()),
                reason: "exact rule".into(),
            };
        }
    }
    for rule in prefix_rules {
        if auto_approve_rule_problem(rule).is_some() {
            continue;
        }
        let head = words(rule);
        if tokens.len() < head.len() {
            continue;
        }
        if !head
            .iter()
            .enumerate()
            .all(|(i, token)| tokens[i] == *token)
        {
            continue;
        }
        let rest = &tokens[head.len()..];
        if !rest.iter().all(|token| is_safe_positional(token)) {
            continue;
        }
        let entry = find_entry(&head).map(|(entry, _)| entry);
        if !rest.is_empty() && entry.map(|e| e.positionals) != Some(Positionals::Paths) {
            continue;
        }
        if allowlist_problem(&tokens).is_none() {
            return AutoApproveVerdict {
                auto: true,
                rule: Some(rule.clone()),
                reason: "prefix rule".into(),
            };
        }
    }
    no("no auto-approve rule matches")
}

/// A restart is never auto-approved; a command only when a rule matches.
pub fn auto_decision(
    kind: ProposalKind,
    text: &str,
    exact_rules: &[String],
    prefix_rules: &[String],
) -> AutoApproveVerdict {
    if kind == ProposalKind::Restart {
        return no("a restart always needs a human decision");
    }
    matches_auto_approve(text, exact_rules, prefix_rules)
}

// ------------------------------------------------------------------------------------------------ session grants

/// `sessionRule`: the `auto_rule` of a proposal approved by a session grant.
pub fn session_rule(grant_id: &str) -> String {
    format!("{SESSION_RULE_LEAD}{grant_id}")
}

/// The grant id inside a `session:<grant>` rule; `None` for any other rule.
pub fn session_rule_grant_id(rule: Option<&str>) -> Option<&str> {
    rule?.strip_prefix(SESSION_RULE_LEAD)
}

/// A prefix of only these words names a family of commands too wide to grant; a subcommand must follow.
const PREFIX_NEEDS_SUBCOMMAND: [&str; 2] = ["git", "cstan"];

/// `None` when `prefix` may be stored as a session grant; otherwise why not. The prefix is kept verbatim, so it must
/// already be in the single-spaced form the match uses.
pub fn prefix_grant_problem(prefix: &str) -> Option<String> {
    if let Err(refusal) = normalize_command(prefix) {
        return Some(format!("is not a usable prefix ({})", refusal.code()));
    }
    let classification = classify_command(prefix);
    if !classification.simple {
        return Some(
            "must be one line of letters, digits, space and _ . / : = @ % + , - only".into(),
        );
    }
    if !classification.always_approval.is_empty() {
        let quoted: Vec<String> = classification
            .always_approval
            .iter()
            .map(|word| format!("\"{word}\""))
            .collect();
        return Some(format!(
            "contains {}, which always needs a human decision",
            quoted.join(", ")
        ));
    }
    if prefix != js_trim(prefix) || prefix.contains("  ") {
        return Some("must have single spaces and no leading or trailing space".into());
    }
    let tokens = words(prefix);
    if tokens[0].contains('=') {
        return Some("must not start with a variable assignment".into());
    }
    if tokens.len() == 1 && PREFIX_NEEDS_SUBCOMMAND.contains(&tokens[0]) {
        return Some(format!(
            "is only \"{}\"; name the subcommand too",
            tokens[0]
        ));
    }
    None
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GrantRef {
    pub grant_id: String,
    pub kind: GrantKind,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GrantVerdict {
    Matched { grant_id: String },
    Unmatched { reason: String },
}

fn unmatched(reason: impl Into<String>) -> GrantVerdict {
    GrantVerdict::Unmatched {
        reason: reason.into(),
    }
}

/// Whether an active session grant lets `command` run without a new approval. An exact grant matches the whole text; a
/// prefix grant matches whole leading tokens, and the rest of the command is classified like any other: a denylist hit,
/// a metacharacter or a second line needs a human.
pub fn match_session_grant(kind: ProposalKind, command: &str, grants: &[GrantRef]) -> GrantVerdict {
    if kind != ProposalKind::Command {
        return unmatched("a restart always needs a human decision");
    }
    if grants.is_empty() {
        return unmatched("no session grant is active");
    }
    if let Some(exact) = grants
        .iter()
        .find(|grant| grant.kind == GrantKind::Exact && grant.text == command)
    {
        return GrantVerdict::Matched {
            grant_id: exact.grant_id.clone(),
        };
    }
    let classification = classify_command(command);
    if !classification.simple {
        return unmatched("the command is not a simple command line");
    }
    if !classification.always_approval.is_empty() {
        return unmatched(format!(
            "the command contains {}, which always needs a human decision",
            classification.always_approval.join(", ")
        ));
    }
    let tokens = words(command);
    if tokens.is_empty() || tokens[0].contains('=') {
        return unmatched("the command starts with an assignment");
    }
    for grant in grants {
        if grant.kind != GrantKind::Prefix {
            continue;
        }
        let head = words(&grant.text);
        if tokens.len() < head.len() {
            continue;
        }
        if head
            .iter()
            .enumerate()
            .all(|(i, token)| tokens[i] == *token)
        {
            return GrantVerdict::Matched {
                grant_id: grant.grant_id.clone(),
            };
        }
    }
    unmatched("no session grant matches")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn hash12_is_the_first_twelve_hex_characters_of_the_json_array_digest() {
        let hash = command_hash("command", "ls -la", false);
        assert_eq!(hash.len(), 64);
        assert_eq!(hash_prefix(&hash), &hash[..12]);
        assert_ne!(hash, command_hash("command", "ls -la", true));
        assert_ne!(hash, command_hash("restart", "ls -la", false));
    }

    #[test]
    fn normalisation_refuses_what_node_refuses() {
        assert_eq!(normalize_command("ls\t-l\n").unwrap(), "ls\t-l\n");
        assert_eq!(normalize_command("  \n"), Err(TextRefusal::Empty));
        assert_eq!(normalize_command("é"), Err(TextRefusal::NonAscii));
        assert_eq!(normalize_command("a\u{0}b"), Err(TextRefusal::NonAscii));
        assert_eq!(
            normalize_command(&"a".repeat(MAX_OPERATOR_COMMAND_BYTES + 1)),
            Err(TextRefusal::TooLong)
        );
        assert!(normalize_command(&"a".repeat(MAX_OPERATOR_COMMAND_BYTES)).is_ok());
        assert_eq!(
            normalize_reason(&"a".repeat(MAX_OPERATOR_REASON_BYTES + 1)),
            Err(TextRefusal::TooLong)
        );
    }

    #[test]
    fn a_read_only_command_needs_a_rule_and_a_denied_word_never_matches() {
        let exact = strings(&["ls -la"]);
        let prefix = strings(&["ls"]);
        assert!(matches_auto_approve("ls -la", &exact, &[]).auto);
        assert!(matches_auto_approve("ls src", &[], &prefix).auto);
        assert!(!matches_auto_approve("ls -la", &[], &[]).auto);
        assert!(!matches_auto_approve("ls ; rm x", &exact, &prefix).auto);
        assert!(!matches_auto_approve("rm -rf x", &strings(&["rm -rf x"]), &[]).auto);
        assert!(!matches_auto_approve("ls -R", &[], &prefix).auto);
        assert_eq!(
            auto_decision(ProposalKind::Restart, "ls", &exact, &prefix).reason,
            "a restart always needs a human decision"
        );
    }

    #[test]
    fn a_prefix_grant_matches_whole_words_only_and_the_denylist_still_applies() {
        let grants = vec![GrantRef {
            grant_id: "g1".into(),
            kind: GrantKind::Prefix,
            text: "git status".into(),
        }];
        assert_eq!(
            match_session_grant(ProposalKind::Command, "git status -s", &grants),
            GrantVerdict::Matched {
                grant_id: "g1".into()
            }
        );
        assert!(matches!(
            match_session_grant(ProposalKind::Command, "git statusx", &grants),
            GrantVerdict::Unmatched { .. }
        ));
        assert!(matches!(
            match_session_grant(ProposalKind::Command, "git status --force", &grants),
            GrantVerdict::Unmatched { .. }
        ));
        assert!(matches!(
            match_session_grant(ProposalKind::Restart, "git status", &grants),
            GrantVerdict::Unmatched { .. }
        ));
        assert_eq!(
            prefix_grant_problem("git").as_deref(),
            Some("is only \"git\"; name the subcommand too")
        );
        assert!(prefix_grant_problem("git status").is_none());
        assert!(prefix_grant_problem("rm").is_some());
    }

    #[test]
    fn session_rules_round_trip() {
        assert_eq!(session_rule("g9"), "session:g9");
        assert_eq!(session_rule_grant_id(Some("session:g9")), Some("g9"));
        assert_eq!(session_rule_grant_id(Some("full-auto")), None);
        assert_eq!(session_rule_grant_id(None), None);
    }
}
