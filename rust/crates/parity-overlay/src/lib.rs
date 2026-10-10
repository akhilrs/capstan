//! Overlays: where the Rust implementation deliberately differs from a case of a Node parity fixture, an overlay file next
//! to the harness replaces that case's whole expected output and says why. The Node fixtures are never edited. The
//! schema and the reader are the same for every replay harness; see README.md.

use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeMap;
use std::fmt;
use std::path::{Component, Path, PathBuf};

/// One overlay file.
#[derive(Clone, Debug, PartialEq)]
pub struct Overlay {
    /// The fixture file, relative to the fixtures directory.
    pub fixture: String,
    /// The case of that fixture, by the name the harness gives it.
    pub case: String,
    /// The whole expected output of the case, in the shape the harness compares.
    pub expected: Value,
    /// Why Rust differs from Node here.
    pub reason: String,
    /// The overlay file this came from.
    pub file: PathBuf,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Raw {
    fixture: String,
    case: String,
    expected: Value,
    reason: String,
}

/// Why overlays could not be loaded; the text names the overlay file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OverlayError(String);

impl fmt::Display for OverlayError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for OverlayError {}

fn error(file: &Path, message: impl fmt::Display) -> OverlayError {
    OverlayError(format!("overlay {}: {message}", file.display()))
}

/// The overlays of one harness, by fixture and case.
#[derive(Clone, Debug, Default)]
pub struct Overlays {
    by_case: BTreeMap<(String, String), Overlay>,
}

impl Overlays {
    /// The expected output that replaces the case of the fixture, if an overlay names it.
    pub fn expected(&self, fixture: &str, case: &str) -> Option<&Value> {
        self.get(fixture, case).map(|overlay| &overlay.expected)
    }

    pub fn get(&self, fixture: &str, case: &str) -> Option<&Overlay> {
        self.by_case.get(&(fixture.to_string(), case.to_string()))
    }

    pub fn iter(&self) -> impl Iterator<Item = &Overlay> {
        self.by_case.values()
    }

    pub fn len(&self) -> usize {
        self.by_case.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_case.is_empty()
    }
}

/// Reads every `*.json` file of `directory` (none if the directory does not exist) and checks it against the fixtures:
/// `fixtures` is the directory the overlays' `fixture` paths are relative to, and `cases_of` lists the case names of a
/// fixture file (an error if it cannot be read). An overlay is refused when its fixture or case does not exist, when it has
/// no reason, or when another overlay already replaces the same case.
pub fn load(
    directory: &Path,
    fixtures: &Path,
    cases_of: &dyn Fn(&Path) -> Result<Vec<String>, String>,
) -> Result<Overlays, OverlayError> {
    let mut overlays = Overlays::default();
    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(overlays),
        Err(e) => return Err(error(directory, e)),
    };
    let mut files: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|e| e == "json"))
        .collect();
    files.sort();
    for file in files {
        let text = std::fs::read_to_string(&file).map_err(|e| error(&file, e))?;
        let raw: Raw = serde_json::from_str(&text).map_err(|e| error(&file, e))?;
        if raw.reason.trim().is_empty() {
            return Err(error(
                &file,
                "has no reason: say why Rust differs from Node",
            ));
        }
        let relative = Path::new(&raw.fixture);
        if raw.fixture.is_empty()
            || !relative
                .components()
                .all(|part| matches!(part, Component::Normal(_)))
        {
            return Err(error(
                &file,
                format!(
                    "fixture {:?} must be a path inside the fixtures directory",
                    raw.fixture
                ),
            ));
        }
        let fixture_file = fixtures.join(relative);
        if !fixture_file.is_file() {
            return Err(error(
                &file,
                format!("names the fixture {} which does not exist", raw.fixture),
            ));
        }
        let cases = cases_of(&fixture_file).map_err(|e| {
            error(
                &file,
                format!("fixture {} cannot be read: {e}", raw.fixture),
            )
        })?;
        if !cases.contains(&raw.case) {
            return Err(error(
                &file,
                format!("fixture {} has no case {:?}", raw.fixture, raw.case),
            ));
        }
        let key = (raw.fixture.clone(), raw.case.clone());
        if let Some(first) = overlays.by_case.get(&key) {
            return Err(error(
                &file,
                format!(
                    "replaces {} / {} which {} already replaces",
                    raw.fixture,
                    raw.case,
                    first.file.display()
                ),
            ));
        }
        overlays.by_case.insert(
            key,
            Overlay {
                fixture: raw.fixture,
                case: raw.case,
                expected: raw.expected,
                reason: raw.reason,
                file,
            },
        );
    }
    Ok(overlays)
}
