//! `cstan guard-write <memory-dir>`: the PreToolUse hook that lets a PM write only inside its own Claude Code memory
//! folder. Claude Code gives the hook the tool call as JSON on stdin; exit 0 lets the call go on to the normal
//! permission rules, exit 2 refuses it. Anything the guard cannot be sure about is refused (fail closed).

use serde_json::Value;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

/// The most stdin the guard reads; a Write call carries the whole file content, so this is generous.
const MAX_INPUT: u64 = 64 * 1024 * 1024;

/// Reads the hook input from stdin and decides. `Err` carries the reason printed on stderr.
pub fn run(memory_dir: &str) -> Result<(), String> {
    let mut input = String::new();
    std::io::stdin()
        .take(MAX_INPUT)
        .read_to_string(&mut input)
        .map_err(|_| "cannot read the tool call".to_string())?;
    check(memory_dir, &input)
}

/// Reads the hook input from stdin and decides for a role with write folders.
pub fn run_dirs(dirs: &[String]) -> Result<(), String> {
    let mut input = String::new();
    std::io::stdin()
        .take(MAX_INPUT)
        .read_to_string(&mut input)
        .map_err(|_| "cannot read the tool call".to_string())?;
    check_dirs(dirs, &input)
}

/// Allows a Write, Edit or MultiEdit whose real path lies strictly inside one of `dirs`.
pub fn check_dirs(dirs: &[String], input: &str) -> Result<(), String> {
    let (target, path) = target_of(input)?;
    let mut resolved = Vec::new();
    for dir in dirs {
        resolved.push(real_path(dir).map_err(|_| "a write folder cannot be resolved".to_string())?);
    }
    if resolved
        .iter()
        .any(|dir| target != *dir && target.starts_with(dir))
    {
        Ok(())
    } else {
        Err(format!(
            "this role may write only under {}; {path} is outside them",
            dirs.join(", ")
        ))
    }
}

/// Allows a Write, Edit or MultiEdit whose real path lies strictly inside `memory_dir`.
pub fn check(memory_dir: &str, input: &str) -> Result<(), String> {
    let (target, path) = target_of(input)?;
    let memory =
        real_path(memory_dir).map_err(|_| "the memory folder cannot be resolved".to_string())?;
    if target != memory && target.starts_with(&memory) {
        Ok(())
    } else {
        Err(format!(
            "a PM may write only under its memory folder ({}); {path} is outside it",
            memory.display()
        ))
    }
}

/// The real path of the file a Write, Edit or MultiEdit call names, with the path as given.
fn target_of(input: &str) -> Result<(PathBuf, String), String> {
    let call: Value =
        serde_json::from_str(input).map_err(|_| "the tool call is not valid JSON".to_string())?;
    match call.get("tool_name").and_then(Value::as_str) {
        Some("Write" | "Edit" | "MultiEdit") => {}
        _ => return Err("only Write, Edit and MultiEdit can be checked".into()),
    }
    let path = call
        .get("tool_input")
        .and_then(|input| input.get("file_path"))
        .and_then(Value::as_str)
        .ok_or("the tool call has no file_path")?;
    Ok((real_path(path)?, path.to_string()))
}

/// The path with every symlink resolved, also for the part that does not exist yet. Refuses a relative path, a `..`
/// component, a NUL, and a dangling symlink (where the write would land is unknown).
fn real_path(path: &str) -> Result<PathBuf, String> {
    if path.is_empty() || path.contains('\0') {
        return Err("the path is empty or has a NUL".into());
    }
    let path = Path::new(path);
    if !path.is_absolute() {
        return Err(format!("{} is not an absolute path", path.display()));
    }
    if path.components().any(|c| c == Component::ParentDir) {
        return Err(format!("{} has a .. component", path.display()));
    }
    let mut existing = path.to_path_buf();
    let mut rest: Vec<std::ffi::OsString> = Vec::new();
    loop {
        match std::fs::symlink_metadata(&existing) {
            Ok(_) => {
                let mut real = std::fs::canonicalize(&existing)
                    .map_err(|_| format!("{} cannot be resolved", existing.display()))?;
                real.extend(rest.iter().rev());
                return Ok(real);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let name = existing
                    .file_name()
                    .ok_or("the path has no existing parent")?
                    .to_owned();
                rest.push(name);
                if !existing.pop() {
                    return Err("the path has no existing parent".into());
                }
            }
            Err(_) => return Err(format!("{} cannot be inspected", existing.display())),
        }
    }
}
