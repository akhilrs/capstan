//! `cstan kb search|for|uses|status|build|refresh|add|update|remove`.
//!
//! The lookups and briefs are answered by the daemon's `kb` route (the index is built from the entries file at HEAD). The
//! three writers work in the caller's own working tree: they ask the daemon to check the entry against the controller's
//! caps (`kb check`), then rewrite the entries file atomically. They never commit.
use std::path::{Path, PathBuf};

use capstan_config::kb::{parse_file, remove, to_line, upsert_sorted};
use capstan_config::{KbEntry, KbKind};
use serde_json::Value;

use crate::{git, invalid, parse_options, Cli, Fail, Flow};

const BODY_PREVIEW: usize = 160;

/// A refusal is printed as `kb: ...` and ends the command with exit 1.
const REFUSED: i32 = 1;

fn to_json(value: &capstan_wire::js::Value) -> Option<Value> {
    serde_json::from_str(&capstan_wire::js::stringify(value, 0).to_utf8_lossy()).ok()
}

/// The first `BODY_PREVIEW` bytes of `body`, cut at a character boundary.
fn preview(body: &str) -> &str {
    if body.len() <= BODY_PREVIEW {
        return body;
    }
    let mut at = BODY_PREVIEW;
    while !body.is_char_boundary(at) {
        at -= 1;
    }
    &body[..at]
}

fn hit_lines(hits: &[Value]) -> Vec<String> {
    if hits.is_empty() {
        return vec!["kb: no entries".to_string()];
    }
    let text = |hit: &Value, key: &str| {
        hit.get(key)
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string()
    };
    let mut lines = Vec::new();
    for hit in hits {
        let path = hit.get("path").and_then(Value::as_str).unwrap_or("-");
        let mut line = format!(
            "{}  {}  {}  {}",
            text(hit, "id"),
            text(hit, "kind"),
            text(hit, "title"),
            path
        );
        if let Some(mark @ ("stale" | "missing")) = hit.get("state").and_then(Value::as_str) {
            line.push_str(&format!("  {mark}"));
        }
        lines.push(line);
        let body = text(hit, "body");
        if !body.is_empty() {
            lines.push(format!("    {}", preview(&body)));
        }
    }
    lines
}

fn status_lines(status: &Value) -> Vec<String> {
    let number = |key: &str| status.get(key).and_then(Value::as_u64).unwrap_or(0);
    let text = |key: &str| status.get(key).and_then(Value::as_str).unwrap_or("");
    let head = text("head");
    let mut lines = vec![
        format!(
            "kb: {}  head {}  entries {}  stale {}  missing {}",
            text("state"),
            &head[..head.len().min(12)],
            number("entries"),
            number("stale"),
            number("missing")
        ),
        format!(
            "searches in 30 days: {} ({} without a hit)",
            number("searches30d"),
            number("misses30d")
        ),
        format!("vector search: {}", text("vectorsHint")),
    ];
    for item in status
        .get("invalid")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        lines.push(format!(
            "invalid line {}: {}",
            item.get("line").and_then(Value::as_u64).unwrap_or(0),
            item.get("reason").and_then(Value::as_str).unwrap_or("")
        ));
    }
    lines
}

/// Whether the answer of the `kb` route is one `print` shows (the answer of `kb check` is not).
pub fn printable(result: Option<&capstan_wire::js::Value>) -> bool {
    result.is_some_and(|r| {
        ["refused", "hits", "status", "brief"]
            .iter()
            .any(|key| r.get(key).is_some())
    })
}

/// Prints the answer of the `kb` route; the exit code.
pub fn print(
    result: Option<&capstan_wire::js::Value>,
    json: bool,
    out: &mut Vec<u8>,
    err: &mut Vec<u8>,
) -> i32 {
    let Some(value) = result.and_then(to_json) else {
        return 0;
    };
    let line = |buf: &mut Vec<u8>, text: &str| {
        buf.extend_from_slice(text.as_bytes());
        buf.push(b'\n');
    };
    if let Some(why) = value.get("refused").and_then(Value::as_str) {
        line(err, why);
        return REFUSED;
    }
    if let Some(hits) = value.get("hits").and_then(Value::as_array) {
        if json {
            let shown: Vec<Value> = hits.clone();
            line(
                out,
                &serde_json::to_string_pretty(&shown).unwrap_or_default(),
            );
        } else {
            for text in hit_lines(hits) {
                line(out, &text);
            }
        }
    } else if let Some(status) = value.get("status") {
        if json {
            line(
                out,
                &serde_json::to_string_pretty(status).unwrap_or_default(),
            );
        } else {
            for text in status_lines(status) {
                line(out, &text);
            }
        }
    } else if let Some(brief) = value.get("brief").and_then(Value::as_str) {
        let role = value
            .get("role")
            .and_then(Value::as_str)
            .unwrap_or("developer");
        line(out, brief.trim_end_matches('\n'));
        line(
            err,
            &format!(
                "next: cstan spawn {role} --title \"knowledge base\" then cstan send <agent-id> with this brief"
            ),
        );
    }
    0
}

/// The flags of `add` and `update`.
#[derive(Default)]
struct Fields {
    id: Option<String>,
    kind: Option<String>,
    title: Option<String>,
    body: Option<String>,
    path: Option<String>,
    area: Option<String>,
    tags: Option<Vec<String>>,
    aliases: Option<Vec<String>>,
    related: Option<Vec<String>>,
    force: bool,
    verified: bool,
}

fn parse_fields(args: &[String]) -> Result<Fields, Fail> {
    let mut f = Fields::default();
    let mut rest = args.iter();
    while let Some(flag) = rest.next() {
        let value = |rest: &mut std::slice::Iter<'_, String>| {
            rest.next()
                .cloned()
                .ok_or_else(|| invalid(format!("{flag} needs a value")))
        };
        match flag.as_str() {
            "--id" => f.id = Some(value(&mut rest)?),
            "--kind" => f.kind = Some(value(&mut rest)?),
            "--title" => f.title = Some(value(&mut rest)?),
            "--body" => f.body = Some(value(&mut rest)?),
            "--path" => f.path = Some(value(&mut rest)?),
            "--area" => f.area = Some(value(&mut rest)?),
            "--tag" => f.tags.get_or_insert_with(Vec::new).push(value(&mut rest)?),
            "--alias" => f
                .aliases
                .get_or_insert_with(Vec::new)
                .push(value(&mut rest)?),
            "--related" => f
                .related
                .get_or_insert_with(Vec::new)
                .push(value(&mut rest)?),
            "--force" => f.force = true,
            "--verified" => f.verified = true,
            _ => return Err(Fail::Usage),
        }
    }
    Ok(f)
}

fn kind_of(text: &str) -> Result<KbKind, Fail> {
    KbKind::parse(text).ok_or_else(|| {
        invalid("kb: kind must be helper, component, convention, design_rule or decision")
    })
}

fn write_atomic(path: &Path, text: &str) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(format!(".tmp{}", std::process::id()));
    let temp = path.with_file_name(name);
    std::fs::write(&temp, text)?;
    std::fs::rename(&temp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&temp);
    })
}

impl Cli<'_> {
    /// `cstan kb ...`.
    pub fn kb(&mut self, rest: &[String]) -> Result<Flow, Fail> {
        let (positional, json) = parse_options(rest);
        let Some(verb) = positional.first().map(String::as_str) else {
            return Err(Fail::Usage);
        };
        match verb {
            "search" | "for" | "uses" | "status" | "build" | "refresh" => {
                self.run_routed("kb", &positional, json)
            }
            "add" | "update" | "remove" => self.kb_write(&positional),
            _ => Err(Fail::Usage),
        }
    }

    fn refuse(&mut self, why: impl AsRef<str>) -> Result<Flow, Fail> {
        self.io.err_line(why.as_ref());
        Ok(Flow::Code(REFUSED))
    }

    /// The top of the caller's working tree and its HEAD.
    fn kb_tree(&self) -> Result<(PathBuf, String), Fail> {
        let run = |args: &[&str]| {
            let out = git::git(&self.ctx.env, &self.ctx.cwd, args);
            (out.status == Some(0)).then(|| out.stdout.trim().to_string())
        };
        let top = run(&["rev-parse", "--show-toplevel"])
            .ok_or_else(|| invalid("kb: run this command inside a git working tree"))?;
        let head = run(&["rev-parse", "--verify", "HEAD"])
            .ok_or_else(|| invalid("kb: the working tree has no commit"))?;
        Ok((PathBuf::from(top), head))
    }

    /// Asks the daemon's `kb check`; `Ok(Err(code))` when it refused (the message is already printed).
    fn kb_check(&mut self, args: Vec<String>) -> Result<Result<(String, usize), i32>, Fail> {
        let before = self.io.out.len();
        let flow = self.run_routed("kb", &args, true)?;
        let shown = self.io.out.split_off(before);
        match flow {
            Flow::Code(0) => {
                let answer: Value = serde_json::from_slice(&shown)
                    .map_err(|_| invalid("kb: the controller's check answer could not be read"))?;
                let path = answer
                    .get("path")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let max = answer
                    .get("maxEntries")
                    .and_then(Value::as_u64)
                    .unwrap_or(0) as usize;
                Ok(Ok((path, max)))
            }
            Flow::Code(code) => Ok(Err(code)),
            _ => Ok(Err(REFUSED)),
        }
    }

    fn kb_write(&mut self, positional: &[String]) -> Result<Flow, Fail> {
        let verb = positional[0].as_str();
        let (flags, named) = match verb {
            "add" => (&positional[1..], None),
            _ => match positional.get(1) {
                Some(id) if !id.starts_with("--") => (&positional[2..], Some(id.clone())),
                _ => return Err(Fail::Usage),
            },
        };
        if verb == "remove" && !flags.is_empty() {
            return Err(Fail::Usage);
        }
        let fields = parse_fields(flags)?;
        if (fields.force && verb != "add") || (fields.verified && verb != "update") {
            return Err(Fail::Usage);
        }
        let id = match (named, fields.id.clone()) {
            (Some(_), Some(_)) => {
                return Err(invalid("kb: the id is the first word of update, not --id"))
            }
            (Some(id), None) | (None, Some(id)) => id,
            (None, None) => return Err(invalid("kb add needs --id, --kind, --title and --body")),
        };
        let (top, head) = self.kb_tree()?;
        // The configured file and cap come from the controller, which also refuses a missing [kb].
        let (path, max) = match self.kb_check(vec!["check".into(), "path".into()])? {
            Ok(answer) => answer,
            Err(code) => return Ok(Flow::Code(code)),
        };
        let file = top.join(&path);
        let current = std::fs::read_to_string(&file).unwrap_or_default();
        let found = parse_file(&current, usize::MAX)
            .entries
            .into_iter()
            .find(|e| e.id == id);
        let present = found.is_some();
        if verb == "remove" {
            let Some(text) = remove(&current, &id) else {
                return self.refuse(format!("kb: no entry {id} in {path}"));
            };
            self.kb_save(&file, &path, &text)?;
            self.io.out_line(&format!(
                "kb: removed {id} from {path}; commit it on your branch"
            ));
            return Ok(Flow::Code(0));
        }
        let mut entry = if verb == "add" {
            let (Some(kind), Some(title), Some(body)) = (&fields.kind, &fields.title, &fields.body)
            else {
                return Err(invalid("kb add needs --id, --kind, --title and --body"));
            };
            KbEntry {
                id: id.clone(),
                kind: kind_of(kind)?,
                title: title.clone(),
                body: body.clone(),
                path: None,
                area: None,
                tags: vec![],
                aliases: vec![],
                related: vec![],
                verified_commit: head.clone(),
            }
        } else {
            let Some(found) = found else {
                return self.refuse(format!("kb: no entry {id} in {path}"));
            };
            found
        };
        if verb == "update" && flags.is_empty() {
            return Err(invalid("kb update needs a field to change or --verified"));
        }
        if let Some(kind) = &fields.kind {
            entry.kind = kind_of(kind)?;
        }
        if let Some(title) = &fields.title {
            entry.title = title.clone();
        }
        if let Some(body) = &fields.body {
            entry.body = body.clone();
        }
        if fields.path.is_some() {
            entry.path = fields.path.clone();
        }
        if fields.area.is_some() {
            entry.area = fields.area.clone();
        }
        if let Some(tags) = &fields.tags {
            entry.tags = tags.clone();
        }
        if let Some(aliases) = &fields.aliases {
            entry.aliases = aliases.clone();
        }
        if let Some(related) = &fields.related {
            entry.related = related.clone();
        }
        entry.verified_commit = head;
        let mut check = vec!["check".to_string(), verb.to_string(), to_line(&entry)];
        if verb == "add" && fields.force {
            check.push("--force".into());
        }
        if let Err(code) = self.kb_check(check)? {
            return Ok(Flow::Code(code));
        }
        if verb == "add" {
            if present {
                return self.refuse(format!(
                    "kb: id {id} is already in {path}; use cstan kb update {id}"
                ));
            }
            let lines = current.lines().filter(|l| !l.trim().is_empty()).count();
            if max > 0 && lines >= max {
                return self.refuse(format!(
                    "kb: {path} already has {lines} entries, the limit of {max}; remove one first"
                ));
            }
        }
        self.kb_save(&file, &path, &upsert_sorted(&current, &entry))?;
        let (done, to) = if verb == "add" {
            ("added", "to")
        } else {
            ("updated", "in")
        };
        self.io.out_line(&format!(
            "kb: {done} {id} {to} {path}; commit it on your branch"
        ));
        Ok(Flow::Code(0))
    }

    fn kb_save(&self, file: &Path, shown: &str, text: &str) -> Result<(), Fail> {
        write_atomic(file, text)
            .map_err(|e| crate::runtime(format!("kb: could not write {shown}: {e}")))
    }
}
