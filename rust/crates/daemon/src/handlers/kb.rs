//! The `kb` command: lookups in the controller's knowledge base index, the build and refresh briefs, and the check the
//! `cstan kb add` and `update` forms run before they write a line in the caller's working tree.
//!
//! The route is read-only but for the index's queries log. Every refusal is an `ok` answer with a `refused` member (the
//! CLI prints it and exits 1), so a refusal reads the same for every form.

use super::shared::{fail, ok, CommandCall, CommandEnv, CommandResponse, ErrorCode};
use super::HandlerMap;
use crate::kb::{self, BriefKind, Hit, KbError, KbIndex, Status};
use capstan_config::kb::{parse_file, validate};
use capstan_config::{KbConfig, KbEntry};
use serde_json::{json, Value};
use std::collections::HashMap;

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

fn check(
    env: &CommandEnv<'_>,
    index: &KbIndex,
    cfg: &KbConfig,
    args: &[String],
) -> Option<CommandResponse> {
    if args == ["path"] {
        return Some(ok(json!({"path": cfg.path, "maxEntries": cfg.max_entries})));
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
        if head_entries(env, cfg).contains_key(&entry.id) {
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
    Some(ok(json!({"path": cfg.path, "maxEntries": cfg.max_entries})))
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
        return usage("kb takes search, for, uses, status, build, refresh, add, update or remove");
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
        _ => usage("kb takes search, for, uses, status, build, refresh, add, update or remove"),
    }
}
