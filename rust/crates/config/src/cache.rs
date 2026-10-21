//! The `[cache]` section: shared tool caches (compiler, package manager) that agents reuse across worktrees. This module
//! holds the shapes, the built-in preset registry and the marker table as data, and resolves and validates the section.
//! Nothing here touches the filesystem: `detect` is stored as a flag and the project scan runs in the cache store.
use std::path::{Path, PathBuf};

use capstan_wire::js::Value;

use crate::error::{invalid, Result};
use crate::json::{number, object, optional, strings, text};
use crate::keys::{
    CACHE_ENTRY_KEYS, CACHE_KEYS, CACHE_PRESET_KEYS, CACHE_RUST_KEYS, CACHE_SETUP_REUSE_KEYS,
};
use crate::primitives::*;
use crate::types::{RESERVED_ENV_NAMES, RESERVED_ENV_PREFIXES};
use crate::value::{Item, Table};

pub const DEFAULT_CACHE_MAX_SIZE_GB: u64 = 20;
pub const DEFAULT_CACHE_MAX_AGE_DAYS: i64 = 30;
pub const MIN_CACHE_SIZE_GB: f64 = 0.1;
pub const MAX_CACHE_SIZE_GB: f64 = 10_000.0;
pub const MAX_CACHE_AGE_DAYS: i64 = 3650;
const GIB: f64 = 1_073_741_824.0;
const MAX_CACHE_ENTRIES: usize = 16;
const MAX_ENTRY_ITEMS: usize = 8;
const MAX_COMMANDS: usize = 8;
const MAX_REUSE_PATHS: usize = 16;
const MAX_VALUE_CHARS: usize = 256;

/// Preset names, in the order the documentation lists them.
pub const CACHE_PRESETS: [&str; 4] = ["rust", "python", "node", "go"];
/// `[cache.rust] mode`; the first is the default.
pub const RUST_MODES: [&str; 3] = ["sccache", "shared-build-dir", "off"];

/// What a project marker file means for `detect`: the preset and the file names (a `*` matches any characters).
/// The scan reads the project root and one directory below it, skipping `DETECT_SKIP_DIRS`.
pub const DETECT_MARKERS: [(&str, &[&str]); 4] = [
    ("rust", &["Cargo.toml"]),
    (
        "python",
        &[
            "pyproject.toml",
            "requirements*.txt",
            "setup.py",
            "uv.lock",
            "poetry.lock",
        ],
    ),
    ("node", &["package.json"]),
    ("go", &["go.mod"]),
];
pub const DETECT_SKIP_DIRS: [&str; 3] = [".git", "node_modules", "target"];
/// How many directories below the project root the scan looks into.
pub const DETECT_MAX_DEPTH: usize = 1;

/// The environment names whose use for a cache would change how an agent's shell works or would be a credential.
const SECRET_NAME_PARTS: [&str; 6] = ["TOKEN", "SECRET", "KEY", "PASSWORD", "PASSWD", "CREDENTIAL"];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CacheScope {
    #[default]
    Project,
    User,
}

impl CacheScope {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Project => "project",
            Self::User => "user",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CacheOrigin {
    Preset,
    Custom,
    /// A preset that is not listed in `presets` and applies only when `detect` finds its marker files.
    Detected,
}

impl CacheOrigin {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Preset => "preset",
            Self::Custom => "custom",
            Self::Detected => "detected",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct CacheEntry {
    pub name: String,
    pub origin: CacheOrigin,
    /// `(env name, subdirectory of the entry's cache directory)`.
    pub dirs: Vec<(String, String)>,
    /// `(env name, fixed value)`.
    pub set: Vec<(String, String)>,
    /// A tool that must be on the agent's PATH for the entry to apply.
    pub requires: Option<String>,
    /// Commands run with the entry's environment when the cache is pruned.
    pub prune: Vec<String>,
    /// Commands run before the entry's directories are cleared.
    pub stop: Vec<String>,
    /// `rust` only: `sccache`, `shared-build-dir` or `off`.
    pub mode: Option<String>,
    pub max_size_bytes: Option<u64>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SetupReuse {
    pub key_files: Vec<String>,
    pub paths: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CacheConfig {
    /// The key `root`: absolute or starting with `~/`.
    pub base: Option<PathBuf>,
    pub scope: CacheScope,
    pub max_size_bytes: u64,
    pub max_age_days: u32,
    pub detect: bool,
    pub entries: Vec<CacheEntry>,
    /// With `detect`: every preset not in `presets`, resolved with its `[cache.<preset>]` options; empty otherwise.
    pub detectable: Vec<CacheEntry>,
    pub setup_reuse: Option<SetupReuse>,
}

impl CacheConfig {
    pub(crate) fn to_value(&self) -> Value {
        let entry_values =
            |list: &[CacheEntry]| Value::Array(list.iter().map(entry_value).collect());
        let reuse = self.setup_reuse.as_ref().map_or(Value::Null, |reuse| {
            object(vec![
                ("keyFiles", strings(&reuse.key_files)),
                ("paths", strings(&reuse.paths)),
            ])
        });
        object(vec![
            (
                "root",
                optional(&self.base.as_ref().map(|b| b.to_string_lossy().into_owned())),
            ),
            ("scope", text(self.scope.as_str())),
            ("maxSizeBytes", number(self.max_size_bytes as i64)),
            ("maxAgeDays", number(i64::from(self.max_age_days))),
            ("detect", Value::Bool(self.detect)),
            ("entries", entry_values(&self.entries)),
            ("detectable", entry_values(&self.detectable)),
            ("setupReuse", reuse),
        ])
    }
}

fn pairs_value(list: &[(String, String)], second: &'static str) -> Value {
    Value::Array(
        list.iter()
            .map(|(env, value)| object(vec![("env", text(env)), (second, text(value))]))
            .collect(),
    )
}

fn entry_value(entry: &CacheEntry) -> Value {
    let size = entry
        .max_size_bytes
        .map_or(Value::Null, |b| number(b as i64));
    object(vec![
        ("name", text(&entry.name)),
        ("origin", text(entry.origin.as_str())),
        ("dirs", pairs_value(&entry.dirs, "subdir")),
        ("set", pairs_value(&entry.set, "value")),
        ("requires", optional(&entry.requires)),
        ("prune", strings(&entry.prune)),
        ("stop", strings(&entry.stop)),
        ("mode", optional(&entry.mode)),
        ("maxSizeBytes", size),
    ])
}

fn pair(env: &str, value: &str) -> (String, String) {
    (env.to_string(), value.to_string())
}

/// The size sccache is given: whole GiB as `<n>G`, otherwise whole MiB as `<n>M`.
fn sccache_size(bytes: u64) -> String {
    let gib = GIB as u64;
    if bytes % gib == 0 {
        format!("{}G", bytes / gib)
    } else {
        format!("{}M", (bytes / (1024 * 1024)).max(1))
    }
}

/// The built-in entry for `name`. `size` is the cap the entry runs under (its own, or the total).
fn preset_entry(
    name: &str,
    mode: Option<&str>,
    incremental: bool,
    size: u64,
    own_size: Option<u64>,
) -> CacheEntry {
    let mut entry = CacheEntry {
        name: name.to_string(),
        origin: CacheOrigin::Preset,
        dirs: Vec::new(),
        set: Vec::new(),
        requires: None,
        prune: Vec::new(),
        stop: Vec::new(),
        mode: None,
        max_size_bytes: own_size,
    };
    match name {
        "rust" => {
            let mode = mode.unwrap_or(RUST_MODES[0]);
            entry.mode = Some(mode.to_string());
            match mode {
                "sccache" => {
                    entry.dirs = vec![pair("SCCACHE_DIR", "sccache")];
                    entry.set = vec![
                        pair("RUSTC_WRAPPER", "sccache"),
                        pair("SCCACHE_CACHE_SIZE", &sccache_size(size)),
                    ];
                    // sccache cannot cache incremental crates, so incremental compilation only costs disk.
                    if !incremental {
                        entry.set.push(pair("CARGO_INCREMENTAL", "0"));
                    }
                    entry.requires = Some("sccache".into());
                    entry.stop = vec!["sccache --stop-server".into()];
                }
                "shared-build-dir" => {
                    entry.dirs = vec![pair("CARGO_BUILD_BUILD_DIR", "cargo-build")];
                    entry.requires = Some("cargo".into());
                }
                _ => {}
            }
        }
        "python" => {
            entry.dirs = vec![
                pair("PIP_CACHE_DIR", "pip"),
                pair("UV_CACHE_DIR", "uv"),
                pair("POETRY_CACHE_DIR", "poetry"),
            ];
            entry.prune = vec!["uv cache prune".into()];
        }
        "node" => {
            entry.dirs = vec![
                pair("NPM_CONFIG_CACHE", "npm"),
                pair("NPM_CONFIG_STORE_DIR", "pnpm-store"),
                pair("YARN_CACHE_FOLDER", "yarn"),
            ];
            entry.prune = vec!["npm cache verify".into(), "pnpm store prune".into()];
        }
        _ => {
            entry.dirs = vec![pair("GOCACHE", "go-build"), pair("GOMODCACHE", "go-mod")];
        }
    }
    entry
}

fn secret_shaped(name: &str) -> bool {
    SECRET_NAME_PARTS.iter().any(|part| name.contains(part))
}

fn env_name_problem(name: &str, env_pass: &[String]) -> Option<String> {
    if !env_name_ok(name) {
        return Some("must be an upper-case variable name (letters, digits and underscore, at most 64 characters)".into());
    }
    if name.starts_with("CAPSTAN_") {
        return Some("must not start with CAPSTAN_: those variables belong to Capstan".into());
    }
    if RESERVED_ENV_NAMES.contains(&name)
        || RESERVED_ENV_PREFIXES
            .iter()
            .any(|prefix| name.starts_with(prefix))
    {
        return Some(format!(
            "must not be {name}: Capstan already sets it or it changes how an agent's shell, loader or git behaves"
        ));
    }
    if env_pass.iter().any(|passed| passed == name) {
        return Some(format!(
            "must not be {name}: it is already passed by [env] pass"
        ));
    }
    if secret_shaped(name) {
        return Some(format!(
            "must not be {name}: a name with TOKEN, SECRET, KEY, PASSWORD, PASSWD or CREDENTIAL looks like a secret and caches never carry secrets; pass such a variable through [env] pass"
        ));
    }
    None
}

fn subdir_ok(subdir: &str) -> bool {
    !subdir.is_empty()
        && subdir != "."
        && subdir.len() <= 64
        && !subdir.contains("..")
        && subdir
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

fn relative_path_problem(path: &str) -> Option<&'static str> {
    if path.starts_with('/') || path.starts_with('~') || path.contains('\\') {
        return Some("must be a relative path");
    }
    if path
        .split('/')
        .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Some("must not be absolute or contain .., . or empty segments");
    }
    None
}

fn tool_name_ok(tool: &str) -> bool {
    !tool.is_empty()
        && tool.len() <= 64
        && tool
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'+' | b'-'))
}

/// `max_size_gb`: an integer or a float from 0.1 to 10000, in bytes.
fn size_bytes(value: Option<&Item>, at: &str) -> Result<Option<u64>> {
    let gb = match value {
        None => return Ok(None),
        Some(Item::Int(n)) => *n as f64,
        Some(Item::Float(f)) => *f,
        Some(Item::BigInt(_)) => f64::INFINITY,
        Some(_) => return invalid(format!("{at} must be a number")),
    };
    if !(MIN_CACHE_SIZE_GB..=MAX_CACHE_SIZE_GB).contains(&gb) {
        return invalid(format!(
            "{at} must be between {MIN_CACHE_SIZE_GB} and {MAX_CACHE_SIZE_GB}"
        ));
    }
    Ok(Some((gb * GIB).round() as u64))
}

/// A table of `ENV = "text"` pairs, every name checked and recorded in `seen` (one name per cache).
fn env_pairs(
    value: Option<&Item>,
    at: &str,
    entry: &str,
    env_pass: &[String],
    seen: &mut Vec<(String, String)>,
    is_dir: bool,
) -> Result<Vec<(String, String)>> {
    let table = optional_table(value, at)?;
    if table.len() > MAX_ENTRY_ITEMS {
        return invalid(format!("{at} exceeds {MAX_ENTRY_ITEMS} entries"));
    }
    let mut pairs = Vec::new();
    for (name, item) in table.entries() {
        let key = format!("{at}.{name}");
        if let Some(problem) = env_name_problem(name, env_pass) {
            return invalid(format!("{key} {problem}"));
        }
        if let Some((_, other)) = seen.iter().find(|(seen_name, _)| seen_name == name) {
            return invalid(format!("{key} is also set by cache entry {other}"));
        }
        seen.push((name.to_string(), entry.to_string()));
        let value = required_string(Some(item), &key, MAX_VALUE_CHARS, false)?;
        if is_dir {
            if !subdir_ok(value) {
                return invalid(format!(
                    "{key} must be a single directory name of letters, digits, dot, underscore and dash, not absolute and without .."
                ));
            }
        } else {
            guard_credential_shape(value, &key)?;
        }
        pairs.push(pair(name, value));
    }
    Ok(pairs)
}

fn commands(value: Option<&Item>, at: &str) -> Result<Vec<String>> {
    string_list(value, at, MAX_COMMANDS)
}

fn resolve_root(value: Option<&Item>, project_root: &Path) -> Result<Option<PathBuf>> {
    let Some(root) = optional_string(value, "cache.root", 1024)? else {
        return Ok(None);
    };
    guard_credential_shape(root, "cache.root")?;
    let absolute = root.starts_with('/');
    let home_relative = root.starts_with("~/");
    if !absolute && !home_relative {
        return invalid("cache.root must be an absolute path or start with ~/");
    }
    let tail = if home_relative { &root[2..] } else { root };
    if tail.split('/').any(|part| part == "..") {
        return invalid("cache.root must not contain ..");
    }
    if !home_relative && root.trim_matches('/').is_empty() {
        return invalid("cache.root must not be the filesystem root");
    }
    let expanded = if home_relative {
        std::env::var_os("HOME").map(|home| PathBuf::from(home).join(tail))
    } else {
        Some(PathBuf::from(root))
    };
    if let Some(expanded) = expanded {
        if expanded.starts_with(project_root) {
            return invalid("cache.root must not be inside the project root");
        }
    }
    Ok(Some(PathBuf::from(root.trim_end_matches('/'))))
}

fn resolve_setup_reuse(value: Option<&Item>) -> Result<Option<SetupReuse>> {
    if value.is_none() {
        return Ok(None);
    }
    let table = optional_table(value, "cache.setup_reuse")?;
    reject_unknown_keys(table, CACHE_SETUP_REUSE_KEYS, "cache.setup_reuse")?;
    let mut lists = Vec::new();
    for key in ["key_files", "paths"] {
        let at = format!("cache.setup_reuse.{key}");
        let list = string_list(table.get(key), &at, MAX_REUSE_PATHS)?;
        if list.is_empty() {
            return invalid(format!("{at} must list at least one path"));
        }
        for (index, path) in list.iter().enumerate() {
            if let Some(problem) = relative_path_problem(path) {
                return invalid(format!("{at}[{index}] {problem}"));
            }
        }
        lists.push(list);
    }
    let paths = lists.pop().unwrap_or_default();
    let key_files = lists.pop().unwrap_or_default();
    Ok(Some(SetupReuse { key_files, paths }))
}

/// Resolves `[cache]`; an absent table is `None`, which changes nothing for the project.
pub fn resolve_cache(
    value: Option<&Item>,
    project_root: &Path,
    env_pass: &[String],
) -> Result<Option<CacheConfig>> {
    if value.is_none() {
        return Ok(None);
    }
    let table = required_table(value, "cache")?;
    reject_unknown_keys(table, CACHE_KEYS, "cache")?;
    let preset_names = string_list(table.get("presets"), "cache.presets", CACHE_PRESETS.len())?;
    for (index, name) in preset_names.iter().enumerate() {
        if !CACHE_PRESETS.contains(&name.as_str()) {
            return invalid(format!(
                "cache.presets[{index}] must be one of {}",
                CACHE_PRESETS.join(", ")
            ));
        }
        if preset_names[..index].contains(name) {
            return invalid(format!("cache.presets[{index}] repeats {name}"));
        }
    }
    let detect = optional_boolean(table.get("detect"), "cache.detect", false)?;
    let base = resolve_root(table.get("root"), project_root)?;
    let scope = match table.get("scope") {
        None => CacheScope::Project,
        Some(_) => match enum_value(table.get("scope"), "cache.scope", &["project", "user"])? {
            "user" => CacheScope::User,
            _ => CacheScope::Project,
        },
    };
    let max_size_bytes = size_bytes(table.get("max_size_gb"), "cache.max_size_gb")?
        .unwrap_or(DEFAULT_CACHE_MAX_SIZE_GB * GIB as u64);
    let max_age_days = optional_integer(
        table.get("max_age_days"),
        "cache.max_age_days",
        1,
        MAX_CACHE_AGE_DAYS,
        DEFAULT_CACHE_MAX_AGE_DAYS,
    )? as u32;

    let mut seen_env: Vec<(String, String)> = Vec::new();
    let mut entries = Vec::new();
    let mut sized: Vec<(String, u64)> = Vec::new();

    for name in &preset_names {
        entries.push(resolve_preset(
            name,
            table,
            max_size_bytes,
            &mut sized,
            &mut seen_env,
        )?);
    }
    // A preset table is only meaningful for a preset that is on, or may be switched on by `detect`.
    let mut detectable = Vec::new();
    for preset in CACHE_PRESETS {
        if !detect && table.get(preset).is_some() && !preset_names.iter().any(|name| name == preset)
        {
            return invalid(format!(
                "cache.{preset} is set but {preset} is not in cache.presets"
            ));
        }
    }
    if detect {
        for preset in CACHE_PRESETS {
            if preset_names.iter().any(|name| name == preset) {
                continue;
            }
            // Resolved now so a mistake is not found only when the project is scanned; its env names are checked
            // against the other entries when it is switched on, because it applies only if its markers are found.
            let mut entry =
                resolve_preset(preset, table, max_size_bytes, &mut sized, &mut Vec::new())?;
            entry.origin = CacheOrigin::Detected;
            detectable.push(entry);
        }
    }

    let entry_table = optional_table(table.get("entries"), "cache.entries")?;
    if entry_table.len() + entries.len() > MAX_CACHE_ENTRIES {
        return invalid(format!("cache has more than {MAX_CACHE_ENTRIES} entries"));
    }
    for (name, item) in entry_table.entries() {
        let at = format!("cache.entries.{name}");
        if CACHE_PRESETS.contains(&name) {
            return invalid(format!(
                "{at} is named like the built-in preset {name}; choose another name, or set the preset under cache.{name}"
            ));
        }
        if !name_ok(name) {
            return invalid(format!(
                "{at} name must be lower-case letters, digits, underscore or dash, starting with a letter"
            ));
        }
        let fields = required_table(Some(item), &at)?;
        reject_unknown_keys(fields, CACHE_ENTRY_KEYS, &at)?;
        let dirs = env_pairs(
            fields.get("dirs"),
            &format!("{at}.dirs"),
            name,
            env_pass,
            &mut seen_env,
            true,
        )?;
        let set = env_pairs(
            fields.get("set"),
            &format!("{at}.set"),
            name,
            env_pass,
            &mut seen_env,
            false,
        )?;
        if dirs.is_empty() && set.is_empty() {
            return invalid(format!("{at} needs dirs or set"));
        }
        let requires = match fields.get("requires") {
            None => None,
            Some(_) => {
                let tool =
                    required_string(fields.get("requires"), &format!("{at}.requires"), 64, false)?;
                if !tool_name_ok(tool) {
                    return invalid(format!(
                        "{at}.requires must be a program name without a path"
                    ));
                }
                Some(tool.to_string())
            }
        };
        let own_size = size_bytes(fields.get("max_size_gb"), &format!("{at}.max_size_gb"))?;
        if let Some(bytes) = own_size {
            sized.push((at.clone(), bytes));
        }
        entries.push(CacheEntry {
            name: name.to_string(),
            origin: CacheOrigin::Custom,
            dirs,
            set,
            requires,
            prune: commands(fields.get("prune"), &format!("{at}.prune"))?,
            stop: commands(fields.get("stop"), &format!("{at}.stop"))?,
            mode: None,
            max_size_bytes: own_size,
        });
    }

    for (at, bytes) in &sized {
        if *bytes > max_size_bytes {
            return invalid(format!("{at}.max_size_gb is larger than cache.max_size_gb"));
        }
    }
    if sized.iter().map(|(_, bytes)| *bytes).sum::<u64>() > max_size_bytes {
        return invalid(
            "the max_size_gb of the cache entries add up to more than cache.max_size_gb",
        );
    }

    let setup_reuse = resolve_setup_reuse(table.get("setup_reuse"))?;
    Ok(Some(CacheConfig {
        base,
        scope,
        max_size_bytes,
        max_age_days,
        detect,
        entries,
        detectable,
        setup_reuse,
    }))
}

fn resolve_preset(
    name: &str,
    table: &Table,
    total: u64,
    sized: &mut Vec<(String, u64)>,
    seen_env: &mut Vec<(String, String)>,
) -> Result<CacheEntry> {
    let at = format!("cache.{name}");
    let fields = optional_table(table.get(name), &at)?;
    let allowed: &[&str] = if name == "rust" {
        CACHE_RUST_KEYS
    } else {
        CACHE_PRESET_KEYS
    };
    reject_unknown_keys(fields, allowed, &at)?;
    let mode = if name == "rust" && fields.get("mode").is_some() {
        Some(enum_value(
            fields.get("mode"),
            "cache.rust.mode",
            &RUST_MODES,
        )?)
    } else {
        None
    };
    let incremental = optional_boolean(fields.get("incremental"), "cache.rust.incremental", false)?;
    let own = size_bytes(fields.get("max_size_gb"), &format!("{at}.max_size_gb"))?;
    if let Some(bytes) = own {
        sized.push((at, bytes));
    }
    let entry = preset_entry(name, mode, incremental, own.unwrap_or(total), own);
    for (env, _) in entry.dirs.iter().chain(entry.set.iter()) {
        seen_env.push((env.clone(), entry.name.clone()));
    }
    Ok(entry)
}
