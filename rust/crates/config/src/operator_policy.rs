//! The subset of `src/operator-policy.ts` that config loading uses: which commands may be configured as auto-approve
//! rules (`autoApproveRuleProblem`, with `normalizeCommand`, `classifyCommand` and the read-only allowlist behind it).
use crate::text::{is_js_space, trim};

const MAX_OPERATOR_COMMAND_BYTES: usize = 8192;

const ALWAYS_APPROVAL: [&str; 104] = [
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
const DANGEROUS_SHORT_LETTERS: [char; 10] = ['f', 'd', 'D', 'x', 'r', 'R', 'o', 'O', 'e', 'c'];

enum Positionals {
    None,
    Paths,
    Words(&'static [&'static str]),
}

struct Entry {
    command: &'static str,
    subcommand: Option<&'static str>,
    options: &'static [&'static str],
    positionals: Positionals,
}

/// The only commands that may ever run without a human decision; all are read-only and run no project code.
const ALLOWLIST: [Entry; 10] = [
    Entry {
        command: "ls",
        subcommand: None,
        options: &["-l", "-a", "-la", "-h"],
        positionals: Positionals::Paths,
    },
    Entry {
        command: "pwd",
        subcommand: None,
        options: &[],
        positionals: Positionals::None,
    },
    Entry {
        command: "whoami",
        subcommand: None,
        options: &[],
        positionals: Positionals::None,
    },
    Entry {
        command: "date",
        subcommand: None,
        options: &[],
        positionals: Positionals::None,
    },
    Entry {
        command: "uname",
        subcommand: None,
        options: &["-a"],
        positionals: Positionals::None,
    },
    Entry {
        command: "df",
        subcommand: None,
        options: &["-h"],
        positionals: Positionals::None,
    },
    Entry {
        command: "cstan",
        subcommand: Some("ping"),
        options: &[],
        positionals: Positionals::None,
    },
    Entry {
        command: "cstan",
        subcommand: Some("status"),
        options: &[],
        positionals: Positionals::None,
    },
    Entry {
        command: "git",
        subcommand: Some("rev-parse"),
        options: &["--abbrev-ref", "--short"],
        positionals: Positionals::Words(&["HEAD"]),
    },
    Entry {
        command: "git",
        subcommand: Some("ls-files"),
        options: &[],
        positionals: Positionals::None,
    },
];

/// `/^[A-Za-z0-9 _./:=@%+,-]+$/`.
fn simple_command(text: &str) -> bool {
    !text.is_empty()
        && text
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b" _./:=@%+,-".contains(&b))
}

/// `/^[A-Za-z0-9_./:@%+,][A-Za-z0-9_./:@%+,-]*$/` and no `=`.
fn safe_positional(token: &str) -> bool {
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
        if form.chars().count() > 1 && form != form.to_lowercase() {
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

struct Classification {
    simple: bool,
    always_approval: Vec<String>,
}

fn classify_command(text: &str) -> Classification {
    let simple = simple_command(text) && !text.contains('\n');
    let mut forms: Vec<String> = Vec::new();
    let mut letters: Vec<char> = Vec::new();
    for token in words(text) {
        for form in word_forms(token) {
            add(&mut forms, &form);
        }
        for letter in short_letters(token) {
            if !letters.contains(&letter) {
                letters.push(letter);
            }
        }
    }
    let mut always_approval: Vec<String> = forms
        .into_iter()
        .filter(|form| ALWAYS_APPROVAL.contains(&form.as_str()))
        .collect();
    always_approval.extend(
        letters
            .into_iter()
            .filter(|letter| DANGEROUS_SHORT_LETTERS.contains(letter))
            .map(|letter| format!("-{letter}")),
    );
    Classification {
        simple,
        always_approval,
    }
}

fn find_entry<'a>(tokens: &[&'a str]) -> Option<(&'static Entry, Vec<&'a str>)> {
    let command = tokens.first().copied();
    let second = tokens.get(1).copied();
    for entry in &ALLOWLIST {
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
        match &entry.positionals {
            Positionals::Words(list) if list.contains(&token) => continue,
            Positionals::Paths if safe_positional(token) => continue,
            _ => {}
        }
        return Some(format!(
            "has argument \"{token}\", which the allowlist does not accept for {}",
            entry.command
        ));
    }
    None
}

/// `normalizeCommand` for the rule texts config accepts: the refusal code, or `None` when the text is accepted.
fn refusal(text: &str) -> Option<&'static str> {
    if !text
        .bytes()
        .all(|b| (0x20..=0x7e).contains(&b) || b == b'\n' || b == b'\t')
    {
        return Some("non_ascii");
    }
    if trim(text).is_empty() {
        return Some("empty");
    }
    if text.len() > MAX_OPERATOR_COMMAND_BYTES {
        return Some("too_long");
    }
    None
}

/// `None` when `rule` may be a configured auto-approve rule; otherwise why not.
pub fn auto_approve_rule_problem(rule: &str) -> Option<String> {
    if let Some(code) = refusal(rule) {
        return Some(format!("is not a usable command ({code})"));
    }
    let classification = classify_command(rule);
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
    if rule != trim(rule) || rule.contains("  ") {
        return Some("must have single spaces and no leading or trailing space".into());
    }
    let tokens = words(rule);
    if tokens[0].contains('=') {
        return Some("must not start with a variable assignment".into());
    }
    allowlist_problem(&tokens)
}
