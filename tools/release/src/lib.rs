//! Release helpers: the SemVer level from Conventional Commits and the CHANGELOG section. A port of the former
//! scripts/release-notes.mjs; the subject rules are the ones in src/conventions.ts and the controller's commit check.
use std::path::Path;
use std::process::Command;

const COMMIT_TYPES: [&str; 11] = [
    "feat", "fix", "docs", "refactor", "perf", "test", "build", "ci", "chore", "style", "revert",
];

const SECTIONS: [(&str, &str); 4] = [
    ("feat", "Features"),
    ("fix", "Bug fixes"),
    ("perf", "Performance"),
    ("revert", "Reverts"),
];

const OTHER_TITLES: [(&str, &str); 7] = [
    ("docs", "Documentation"),
    ("refactor", "Refactoring"),
    ("test", "Tests"),
    ("build", "Build"),
    ("ci", "CI"),
    ("chore", "Chores"),
    ("style", "Style"),
];

const TYPE_RANK: [&str; 11] = [
    "feat", "fix", "perf", "revert", "refactor", "docs", "test", "build", "ci", "style", "chore",
];

/// Subjects shorter than this are never treated as a cut-off title of a longer one.
const MIN_CUT_OFF: usize = 30;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Parsed {
    pub kind: String,
    pub scope: Option<String>,
    pub breaking: bool,
    pub description: String,
    pub note: Option<String>,
}

/// A commit as the release script reads it: its id (may be empty) and the full message.
#[derive(Debug, Clone)]
pub struct Commit {
    pub sha: String,
    pub message: String,
}

impl Commit {
    pub fn new(sha: &str, message: &str) -> Self {
        Commit {
            sha: sha.to_string(),
            message: message.to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub parsed: Parsed,
    pub sha: String,
}

/// Parses a commit message; None when the subject is not a conforming Conventional Commit.
pub fn parse_commit(message: &str) -> Option<Parsed> {
    let text = message.replace("\r\n", "\n").replace('\r', "\n");
    let mut lines = text.split('\n');
    let subject = lines.next().unwrap_or("");
    let (head, description) = subject.split_once(": ")?;
    let (head, bang) = match head.strip_suffix('!') {
        Some(rest) => (rest, true),
        None => (head, false),
    };
    let (kind, scope) = match head.split_once('(') {
        Some((kind, rest)) => {
            let scope = rest.strip_suffix(')')?;
            if scope.contains(['(', ')']) || scope.chars().any(char::is_whitespace) {
                return None;
            }
            (kind, Some(scope))
        }
        None => (head, None),
    };
    if kind.is_empty() || !kind.chars().all(|c| c.is_ascii_lowercase()) {
        return None;
    }
    if !COMMIT_TYPES.contains(&kind) || scope == Some("") {
        return None;
    }
    if description.trim().is_empty() || description.starts_with(char::is_whitespace) {
        return None;
    }
    let mut note = None;
    for line in lines {
        let rest = line
            .strip_prefix("BREAKING CHANGE:")
            .or_else(|| line.strip_prefix("BREAKING-CHANGE:"));
        if let Some(rest) = rest {
            note = Some(rest.strip_prefix(' ').unwrap_or(rest).trim().to_string());
            break;
        }
    }
    Some(Parsed {
        kind: kind.to_string(),
        scope: scope.map(str::to_string),
        breaking: bang || note.is_some(),
        description: description.to_string(),
        note: note.filter(|n| !n.is_empty()),
    })
}

fn normalize(commit: &Commit) -> Option<Entry> {
    parse_commit(&commit.message).map(|parsed| Entry {
        parsed,
        sha: commit.sha.clone(),
    })
}

pub fn parse_version(version: &str) -> Result<[u64; 3], String> {
    let parts: Vec<&str> = version.split('.').collect();
    let bad = || format!("not a X.Y.Z version: {version}");
    if parts.len() != 3 {
        return Err(bad());
    }
    let mut out = [0u64; 3];
    for (slot, part) in out.iter_mut().zip(parts) {
        if part.is_empty() || !part.chars().all(|c| c.is_ascii_digit()) {
            return Err(bad());
        }
        *slot = part.parse().map_err(|_| bad())?;
    }
    Ok(out)
}

pub fn compare_versions(a: &str, b: &str) -> Result<std::cmp::Ordering, String> {
    Ok(parse_version(a)?.cmp(&parse_version(b)?))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Next {
    pub version: String,
    pub level: &'static str,
}

/// The next version from commits; None when nothing is releasable.
pub fn next_version(current: &str, commits: &[Commit]) -> Result<Option<Next>, String> {
    let [major, minor, patch] = parse_version(current)?;
    let mut level: Option<&'static str> = None;
    for raw in commits {
        let Some(c) = normalize(raw) else { continue };
        let p = &c.parsed;
        if p.breaking {
            level = Some("major");
        } else if p.kind == "feat" && level != Some("major") {
            level = Some("minor");
        } else if ["fix", "perf", "revert"].contains(&p.kind.as_str()) && level.is_none() {
            level = Some("patch");
        }
    }
    let Some(mut level) = level else {
        return Ok(None);
    };
    if level == "major" && major == 0 {
        level = "minor";
    }
    let version = match level {
        "major" => format!("{}.0.0", major + 1),
        "minor" => format!("{major}.{}.0", minor + 1),
        _ => format!("{major}.{minor}.{}", patch + 1),
    };
    Ok(Some(Next { version, level }))
}

fn normalize_subject(description: &str) -> String {
    let lower = description.trim().to_lowercase();
    let spaced = lower.split_whitespace().collect::<Vec<_>>().join(" ");
    spaced
        .trim_end_matches(|c: char| c.is_whitespace() || ".,;:!?".contains(c))
        .to_string()
}

/// Equal subjects, or one a prefix of the other where the shorter is long enough to be a cut-off title.
fn same_subject(a: &str, b: &str) -> bool {
    let (short, long) = if a.chars().count() <= b.chars().count() {
        (a, b)
    } else {
        (b, a)
    };
    short == long || (short.chars().count() >= MIN_CUT_OFF && long.starts_with(short))
}

fn rank(kind: &str) -> usize {
    TYPE_RANK
        .iter()
        .position(|t| *t == kind)
        .unwrap_or(usize::MAX)
}

fn merge(prior: &Entry, c: &Entry) -> Entry {
    let better = if rank(&c.parsed.kind) < rank(&prior.parsed.kind) {
        c
    } else {
        prior
    };
    let mut scopes: Vec<&String> = [&prior.parsed.scope, &c.parsed.scope]
        .into_iter()
        .flatten()
        .collect();
    scopes.sort_by_key(|s| std::cmp::Reverse(s.chars().count()));
    let description = if c.parsed.description.trim().chars().count()
        > prior.parsed.description.trim().chars().count()
    {
        c.parsed.description.clone()
    } else {
        prior.parsed.description.clone()
    };
    let mut parsed = better.parsed.clone();
    parsed.scope = scopes.first().map(|s| (*s).clone());
    parsed.description = description;
    parsed.breaking = prior.parsed.breaking || c.parsed.breaking;
    parsed.note = prior.parsed.note.clone().or_else(|| c.parsed.note.clone());
    Entry {
        parsed,
        sha: better.sha.clone(),
    }
}

/// Parsed commits with repeats removed. Scope is ignored when comparing; subjects match when their normalized forms are
/// equal or one is a prefix of the other and at least 30 characters (a cut-off title). Types are compared across: the
/// merged entry takes the highest-ranking type, the longest subject and the more specific scope, at the first one's
/// position.
pub fn dedupe_commits(commits: &[Commit]) -> Vec<Entry> {
    let mut kept: Vec<(String, Entry)> = Vec::new();
    for raw in commits {
        let Some(c) = normalize(raw) else { continue };
        let subject = normalize_subject(&c.parsed.description);
        match kept.iter().position(|(s, _)| same_subject(s, &subject)) {
            None => kept.push((subject, c)),
            Some(at) => {
                let merged = merge(&kept[at].1, &c);
                kept[at] = (normalize_subject(&merged.parsed.description), merged);
            }
        }
    }
    kept.into_iter().map(|(_, e)| e).collect()
}

/// The CHANGELOG section for a release.
pub fn render_changelog_section(version: &str, date: &str, commits: &[Commit]) -> String {
    let parsed = dedupe_commits(commits);
    let item = |c: &Entry, text: &str| {
        let scope = c
            .parsed
            .scope
            .as_ref()
            .map(|s| format!("**{s}:** "))
            .unwrap_or_default();
        let sha = if c.sha.is_empty() {
            String::new()
        } else {
            format!(" ({})", c.sha.chars().take(7).collect::<String>())
        };
        format!("- {scope}{text}{sha}")
    };
    let mut groups: Vec<(&str, Vec<String>)> = Vec::new();
    let breaking: Vec<&Entry> = parsed.iter().filter(|c| c.parsed.breaking).collect();
    if !breaking.is_empty() {
        groups.push((
            "Breaking changes",
            breaking
                .iter()
                .map(|c| item(c, c.parsed.note.as_deref().unwrap_or(&c.parsed.description)))
                .collect(),
        ));
    }
    let rest: Vec<&Entry> = parsed.iter().filter(|c| !c.parsed.breaking).collect();
    for (kind, title) in SECTIONS.iter().chain(OTHER_TITLES.iter()) {
        let own: Vec<String> = rest
            .iter()
            .filter(|c| c.parsed.kind == *kind)
            .map(|c| item(c, &c.parsed.description))
            .collect();
        if !own.is_empty() {
            groups.push((title, own));
        }
    }
    let mut out = format!("## {version} ({date})\n");
    for (title, lines) in groups {
        out.push_str(&format!("\n### {title}\n\n{}\n", lines.join("\n")));
    }
    out
}

/// The highest vX.Y.Z tag reachable from HEAD, or None.
pub fn last_release_tag(root: &Path) -> Option<String> {
    let out = Command::new("git")
        .args(["tag", "--merged", "HEAD", "--list", "v*"])
        .current_dir(root)
        .output()
        .ok()?;
    if !out.status.success() {
        return None; // no commits yet
    }
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    let mut tags: Vec<(&str, [u64; 3])> = text
        .lines()
        .filter_map(|t| Some((t, parse_version(t.strip_prefix('v')?).ok()?)))
        .collect();
    tags.sort_by_key(|(_, v)| *v);
    tags.last().map(|(t, _)| (*t).to_string())
}

/// The commits since `tag` (all of them without one) that count toward the release, and the lines naming those that were
/// ignored (merges and non-conforming subjects). `chore(release):` commits are neither.
pub fn commits_since(root: &Path, tag: Option<&str>) -> Result<(Vec<Commit>, Vec<String>), String> {
    let range = tag
        .map(|t| format!("{t}..HEAD"))
        .unwrap_or_else(|| "HEAD".to_string());
    let out = Command::new("git")
        .args(["log", "--format=%H%x1f%P%x1f%B%x1e", &range])
        .current_dir(root)
        .output()
        .map_err(|e| format!("cannot run git: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "git log {range} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    let log = String::from_utf8_lossy(&out.stdout).into_owned();
    let mut commits = Vec::new();
    let mut ignored = Vec::new();
    for entry in log.split('\x1e') {
        let entry = entry.strip_prefix('\n').unwrap_or(entry);
        let mut fields = entry.splitn(3, '\x1f');
        let (Some(sha), Some(parents), Some(message)) =
            (fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        if sha.is_empty() {
            continue;
        }
        let subject = message.split('\n').next().unwrap_or("");
        let short: String = sha.chars().take(7).collect();
        if parents.split_whitespace().count() > 1 {
            ignored.push(format!("{short} {subject} (merge)"));
        } else if parse_commit(message).is_none() {
            ignored.push(format!("{short} {subject}"));
        } else if !subject.starts_with("chore(release):") {
            commits.push(Commit {
                sha: sha.to_string(),
                message: message.to_string(),
            });
        }
    }
    Ok((commits, ignored))
}
