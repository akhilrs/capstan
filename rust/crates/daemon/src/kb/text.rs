//! The lines and briefs the controller shows: owned paths, Reuse lines and the build and refresh briefs.
use capstan_config::KbConfig;

use super::index::KbIndex;

const BRIEF_LIMIT: usize = 2900;

/// Build a knowledge base or refresh its stale entries.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BriefKind {
    Build,
    Refresh,
}

/// The first word of each owns entry, cut before its first glob character.
pub fn owned_paths(owns: &[String]) -> Vec<String> {
    owns.iter()
        .filter_map(|entry| entry.split_whitespace().next())
        .map(|word| {
            let cut = word.find(['*', '?', '[', '{']).unwrap_or(word.len());
            word[..cut].trim_end_matches('/').to_string()
        })
        .collect()
}

fn cut_to(mut text: String, max: usize) -> String {
    if text.len() <= max {
        return text;
    }
    let mut at = max;
    while !text.is_char_boundary(at) {
        at -= 1;
    }
    text.truncate(at);
    text
}

/// The entries that cover what a package owns, as lines for its brief; None when there are none or `max_bytes` is 0.
pub fn reuse_lines(index: &KbIndex, owns: &[String], max_bytes: usize) -> Option<String> {
    if max_bytes == 0 {
        return None;
    }
    let paths = owned_paths(owns);
    let hits = index.for_paths(&paths).ok()?;
    if hits.is_empty() {
        return None;
    }
    let mut lines: Vec<String> = Vec::new();
    for hit in &hits {
        let place = hit.path.as_deref().or(hit.area.as_deref()).unwrap_or("");
        let mark = match hit.state.as_str() {
            "stale" => " [stale]",
            "missing" => " [missing]",
            _ => "",
        };
        lines.push(format!(
            "- {} ({}) {}: {place}{mark}",
            hit.id, hit.kind, hit.title
        ));
    }
    let first = paths.iter().find(|p| !p.is_empty()).cloned();
    let more = first.map(|f| format!("- (more: cstan kb for {f})"));
    let more_len = more.as_ref().map_or(0, |m| m.len() + 1);
    let mut out = String::new();
    for (i, line) in lines.iter().enumerate() {
        let rest = i + 1 < lines.len();
        let needed = out.len() + line.len() + 1 + if rest { more_len } else { 0 };
        if needed > max_bytes {
            if i == 0 {
                out.push_str(&cut_to(line.clone(), max_bytes));
                out.push('\n');
            } else if let Some(m) = &more {
                if out.len() + m.len() < max_bytes {
                    out.push_str(m);
                    out.push('\n');
                }
            }
            break;
        }
        out.push_str(line);
        out.push('\n');
    }
    let out = out.trim_end().to_string();
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

/// The brief for the agent that builds or refreshes the knowledge base (or the design record when `design`).
pub fn brief(index: &KbIndex, cfg: &KbConfig, kind: BriefKind, design: bool) -> String {
    let mut text = String::new();
    if design {
        let who = &cfg.design_role;
        match kind {
            BriefKind::Build => text.push_str(&format!(
                "Role: {who}. Write the design record ([design] design_record, and docs/design/rules.md) from the \
                 existing screens and code at HEAD, following cstan design guide. Commit the files on your branch and \
                 report.\n"
            )),
            BriefKind::Refresh => text.push_str(&format!(
                "Role: {who}. Update the design record ([design] design_record, and docs/design/rules.md) for the UI \
                 files changed since the record's last commit (git log <that commit>..HEAD -- {}), following cstan \
                 design guide. Commit the files on your branch and report.\n",
                if cfg.ui_paths.is_empty() { "<ui paths>".to_string() } else { cfg.ui_paths.join(" ") }
            )),
        }
        return cut_to(text, BRIEF_LIMIT);
    }
    let who = &cfg.role;
    match kind {
        BriefKind::Build => text.push_str(&format!(
            "Role: {who}. Build the knowledge base at {path}. Read the code at HEAD and add entries with cstan kb add for \
             reusable helpers, modules, components, conventions, design rules and decisions. One fact per entry, no code. \
             Caps: at most {entries} entries, {bytes} bytes per entry. Then commit {path} on your branch and report.\n",
            path = cfg.path,
            entries = cfg.max_entries,
            bytes = cfg.max_entry_bytes
        )),
        BriefKind::Refresh => {
            text.push_str(&format!(
                "Role: {who}. Refresh the knowledge base at {path}. For each entry below, re-read the code at its path and \
                 run cstan kb update <id> (add --verified when it is still true) or cstan kb remove <id>. Caps: \
                 {bytes} bytes per entry. Then commit {path} on your branch and report.\nEntries:\n",
                path = cfg.path,
                bytes = cfg.max_entry_bytes
            ));
            let items = index.attention(50).unwrap_or_default();
            if items.is_empty() {
                text.push_str("(none are stale or missing)\n");
            }
            for (id, path, reason) in items {
                let line = format!("- {id} {path}: {reason}\n");
                if text.len() + line.len() > BRIEF_LIMIT {
                    text.push_str("- (more: cstan kb status)\n");
                    break;
                }
                text.push_str(&line);
            }
        }
    }
    cut_to(text, BRIEF_LIMIT)
}
