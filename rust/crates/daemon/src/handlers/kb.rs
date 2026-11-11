//! The `kb` command: lookups in the controller's knowledge base index, the build and refresh briefs, and the check the
//! `cstan kb add` and `update` forms run before they write a line in the caller's working tree.
//!
//! The route is read-only but for the index's queries log. Every refusal is an `ok` answer with a `refused` member (the
//! CLI prints it and exits 1), so a refusal reads the same for every form.

use super::shared::{fail, ok, CommandCall, CommandEnv, CommandResponse, ErrorCode};
use super::HandlerMap;
use crate::deps::new_context;
use crate::kb::local::{self, Write};
use crate::kb::{self, BriefKind, Hit, KbError, KbIndex, Status};
use capstan_config::kb::{parse_file, to_line, validate, KbStorage};
use capstan_config::{KbConfig, KbEntry};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

const NOT_CONFIGURED: &str = "kb: not configured; add a [kb] table to capstan.toml";

/// Registers the handler of the route this module serves (kb).
pub fn register(map: &mut HandlerMap) {
    map.insert("kb", kb_command);
}

fn refused(message: impl Into<String>) -> Option<CommandResponse> {
    Some(ok(json!({ "refused": message.into() })))
}

fn usage(text: &str) -> Option<CommandResponse> {
    Some(fail(ErrorCode::InvalidRequest, text))
}

fn kb_error(error: &KbError) -> Option<CommandResponse> {
    match error {
        KbError::NotConfigured => refused(NOT_CONFIGURED),
        other => refused(format!("kb: {other}")),
    }
}

/// The entries of the file as committed at HEAD, by id (empty when the file is absent).
fn head_entries(env: &CommandEnv<'_>, cfg: &KbConfig) -> HashMap<String, KbEntry> {
    let spec = format!("HEAD:{}", cfg.path);
    let root = &env.deps.options.workspace_root;
    match kb::git(root, &["show", &spec]) {
        Ok((0, text)) => parse_file(&text, cfg.max_entry_bytes.max(0) as usize)
            .entries
            .into_iter()
            .map(|entry| (entry.id.clone(), entry))
            .collect(),
        _ => HashMap::new(),
    }
}

fn hit_value(hit: &Hit, bodies: &HashMap<String, KbEntry>) -> Value {
    json!({
        "id": hit.id, "kind": hit.kind, "title": hit.title, "path": hit.path, "area": hit.area,
        "state": hit.state, "reason": hit.reason, "uses": hit.uses,
        "body": bodies.get(&hit.id).map(|e| e.body.as_str()).unwrap_or(""),
    })
}

fn status_value(status: &Status) -> Value {
    json!({
        "state": status.state, "head": status.head, "entries": status.entries,
        "invalid": status.invalid.iter().map(|(line, reason)| json!({"line": line, "reason": reason})).collect::<Vec<_>>(),
        "stale": status.stale, "missing": status.missing,
        "searches30d": status.searches_30d, "misses30d": status.misses_30d,
        "vectorsHint": status.vectors_hint,
    })
}

/// A flag with a value anywhere in `args`; the rest stay in order.
fn take_value(args: &mut Vec<String>, flag: &str) -> Result<Option<String>, String> {
    let Some(at) = args.iter().position(|a| a == flag) else {
        return Ok(None);
    };
    args.remove(at);
    if at >= args.len() {
        return Err(format!("{flag} needs a value"));
    }
    Ok(Some(args.remove(at)))
}

fn take_flag(args: &mut Vec<String>, flag: &str) -> bool {
    match args.iter().position(|a| a == flag) {
        Some(at) => {
            args.remove(at);
            true
        }
        None => false,
    }
}

/// When the PM was last told of a knowledge base change, per state directory (one controller per directory).
static LAST_NOTE: Mutex<Vec<(PathBuf, Instant)>> = Mutex::new(Vec::new());

/// Whether a note may go out now (the time is recorded by `note_sent` once it is queued).
fn note_due(state_dir: &std::path::Path, window: Duration) -> bool {
    let last = LAST_NOTE.lock().unwrap_or_else(|p| p.into_inner());
    match last.iter().find(|(dir, _)| dir == state_dir) {
        Some((_, at)) => Instant::now().duration_since(*at) >= window,
        None => true,
    }
}

/// Records that a note went out now.
fn note_sent(state_dir: &std::path::Path) {
    let mut last = LAST_NOTE.lock().unwrap_or_else(|p| p.into_inner());
    let now = Instant::now();
    match last.iter_mut().find(|(dir, _)| dir == state_dir) {
        Some((_, at)) => *at = now,
        None => last.push((state_dir.to_path_buf(), now)),
    }
}

/// Test seam: the next note's enqueue fails.
static FAIL_NOTE: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());

/// Makes the next PM note fail to queue, so a test can see that a failed note keeps the window open.
#[doc(hidden)]
pub fn fail_next_note(state_dir: &std::path::Path) {
    FAIL_NOTE
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .push(state_dir.to_path_buf());
}

fn note_fails(state_dir: &std::path::Path) -> bool {
    let mut dirs = FAIL_NOTE.lock().unwrap_or_else(|p| p.into_inner());
    match dirs.iter().position(|d| d == state_dir) {
        Some(at) => {
            dirs.remove(at);
            true
        }
        None => false,
    }
}

/// Moves the last note of `state_dir` `ago` into the past, so a test can cross the note window without waiting.
#[doc(hidden)]
pub fn age_last_note(state_dir: &std::path::Path, ago: Duration) {
    let mut last = LAST_NOTE.lock().unwrap_or_else(|p| p.into_inner());
    if let Some((_, at)) = last.iter_mut().find(|(dir, _)| dir == state_dir) {
        *at = at.checked_sub(ago).unwrap_or(*at);
    }
}

/// Who is calling: (name, kind or role) and whether the caller is the PM or the operator.
fn caller(env: &CommandEnv<'_>, call: &CommandCall<'_>) -> (String, String, bool, bool) {
    let agent = env.agent_of(call.identity);
    let name = match agent {
        Some(a) => a.agent_id.clone(),
        None if call.identity.role == "operator" => "operator".to_string(),
        None => call.identity.actor_id.clone(),
    };
    let kind = agent
        .map(|a| a.kind.clone())
        .unwrap_or_else(|| call.identity.role.clone());
    let operator = call.identity.role == "operator";
    let pm = agent.is_some_and(|a| a.kind == "PM");
    (name, kind, pm, operator)
}

/// One info message to the active PM about a change, at most one per `pm_note_minutes` (none when that is 0 or there is
/// no PM; a project with no PM does not spend the window).
fn tell_pm(env: &CommandEnv<'_>, cfg: &KbConfig, body: String) {
    if cfg.pm_note_minutes <= 0 {
        return;
    }
    let state_dir = env.deps.options.state_directory.clone();
    let window = Duration::from_secs(cfg.pm_note_minutes as u64 * 60);
    let controller = env.deps.credential().to_string();
    let sent = env.deps.kernel.run(move |core| {
        let agents = core.active_agents()?;
        let pm = agents
            .as_array()
            .into_iter()
            .flatten()
            .find(|a| a["kind"] == "PM")
            .and_then(|a| a["agentId"].as_str().map(str::to_string));
        let Some(pm) = pm else {
            return Ok(Some(false));
        };
        if !note_due(&state_dir, window) {
            return Ok(None);
        }
        if note_fails(&state_dir) {
            return Err(capstan_kernel::KernelError::controller(
                "note enqueue failed (test)",
            ));
        }
        let context = new_context(core, &controller)?;
        core.enqueue_message(
            &context,
            &json!({"recipientAgentId": pm, "body": body, "class": "info"}),
        )?;
        note_sent(&state_dir);
        Ok(Some(true))
    });
    match sent {
        Ok(Some(false)) => env.log("kb_note_skipped", json!({"reason": "no active PM"})),
        Ok(_) => {}
        Err(error) => env.log("kb_note_failed", json!({"error": error.to_string()})),
    }
}

fn parse_entry(cfg: &KbConfig, text: &str) -> Result<KbEntry, String> {
    let max = cfg.max_entry_bytes.max(0) as usize;
    let parsed = parse_file(text, max);
    parsed.entries.into_iter().next().ok_or_else(|| {
        parsed
            .invalid
            .first()
            .map(|(_, reason)| reason.clone())
            .unwrap_or_else(|| "kb entry: not an entry".to_string())
    })
}

fn change_value(change: &local::Change) -> Value {
    json!({
        "at": change.at, "agent": change.agent, "role": change.role, "verb": change.verb,
        "id": change.id, "title": change.title, "count": change.count,
    })
}

fn require_local(cfg: &KbConfig) -> Result<(), String> {
    if cfg.storage == KbStorage::Local {
        Ok(())
    } else {
        Err(
            "kb: [kb] storage is repo; edit the entries file with cstan kb add in your working tree and commit it"
                .to_string(),
        )
    }
}

fn write_verb(
    env: &CommandEnv<'_>,
    call: &CommandCall<'_>,
    cfg: &KbConfig,
    args: &mut Vec<String>,
) -> Option<CommandResponse> {
    if let Err(why) = require_local(cfg) {
        return refused(why);
    }
    let force = take_flag(args, "--force");
    let expect = match take_value(args, "--expect") {
        Ok(v) => v,
        Err(why) => return usage(&why),
    };
    let (Some(sub), Some(arg)) = (args.first().cloned(), args.get(1).cloned()) else {
        return usage(
            "kb write takes add <entry-json> [--force], update <entry-json> or remove <id>",
        );
    };
    if args.len() != 2 || (force && sub != "add") || (expect.is_some() && sub == "add") {
        return usage(
            "kb write takes add <entry-json> [--force], update <entry-json> or remove <id>",
        );
    }
    let change = match sub.as_str() {
        "add" | "update" => match parse_entry(cfg, &arg) {
            Ok(entry) if sub == "add" => Write::Add { entry, force },
            Ok(entry) => Write::Update { entry, expect },
            Err(why) => return refused(why),
        },
        "remove" => Write::Remove { id: arg, expect },
        _ => {
            return usage(
                "kb write takes add <entry-json> [--force], update <entry-json> or remove <id>",
            )
        }
    };
    let (name, kind, pm, operator) = caller(env, call);
    let state = &env.deps.options.state_directory;
    let root = &env.deps.options.workspace_root;
    let done = match local::write(state, root, cfg, change, (&name, &kind)) {
        Ok(done) => done,
        Err(why) => return refused(why),
    };
    env.log(
        "kb_change",
        json!({"agent": name, "verb": done.verb, "id": done.id}),
    );
    if !pm && !operator {
        let past = match done.verb.as_str() {
            "add" => "added",
            "update" => "updated",
            _ => "removed",
        };
        tell_pm(
            env,
            cfg,
            format!(
                "kb: {name} {past} {} ({}); later changes within {} minutes are not announced; cstan kb log lists every change",
                done.id, done.title, cfg.pm_note_minutes
            ),
        );
    }
    Some(ok(
        json!({"saved": kb::location(state, root, cfg), "verb": done.verb, "id": done.id}),
    ))
}

/// How long an unfinished staged import stays before the next `import-part` removes it.
const PART_MAX_AGE: Duration = Duration::from_secs(3600);

fn part_path(state: &std::path::Path, token: &str) -> Option<PathBuf> {
    let valid = token.len() == 16
        && token
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
    valid.then(|| state.join("kb").join(format!("import-{token}.part")))
}

/// Removes staged imports older than an hour.
fn sweep_parts(state: &std::path::Path) {
    let Ok(dir) = std::fs::read_dir(state.join("kb")) else {
        return;
    };
    for item in dir.flatten() {
        let name = item.file_name();
        let name = name.to_string_lossy();
        if !(name.starts_with("import-") && name.ends_with(".part")) {
            continue;
        }
        let old = item
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age > PART_MAX_AGE);
        if old {
            let _ = std::fs::remove_file(item.path());
        }
    }
}

/// `import-part <token> <chunk>` and `import <token> [--replace]`: the chunks are staged in a part file and applied as one
/// write, so an import is all or nothing and writes one change line.
fn import_verb(
    env: &CommandEnv<'_>,
    call: &CommandCall<'_>,
    cfg: &KbConfig,
    verb: &str,
    args: &mut Vec<String>,
) -> Option<CommandResponse> {
    if let Err(why) = require_local(cfg) {
        return refused(why);
    }
    let (name, kind, pm, operator) = caller(env, call);
    if !pm && !operator {
        return refused("kb: import is for the PM or the operator");
    }
    let state = &env.deps.options.state_directory;
    let root = &env.deps.options.workspace_root;
    let part = verb == "import-part";
    let replace = !part && take_flag(args, "--replace");
    if args.len() != if part { 2 } else { 1 } {
        return usage(if part {
            "kb import-part takes a token and a chunk of entry lines"
        } else {
            "kb import takes a token and optionally --replace"
        });
    }
    let Some(path) = part_path(state, &args[0]) else {
        return usage("kb import: the token is 16 lowercase hex characters");
    };
    if part {
        sweep_parts(state);
        let cap = (cfg.max_entries.max(0) as u64).saturating_mul(cfg.max_entry_bytes.max(0) as u64);
        let have = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        if have + args[1].len() as u64 > cap {
            let _ = std::fs::remove_file(&path);
            return refused(format!(
                "kb: the import is over the limit of {} entries of {} bytes; nothing was imported",
                cfg.max_entries, cfg.max_entry_bytes
            ));
        }
        let written = (|| {
            use std::io::Write as _;
            std::fs::create_dir_all(state.join("kb"))?;
            let mut options = std::fs::OpenOptions::new();
            options.create(true).append(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            options.open(&path)?.write_all(args[1].as_bytes())
        })();
        return match written {
            Ok(()) => Some(ok(json!({"staged": args[1].len()}))),
            Err(e) => {
                let _ = std::fs::remove_file(&path);
                refused(format!("kb: local store: {e}"))
            }
        };
    }
    let text = std::fs::read_to_string(&path);
    let _ = std::fs::remove_file(&path);
    let Ok(text) = text else {
        return refused("kb: no staged import for that token; run cstan kb import again");
    };
    let result = match local::import(state, root, cfg, &text, replace, (&name, &kind)) {
        Ok(result) => result,
        Err(why) => return refused(why),
    };
    env.log(
        "kb_change",
        json!({"agent": name, "verb": "import", "id": "", "added": result.added, "replaced": result.replaced}),
    );
    Some(ok(json!({
        "added": result.added, "replaced": result.replaced, "skipped": result.skipped,
        "invalid": result.invalid.iter().map(|(line, reason)| json!({"line": line, "reason": reason})).collect::<Vec<_>>(),
        "saved": kb::location(state, root, cfg),
    })))
}

fn check(
    env: &CommandEnv<'_>,
    index: &KbIndex,
    cfg: &KbConfig,
    args: &[String],
) -> Option<CommandResponse> {
    let state = &env.deps.options.state_directory;
    let place = json!({
        "path": cfg.path, "maxEntries": cfg.max_entries, "storage": cfg.storage.as_str(),
        "location": kb::location(state, &env.deps.options.workspace_root, cfg),
    });
    if args == ["path"] {
        return Some(ok(place));
    }
    let (verb, text) = match args {
        [verb, text] if verb == "add" || verb == "update" => (verb.as_str(), text.as_str()),
        [verb, text, flag] if verb == "add" && flag == "--force" => (verb.as_str(), text.as_str()),
        _ => {
            return usage("kb check takes path, add <entry-json> [--force] or update <entry-json>")
        }
    };
    let force = args.len() == 3;
    let max = cfg.max_entry_bytes.max(0) as usize;
    let parsed = parse_file(text, max);
    let Some(entry) = parsed.entries.into_iter().next() else {
        let why = parsed
            .invalid
            .first()
            .map(|(_, reason)| reason.clone())
            .unwrap_or_else(|| "kb entry: not an entry".to_string());
        return refused(why);
    };
    if let Err(why) = validate(&entry, max) {
        return refused(why);
    }
    if verb == "add" {
        let exists = if cfg.storage == KbStorage::Local {
            local::entry(state, &env.deps.options.workspace_root, cfg, &entry.id).is_some()
        } else {
            head_entries(env, cfg).contains_key(&entry.id)
        };
        if exists {
            return refused(format!(
                "kb: id {0} exists; use cstan kb update {0}",
                entry.id
            ));
        }
        if !force {
            match index.similar(&entry) {
                Ok(ids) if !ids.is_empty() => {
                    return refused(format!(
                        "kb: similar entries exist: {}; reuse or update one, or add --force",
                        ids.join(", ")
                    ))
                }
                Ok(_) => {}
                Err(error) => return kb_error(&error),
            }
        }
    }
    Some(ok(place))
}

fn kb_command(env: &CommandEnv<'_>, call: &CommandCall<'_>) -> Option<CommandResponse> {
    let Some(cfg) = env
        .config()
        .and_then(|c| c.kb.clone())
        .filter(|k| k.enabled)
    else {
        return refused(NOT_CONFIGURED);
    };
    let mut args: Vec<String> = call.args.to_vec();
    if args.is_empty() {
        return usage("kb takes search, for, uses, status, build, refresh, get, write, import, log, export, add, update or remove");
    }
    let verb = args.remove(0);
    let root = env.deps.options.workspace_root.clone();
    let index = match kb::ensure(&env.deps.options.state_directory, &root, &cfg) {
        Ok(index) => index,
        Err(error) => return kb_error(&error),
    };
    match verb.as_str() {
        "search" => {
            let limit = match take_value(&mut args, "--limit") {
                Ok(Some(n)) => match n.parse::<usize>() {
                    Ok(n) if n >= 1 => n,
                    _ => return usage("--limit takes a number of at least 1"),
                },
                Ok(None) => cfg.search_limit.max(1) as usize,
                Err(why) => return usage(&why),
            };
            if args.is_empty() {
                return usage("kb search takes one or more words");
            }
            match index.search(&args, limit) {
                Ok(hits) => {
                    let bodies = head_entries(env, &cfg);
                    Some(ok(json!({
                        "hits": hits.iter().map(|h| hit_value(h, &bodies)).collect::<Vec<_>>()
                    })))
                }
                Err(error) => kb_error(&error),
            }
        }
        "for" | "uses" => {
            if args.is_empty() || (verb == "uses" && args.len() != 1) {
                return usage(if verb == "for" {
                    "kb for takes one or more paths"
                } else {
                    "kb uses takes one entry id or path"
                });
            }
            let found = if verb == "for" {
                index.for_paths(&args)
            } else {
                index.uses(&args[0])
            };
            match found {
                Ok(hits) => {
                    let bodies = head_entries(env, &cfg);
                    Some(ok(json!({
                        "hits": hits.iter().map(|h| hit_value(h, &bodies)).collect::<Vec<_>>()
                    })))
                }
                Err(error) => kb_error(&error),
            }
        }
        "status" => {
            if !args.is_empty() {
                return usage("kb status takes no arguments");
            }
            match index.status() {
                Ok(status) => Some(ok(json!({ "status": status_value(&status) }))),
                Err(error) => kb_error(&error),
            }
        }
        "build" | "refresh" => {
            let design = take_flag(&mut args, "--design");
            if !args.is_empty() {
                return usage("kb build and kb refresh take only --design");
            }
            let kind = if verb == "build" {
                BriefKind::Build
            } else {
                BriefKind::Refresh
            };
            let role = if design { &cfg.design_role } else { &cfg.role };
            Some(ok(json!({
                "brief": kb::brief(&index, &cfg, kind, design),
                "role": role,
            })))
        }
        "check" => check(env, &index, &cfg, &args),
        "get" => {
            if args.len() != 1 {
                return usage("kb get takes one entry id");
            }
            let state = &env.deps.options.state_directory;
            match local::entry(state, &root, &cfg, &args[0]) {
                Some(entry) => Some(ok(json!({"entry": to_line(&entry), "sha": local::line_sha(&entry)}))),
                None => refused(format!(
                    "kb: no entry {} in {}",
                    args[0],
                    kb::location(state, &root, &cfg)
                )),
            }
        }
        "write" => write_verb(env, call, &cfg, &mut args),
        "import" | "import-part" => import_verb(env, call, &cfg, &verb, &mut args),
        "log" => {
            if let Err(why) = require_local(&cfg) {
                return refused(why);
            }
            let max = cfg.log_max_entries.max(1) as usize;
            let limit = match take_value(&mut args, "--limit") {
                Ok(Some(n)) => match n.parse::<usize>() {
                    Ok(n) if n >= 1 => n.min(max),
                    _ => return usage("--limit takes a number of at least 1"),
                },
                Ok(None) => 20.min(max),
                Err(why) => return usage(&why),
            };
            if !args.is_empty() {
                return usage("kb log takes only --limit");
            }
            let changes = local::log(&env.deps.options.state_directory, limit);
            Some(ok(json!({"changes": changes.iter().map(change_value).collect::<Vec<_>>()})))
        }
        "export" => {
            if !args.is_empty() {
                return usage("kb export takes no arguments");
            }
            let text = kb::source(&env.deps.options.state_directory, &root, &cfg)
                .ok()
                .flatten()
                .unwrap_or_default();
            Some(ok(json!({"text": text})))
        }
        _ => usage("kb takes search, for, uses, status, build, refresh, get, write, import, log, export, add, update or remove"),
    }
}
