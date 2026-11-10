//! Staleness of entries against git, and the plan-open warnings built on the index.
use std::collections::{HashMap, HashSet};
use std::path::Path;

use capstan_config::{Design, KbConfig, KbEntry};

use super::text::owned_paths;
use super::{ensure, git};

const MAX_COMMITS_CHECKED: usize = 300;
const MAX_LOG_COMMITS: usize = 2000;
const RULES_RECORD: &str = "docs/design/rules.md";

/// The state and reason of one entry: ("ok", ""), ("stale", why) or ("missing", why).
pub(crate) type Check = (&'static str, String);

fn tree(root: &Path) -> Result<Vec<String>, String> {
    match git(root, &["ls-tree", "-r", "-z", "--name-only", "HEAD"])? {
        (0, out) => Ok(nul_names(&out)),
        _ => Err("git could not list the files at HEAD".to_string()),
    }
}

/// Names from `-z` git output: split on NUL, so unusual bytes are never quoted.
fn nul_names(out: &str) -> Vec<String> {
    out.split('\0')
        .filter(|n| !n.is_empty())
        .map(str::to_string)
        .collect()
}

fn under(file: &str, path: &str) -> bool {
    let dir = path.trim_end_matches('/');
    file == dir
        || (file.len() > dir.len() && file.starts_with(dir) && file.as_bytes()[dir.len()] == b'/')
}

fn short(sha: &str) -> &str {
    &sha[..sha.len().min(12)]
}

/// One check per entry, in order. A git failure marks every entry with a path as stale ('not checked').
pub(crate) fn check(root: &Path, _head: &str, entries: &[KbEntry]) -> Vec<Check> {
    let unchecked = || -> Vec<Check> {
        entries
            .iter()
            .map(|e| match e.path {
                Some(_) => ("stale", "not checked".to_string()),
                None => ("ok", String::new()),
            })
            .collect()
    };
    if entries.iter().all(|e| e.path.is_none()) {
        return unchecked();
    }
    let Ok(files) = tree(root) else {
        return unchecked();
    };
    let mut changed: HashMap<String, Option<Vec<String>>> = HashMap::new();
    let mut checked = 0usize;
    let mut out = Vec::with_capacity(entries.len());
    for entry in entries {
        let Some(path) = &entry.path else {
            out.push(("ok", String::new()));
            continue;
        };
        if !files.iter().any(|f| under(f, path)) {
            out.push(("missing", "not at HEAD".to_string()));
            continue;
        }
        let sha = entry.verified_commit.as_str();
        if !changed.contains_key(sha) {
            if checked >= MAX_COMMITS_CHECKED {
                out.push(("stale", "not checked".to_string()));
                continue;
            }
            checked += 1;
            let hex = !sha.is_empty() && sha.bytes().all(|b| b.is_ascii_hexdigit());
            let list = if hex {
                match git(root, &["diff", "--name-only", "-z", sha, "HEAD", "--"]) {
                    Ok((0, text)) => Some(nul_names(&text)),
                    _ => None,
                }
            } else {
                None
            };
            changed.insert(sha.to_string(), list);
        }
        match &changed[sha] {
            None => out.push(("stale", "verified commit unknown".to_string())),
            Some(list) if list.iter().any(|f| under(f, path)) => {
                out.push(("stale", format!("changed since {}", short(sha))))
            }
            Some(_) => out.push(("ok", String::new())),
        }
    }
    out
}

/// Whether `text` matches the glob `pattern`: `**` crosses `/`, `*` and `?` do not.
pub(crate) fn glob_match(pattern: &str, text: &str) -> bool {
    fn go(p: &[u8], t: &[u8]) -> bool {
        match p.first() {
            None => t.is_empty(),
            Some(b'*') if p.get(1) == Some(&b'*') => {
                let rest = &p[2..];
                let rest = rest.strip_prefix(b"/").unwrap_or(rest);
                (0..=t.len()).any(|i| go(rest, &t[i..]))
            }
            Some(b'*') => {
                let mut i = 0;
                loop {
                    if go(&p[1..], &t[i..]) {
                        return true;
                    }
                    if i >= t.len() || t[i] == b'/' {
                        return false;
                    }
                    i += 1;
                }
            }
            Some(b'?') => !t.is_empty() && t[0] != b'/' && go(&p[1..], &t[1..]),
            Some(c) => t.first() == Some(c) && go(&p[1..], &t[1..]),
        }
    }
    go(pattern.as_bytes(), text.as_bytes())
}

fn record_paths(design: &Design) -> Vec<String> {
    let mut paths = design.design_record.clone();
    if !paths.iter().any(|p| p == RULES_RECORD) {
        paths.push(RULES_RECORD.to_string());
    }
    paths
}

fn design_warning(
    root: &Path,
    cfg: &KbConfig,
    ui_owns: Option<&[String]>,
    design: &Design,
) -> Result<Option<String>, String> {
    if cfg.ui_paths.is_empty() {
        return Ok(None);
    }
    let files = tree(root)?;
    let ui_files: Vec<&String> = files
        .iter()
        .filter(|f| cfg.ui_paths.iter().any(|g| glob_match(g, f)))
        .collect();
    if ui_files.is_empty() {
        return Ok(None);
    }
    if let Some(owns) = ui_owns {
        let owned = owned_paths(owns);
        let touches = owned
            .iter()
            .any(|o| o.is_empty() || ui_files.iter().any(|f| under(f, o)));
        if !touches {
            return Ok(None);
        }
    }
    let records = record_paths(design);
    let set: HashSet<&str> = files.iter().map(String::as_str).collect();
    let missing: Vec<&str> = records
        .iter()
        .map(String::as_str)
        .filter(|r| !set.contains(r))
        .collect();
    if !missing.is_empty() {
        return Ok(Some(format!(
            "design record: {} missing; next: cstan kb build --design and send its brief to a {} agent",
            missing.join(", "),
            cfg.design_role
        )));
    }
    let mut args: Vec<&str> = vec!["log", "-1", "--format=%H", "HEAD", "--"];
    args.extend(records.iter().map(String::as_str));
    let last = match git(root, &args)? {
        (0, out) => out.trim().to_string(),
        _ => return Err("git could not read the record's history".to_string()),
    };
    if last.is_empty() {
        return Ok(None);
    }
    let range = format!("{last}..HEAD");
    let cap = (cfg.design_stale_commits.max(0) as usize).max(MAX_LOG_COMMITS);
    let limit = format!("-n{cap}");
    let pathspecs: Vec<String> = cfg.ui_paths.iter().map(|g| format!(":(glob){g}")).collect();
    let mut args: Vec<&str> = vec!["log", "--format=%H", &limit, &range, "--"];
    args.extend(pathspecs.iter().map(String::as_str));
    let behind = match git(root, &args)? {
        (0, out) => out.lines().filter(|l| !l.is_empty()).count() as i64,
        _ => return Err("git could not count the UI commits".to_string()),
    };
    if behind > 0 && behind >= cfg.design_stale_commits {
        let first = design
            .design_record
            .first()
            .map(String::as_str)
            .unwrap_or(RULES_RECORD);
        return Ok(Some(format!(
            "design record: {first} is {behind} UI commits behind; next: cstan kb refresh --design"
        )));
    }
    Ok(None)
}

/// The warnings the controller shows at plan open and spawn. A git or store failure gives no warning.
pub fn warnings(
    state_dir: &Path,
    root: &Path,
    cfg: &KbConfig,
    ui_owns: Option<&[String]>,
    design: &Design,
) -> Vec<String> {
    let mut out = Vec::new();
    if !cfg.enabled {
        return out;
    }
    let failed = |why: &str| eprintln!("kb_check_failed: {why}");
    match ensure(state_dir, root, cfg) {
        Err(e) => {
            failed(&e.to_string());
            return out;
        }
        Ok(index) => match index.status() {
            Err(e) => {
                failed(&e.to_string());
                return out;
            }
            Ok(status) if status.state == "missing" => out.push(format!(
                "kb: no knowledge base at {}; next: cstan kb build and send its brief to a {} agent",
                cfg.path, cfg.role
            )),
            Ok(status) => {
                if let Some((line, _)) = status.invalid.first() {
                    out.push(format!(
                        "kb: {} invalid lines in {} (first at line {line}); next: cstan kb status",
                        status.invalid.len(),
                        cfg.path
                    ));
                }
                let flagged = (status.stale + status.missing) as i64;
                if flagged > 0 && flagged >= cfg.stale_warn {
                    out.push(format!(
                        "kb: {} stale and {} missing entries in {}; next: cstan kb refresh",
                        status.stale, status.missing, cfg.path
                    ));
                }
            }
        },
    }
    match design_warning(root, cfg, ui_owns, design) {
        Ok(Some(w)) => out.push(w),
        Ok(None) => {}
        Err(e) => failed(&e),
    }
    out
}
