//! The `[design]` section: whether a package that depends on a design package waits for the user's approval of the
//! design, where the designer keeps the design record, and how the designer takes guidance.
use std::fs;
use std::path::{Path, PathBuf};

use capstan_wire::js::Value;

use crate::error::{invalid, Result};
use crate::json::{number, object, strings, text};
use crate::keys::DESIGN_KEYS;
use crate::primitives::*;
use crate::text::trim;
use crate::value::Item;

const DEFAULT_RECORD: &str = "DESIGN.md";
const DEFAULT_EXTRA: [&str; 2] = ["docs/design/directions.md", "docs/design/rules.md"];
const MAX_PATH_BYTES: usize = 100;
const MAX_LIST_ENTRIES: usize = 8;
const MAX_COLLECTION_BYTES: usize = 300;

/// What the designer does with the design record.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecordMode {
    Maintain,
    Read,
    Off,
}

impl RecordMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Maintain => "maintain",
            Self::Read => "read",
            Self::Off => "off",
        }
    }
}

/// Whether the designer's guidance reaches its prompt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GuidanceMode {
    Builtin,
    On,
    Off,
}

impl GuidanceMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Builtin => "builtin",
            Self::On => "on",
            Self::Off => "off",
        }
    }
}

/// Whether the guidance rules add to the designer's own rules or replace them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GuidanceRules {
    Extend,
    Override,
}

impl GuidanceRules {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Extend => "extend",
            Self::Override => "override",
        }
    }
}

/// The resolved `[design]` section. `configured` is false when the table is absent; the defaults then apply.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Design {
    pub configured: bool,
    pub require_approval: bool,
    pub design_record: Vec<String>,
    pub design_record_mode: RecordMode,
    pub guidance: GuidanceMode,
    pub guidance_rules: GuidanceRules,
    pub guidance_extra: Vec<String>,
    pub directions_min: u8,
    pub directions_max: u8,
    pub reference_collection: Option<String>,
    /// True when any key added after `require_approval` resolves to a value other than its default.
    pub customised: bool,
}

impl Default for Design {
    fn default() -> Self {
        Self {
            configured: false,
            require_approval: true,
            design_record: vec![DEFAULT_RECORD.to_string()],
            design_record_mode: RecordMode::Maintain,
            guidance: GuidanceMode::Builtin,
            guidance_rules: GuidanceRules::Extend,
            guidance_extra: DEFAULT_EXTRA.iter().map(|path| path.to_string()).collect(),
            directions_min: 2,
            directions_max: 3,
            reference_collection: None,
            customised: false,
        }
    }
}

impl Design {
    /// Any new key except `guidance` is not at its default.
    pub fn block_keys_customised(&self) -> bool {
        let base = Self::default();
        self.record_customised()
            || self.guidance_rules != base.guidance_rules
            || self.guidance_extra != base.guidance_extra
            || self.directions_min != base.directions_min
            || self.directions_max != base.directions_max
            || self.reference_collection.is_some()
    }

    /// The record paths or the record mode are not the defaults.
    pub fn record_customised(&self) -> bool {
        let base = Self::default();
        self.design_record != base.design_record
            || self.design_record_mode != base.design_record_mode
    }

    pub(crate) fn to_value(&self) -> Value {
        let mut members = vec![
            ("configured", Value::Bool(self.configured)),
            ("requireApproval", Value::Bool(self.require_approval)),
        ];
        if self.customised {
            members.extend([
                ("designRecord", strings(&self.design_record)),
                ("designRecordMode", text(self.design_record_mode.as_str())),
                ("guidance", text(self.guidance.as_str())),
                ("guidanceRules", text(self.guidance_rules.as_str())),
                ("guidanceExtra", strings(&self.guidance_extra)),
                ("directionsMin", number(i64::from(self.directions_min))),
                ("directionsMax", number(i64::from(self.directions_max))),
                (
                    "referenceCollection",
                    self.reference_collection
                        .as_deref()
                        .map_or(Value::Null, text),
                ),
            ]);
        }
        object(members)
    }
}

/// Resolves `[design]`; an absent table is the default, which requires approval. `home` is the user's home directory
/// (an empty one counts as unset); `warnings` receives a note for a reference collection that does not exist.
pub fn resolve_design(
    value: Option<&Item>,
    project_root: &Path,
    home: Option<&str>,
    warnings: &mut Vec<String>,
) -> Result<Design> {
    if value.is_none() {
        return Ok(Design::default());
    }
    let table = required_table(value, "design")?;
    reject_unknown_keys(table, DESIGN_KEYS, "design")?;
    let base = Design::default();
    let require_approval = optional_boolean(
        table.get("require_approval"),
        "design.require_approval",
        true,
    )?;
    let root = fs::canonicalize(project_root).ok();

    let design_record = match table.get("design_record") {
        None => base.design_record.clone(),
        Some(item) => path_list(item, "design.design_record", true, root.as_deref())?,
    };
    let design_record_mode = match optional_enum(
        table.get("design_record_mode"),
        "design.design_record_mode",
        &["maintain", "read", "off"],
        "maintain",
    )? {
        "read" => RecordMode::Read,
        "off" => RecordMode::Off,
        _ => RecordMode::Maintain,
    };
    let guidance = match optional_enum(
        table.get("guidance"),
        "design.guidance",
        &["builtin", "on", "off"],
        "builtin",
    )? {
        "on" => GuidanceMode::On,
        "off" => GuidanceMode::Off,
        _ => GuidanceMode::Builtin,
    };
    let guidance_rules = match optional_enum(
        table.get("guidance_rules"),
        "design.guidance_rules",
        &["extend", "override"],
        "extend",
    )? {
        "override" => GuidanceRules::Override,
        _ => GuidanceRules::Extend,
    };
    let guidance_extra = match table.get("guidance_extra") {
        None => base.guidance_extra.clone(),
        Some(item) => path_list(item, "design.guidance_extra", false, root.as_deref())?,
    };
    let directions_min = optional_integer(
        table.get("directions_min"),
        "design.directions_min",
        2,
        5,
        2,
    )?;
    let directions_max = optional_integer(
        table.get("directions_max"),
        "design.directions_max",
        2,
        6,
        3,
    )?;
    if directions_min > directions_max {
        return invalid(format!(
            "design.directions_min ({directions_min}) is greater than design.directions_max ({directions_max}); set both"
        ));
    }
    for (index, extra) in guidance_extra.iter().enumerate() {
        if design_record.contains(extra) {
            return invalid(format!(
                "design.guidance_extra[{index}] repeats a design_record entry"
            ));
        }
    }
    let reference_collection = resolve_collection(
        table.get("reference_collection"),
        project_root,
        home,
        warnings,
    )?;

    let mut design = Design {
        configured: true,
        require_approval,
        design_record,
        design_record_mode,
        guidance,
        guidance_rules,
        guidance_extra,
        directions_min: directions_min as u8,
        directions_max: directions_max as u8,
        reference_collection,
        customised: false,
    };
    design.customised = design.guidance != base.guidance || design.block_keys_customised();
    Ok(design)
}

fn optional_enum<'a>(
    value: Option<&Item>,
    at: &str,
    allowed: &[&'a str],
    fallback: &'a str,
) -> Result<&'a str> {
    match value {
        None => Ok(fallback),
        some => enum_value(some, at, allowed),
    }
}

/// A `design_record` (one string or one to eight) or a `guidance_extra` list, normalised.
fn path_list(item: &Item, at: &str, record: bool, root: Option<&Path>) -> Result<Vec<String>> {
    let entries: Vec<(String, &Item)> = match item {
        Item::Array(entries) => {
            if record && (entries.is_empty() || entries.len() > MAX_LIST_ENTRIES) {
                return invalid(format!("{at} must have 1 to {MAX_LIST_ENTRIES} entries"));
            }
            if entries.len() > MAX_LIST_ENTRIES {
                return invalid(format!("{at} must have at most {MAX_LIST_ENTRIES} entries"));
            }
            entries
                .iter()
                .enumerate()
                .map(|(index, entry)| (format!("{at}[{index}]"), entry))
                .collect()
        }
        Item::Str(_) if record => vec![(at.to_string(), item)],
        _ if record => return invalid(format!("{at} must be a non-empty string")),
        _ => return invalid(format!("{at} must be an array of strings")),
    };
    let mut list: Vec<String> = Vec::new();
    for (entry_at, entry) in entries {
        let Item::Str(raw) = entry else {
            return invalid(format!("{entry_at} must be a non-empty string"));
        };
        let normal = normalise_path(raw, &entry_at)?;
        if list.contains(&normal) {
            return invalid(format!("{entry_at} repeats an earlier entry"));
        }
        if let Some(root) = root {
            if !stays_inside(root, &normal) {
                return invalid(format!("{entry_at} must stay inside the project"));
            }
        }
        list.push(normal);
    }
    Ok(list)
}

/// The lexical checks of one repo-relative path; the result has no `.` segments and no repeated slashes.
fn normalise_path(raw: &str, at: &str) -> Result<String> {
    if raw.is_empty() {
        return invalid(format!("{at} must not be empty"));
    }
    if raw != trim(raw) {
        return invalid(format!("{at} must not have leading or trailing whitespace"));
    }
    assert_safe_text(raw, at, false)?;
    let mut chars = raw.chars();
    let drive =
        matches!((chars.next(), chars.next()), (Some(c), Some(':')) if c.is_ascii_alphabetic());
    if raw.starts_with('/') || raw.starts_with('~') || drive {
        return invalid(format!("{at} must be relative to the project"));
    }
    let normal = raw
        .split('/')
        .filter(|segment| !segment.is_empty() && *segment != ".")
        .collect::<Vec<_>>()
        .join("/");
    if normal.is_empty() {
        return invalid(format!("{at} must not be empty"));
    }
    if normal.split('/').any(|segment| segment == "..") {
        return invalid(format!("{at} must not contain .."));
    }
    if normal.contains('\\') {
        return invalid(format!("{at} must not contain a backslash"));
    }
    if normal.contains(['*', '?', '[', ']']) {
        return invalid(format!("{at} must not contain a glob character (* ? [ ])"));
    }
    if raw.ends_with('/') {
        return invalid(format!("{at} must name a file, not end with /"));
    }
    if normal.len() > MAX_PATH_BYTES {
        return invalid(format!("{at} must be at most {MAX_PATH_BYTES} bytes"));
    }
    Ok(normal)
}

/// On disk, no symlink on the way to `relative` leaves the canonical project root, and neither does the nearest
/// existing ancestor of a path that does not exist yet.
fn stays_inside(root: &Path, relative: &str) -> bool {
    let inside = |path: &Path| path.starts_with(root);
    let mut prefix = root.to_path_buf();
    for segment in relative.split('/') {
        let parent = prefix.clone();
        prefix.push(segment);
        match fs::symlink_metadata(&prefix) {
            Err(_) => return fs::canonicalize(&parent).is_ok_and(|real| inside(&real)),
            Ok(meta) if meta.file_type().is_symlink() => match fs::canonicalize(&prefix) {
                Ok(real) if inside(&real) && real != root => {}
                _ => return false,
            },
            Ok(_) => {}
        }
    }
    true
}

fn resolve_collection(
    value: Option<&Item>,
    project_root: &Path,
    home: Option<&str>,
    warnings: &mut Vec<String>,
) -> Result<Option<String>> {
    let at = "design.reference_collection";
    let Some(item) = value else {
        return Ok(None);
    };
    let Item::Str(raw) = item else {
        return invalid(format!("{at} must be a non-empty string"));
    };
    if raw.is_empty() {
        return Ok(None);
    }
    if raw != trim(raw) {
        return invalid(format!("{at} must not have leading or trailing whitespace"));
    }
    assert_safe_text(raw, at, false)?;
    if raw.len() > MAX_COLLECTION_BYTES {
        return invalid(format!("{at} must be at most {MAX_COLLECTION_BYTES} bytes"));
    }
    let expanded = if let Some(tail) = raw.strip_prefix("~/") {
        match home.filter(|home| !home.is_empty()) {
            Some(home) => format!("{}/{tail}", home.trim_end_matches('/')),
            None => return invalid(format!("{at}: ~ needs HOME")),
        }
    } else if raw.starts_with('/') {
        raw.clone()
    } else {
        return invalid(format!("{at} must be an absolute path or start with ~/"));
    };
    let mut path = PathBuf::from("/");
    for segment in expanded.split('/') {
        match segment {
            "" | "." => {}
            ".." => return invalid(format!("{at} must not contain ..")),
            other => path.push(other),
        }
    }
    let written: PathBuf = project_root
        .components()
        .filter(|part| !matches!(part, std::path::Component::CurDir))
        .collect();
    // The value is compared by name, so the project is compared both as written and resolved: one directory can be named
    // through a symlink (`/tmp/...` and `/private/tmp/...` on macOS).
    let roots = [
        fs::canonicalize(project_root).unwrap_or_else(|_| written.clone()),
        written,
    ];
    if roots.iter().any(|root| path.starts_with(root)) {
        return invalid(format!("{at} must be outside the project"));
    }
    if roots.iter().any(|root| root.starts_with(&path)) {
        return invalid(format!("{at} must not contain the project"));
    }
    if !path.exists() {
        warnings.push(format!("{at} {raw} does not exist"));
    }
    Ok(Some(raw.clone()))
}
