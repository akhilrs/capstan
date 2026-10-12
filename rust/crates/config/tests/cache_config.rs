//! The `[cache]` section: defaults, presets, modes and every refusal. Rust-only: no corpus case contains `[cache]`.
use capstan_config::{parse_config, CacheOrigin, CacheScope, ConfigError, RoleConfig};
use std::path::Path;

const BASE: &str = r#"schema_version = 1
[nexora]
track = "never"
[hosts.claude]
kind = "claude"
[roles.pm]
kind = "PM"
host = "claude"
"#;

fn load_with(prefix: &str, extra: &str) -> Result<RoleConfig, String> {
    let text = format!("{prefix}{BASE}{extra}");
    parse_config(text.as_bytes(), Path::new("/nonexistent/project")).map_err(|error| match error {
        ConfigError::Invalid(message) => message,
        other => format!("{other:?}"),
    })
}

fn load(extra: &str) -> Result<RoleConfig, String> {
    load_with("", extra)
}

fn refusal(extra: &str) -> String {
    load(extra).expect_err("the section is refused")
}

fn cache(extra: &str) -> capstan_config::CacheConfig {
    load(extra).expect("loads").cache.expect("cache present")
}

const GIB: u64 = 1_073_741_824;

#[test]
fn an_absent_section_is_none_and_the_json_has_no_cache() {
    let config = load("").expect("loads");
    assert!(config.cache.is_none());
    assert!(!config.to_json().contains("\"cache\""));
}

#[test]
fn an_empty_section_takes_the_defaults() {
    let cache = cache("[cache]\n");
    assert_eq!(cache.max_size_bytes, 20 * GIB);
    assert_eq!(cache.max_age_days, 30);
    assert_eq!(cache.scope, CacheScope::Project);
    assert!(!cache.detect);
    assert!(cache.base.is_none() && cache.entries.is_empty() && cache.setup_reuse.is_none());
}

#[test]
fn the_four_presets_resolve_to_the_documented_pairs() {
    let cache = cache("[cache]\npresets = [\"rust\", \"python\", \"node\", \"go\"]\n");
    let names: Vec<_> = cache.entries.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, ["rust", "python", "node", "go"]);
    assert!(cache
        .entries
        .iter()
        .all(|e| e.origin == CacheOrigin::Preset));
    let rust = &cache.entries[0];
    assert_eq!(rust.mode.as_deref(), Some("sccache"));
    assert_eq!(rust.dirs, [("SCCACHE_DIR".into(), "sccache".into())]);
    assert_eq!(
        rust.set,
        [
            ("RUSTC_WRAPPER".into(), "sccache".into()),
            ("SCCACHE_CACHE_SIZE".into(), "20G".into())
        ]
    );
    assert_eq!(rust.requires.as_deref(), Some("sccache"));
    assert_eq!(rust.stop, ["sccache --stop-server"]);
    let python = &cache.entries[1];
    let dirs: Vec<_> = python
        .dirs
        .iter()
        .map(|(e, d)| (e.as_str(), d.as_str()))
        .collect();
    assert_eq!(
        dirs,
        [
            ("PIP_CACHE_DIR", "pip"),
            ("UV_CACHE_DIR", "uv"),
            ("POETRY_CACHE_DIR", "poetry")
        ]
    );
    assert_eq!(python.prune, ["uv cache prune"]);
    let node = &cache.entries[2];
    let dirs: Vec<_> = node
        .dirs
        .iter()
        .map(|(e, d)| (e.as_str(), d.as_str()))
        .collect();
    assert_eq!(
        dirs,
        [
            ("NPM_CONFIG_CACHE", "npm"),
            ("NPM_CONFIG_STORE_DIR", "pnpm-store"),
            ("YARN_CACHE_FOLDER", "yarn")
        ]
    );
    assert_eq!(node.prune, ["npm cache verify", "pnpm store prune"]);
    let go = &cache.entries[3];
    let dirs: Vec<_> = go
        .dirs
        .iter()
        .map(|(e, d)| (e.as_str(), d.as_str()))
        .collect();
    assert_eq!(dirs, [("GOCACHE", "go-build"), ("GOMODCACHE", "go-mod")]);
}

#[test]
fn rust_modes_resolve_and_anything_else_is_refused() {
    let shared =
        cache("[cache]\npresets = [\"rust\"]\n[cache.rust]\nmode = \"shared-build-dir\"\n");
    assert_eq!(
        shared.entries[0].dirs,
        [("CARGO_BUILD_BUILD_DIR".into(), "cargo-build".into())]
    );
    assert_eq!(shared.entries[0].requires.as_deref(), Some("cargo"));
    let off = cache("[cache]\npresets = [\"rust\"]\n[cache.rust]\nmode = \"off\"\n");
    assert!(off.entries[0].dirs.is_empty() && off.entries[0].set.is_empty());
    assert_eq!(off.entries[0].mode.as_deref(), Some("off"));
    let message = refusal("[cache]\npresets = [\"rust\"]\n[cache.rust]\nmode = \"ccache\"\n");
    assert!(
        message.contains("cache.rust.mode must be one of sccache, shared-build-dir, off"),
        "{message}"
    );
}

#[test]
fn detect_is_a_flag_and_scans_nothing() {
    let cache = cache("[cache]\ndetect = true\n");
    assert!(cache.detect);
    assert!(cache.entries.is_empty());
    let both = self::cache("[cache]\ndetect = true\npresets = [\"go\"]\n");
    assert_eq!(both.entries.len(), 1);
    assert!(capstan_config::DETECT_MARKERS
        .iter()
        .any(|(p, f)| *p == "go" && f.contains(&"go.mod")));
}

#[test]
fn sizes_are_bytes_and_per_entry_caps_are_bounded_by_the_total() {
    let cache = cache("[cache]\npresets = [\"python\"]\nmax_size_gb = 0.5\nmax_age_days = 7\n[cache.python]\nmax_size_gb = 0.25\n");
    assert_eq!(cache.max_size_bytes, GIB / 2);
    assert_eq!(cache.max_age_days, 7);
    let sccache = self::cache(
        "[cache]\npresets = [\"rust\"]\nmax_size_gb = 10\n[cache.rust]\nmax_size_gb = 4\n",
    );
    assert_eq!(sccache.entries[0].max_size_bytes, Some(4 * GIB));
    assert!(sccache.entries[0]
        .set
        .contains(&("SCCACHE_CACHE_SIZE".into(), "4G".into())));
    let message = refusal(
        "[cache]\nmax_size_gb = 5\n[cache.entries.x]\ndirs = { X_DIR = \"x\" }\nmax_size_gb = 6\n",
    );
    assert!(
        message.contains("cache.entries.x.max_size_gb is larger than cache.max_size_gb"),
        "{message}"
    );
    let message = refusal(
        "[cache]\nmax_size_gb = 5\n[cache.entries.x]\ndirs = { X_DIR = \"x\" }\nmax_size_gb = 3\n[cache.entries.y]\ndirs = { Y_DIR = \"y\" }\nmax_size_gb = 3\n",
    );
    assert!(
        message.contains("add up to more than cache.max_size_gb"),
        "{message}"
    );
}

#[test]
fn range_errors_name_the_key() {
    for (body, key) in [
        ("max_size_gb = 0.05", "cache.max_size_gb"),
        ("max_size_gb = 10001", "cache.max_size_gb"),
        ("max_size_gb = \"20\"", "cache.max_size_gb"),
        ("max_age_days = 0", "cache.max_age_days"),
        ("max_age_days = 3651", "cache.max_age_days"),
    ] {
        let message = refusal(&format!("[cache]\n{body}\n"));
        assert!(message.contains(key), "{body}: {message}");
    }
}

fn custom(entry: &str) -> String {
    format!("[cache]\n[cache.entries.mine]\n{entry}\n")
}

#[test]
fn a_custom_entry_resolves() {
    let cache = cache(&custom(
        "dirs = { BUNDLE_PATH = \"bundle\" }\nset = { BUNDLE_JOBS = \"4\" }\nrequires = \"bundle\"\nprune = [\"bundle clean\"]\nstop = [\"spring stop\"]",
    ));
    let entry = &cache.entries[0];
    assert_eq!(entry.origin, CacheOrigin::Custom);
    assert_eq!(entry.dirs, [("BUNDLE_PATH".into(), "bundle".into())]);
    assert_eq!(entry.set, [("BUNDLE_JOBS".into(), "4".into())]);
    assert_eq!(entry.requires.as_deref(), Some("bundle"));
    assert_eq!(entry.prune, ["bundle clean"]);
    assert_eq!(entry.stop, ["spring stop"]);
}

#[test]
fn invalid_entries_are_refused_with_the_key() {
    let cases: Vec<(String, &str)> = vec![
        ("[cache]\npresets = [\"java\"]\n".into(), "cache.presets[0]"),
        ("[cache]\npresets = [\"go\", \"go\"]\n".into(), "cache.presets[1] repeats go"),
        ("[cache.entries.go]\ndirs = { X = \"x\" }\n".replace("[cache.entries", "[cache]\n[cache.entries"), "cache.entries.go"),
        ("[cache]\nbogus = 1\n".into(), "cache has 1 unknown key"),
        (custom("bogus = 1"), "cache.entries.mine has 1 unknown key"),
        (custom("dirs = { lower = \"x\" }"), "cache.entries.mine.dirs.lower"),
        (custom("dirs = { CAPSTAN_X = \"x\" }"), "CAPSTAN_"),
        (custom("dirs = { PATH = \"x\" }"), "cache.entries.mine.dirs.PATH"),
        (custom("dirs = { HOME = \"x\" }"), "cache.entries.mine.dirs.HOME"),
        (custom("dirs = { TMPDIR = \"x\" }"), "cache.entries.mine.dirs.TMPDIR"),
        (custom("dirs = { LD_PRELOAD = \"x\" }"), "cache.entries.mine.dirs.LD_PRELOAD"),
        (custom("dirs = { API_TOKEN = \"x\" }"), "looks like a secret"),
        (custom("set = { DB_PASSWORD = \"x\" }"), "looks like a secret"),
        (custom("set = { MY_KEY = \"x\" }"), "cache.entries.mine.set.MY_KEY"),
        (custom("set = { A_CREDENTIAL = \"x\" }"), "looks like a secret"),
        (custom("set = { A_PASSWD = \"x\" }"), "looks like a secret"),
        (custom("set = { A_SECRET = \"x\" }"), "looks like a secret"),
        (custom("dirs = { A = \"/abs\" }"), "cache.entries.mine.dirs.A"),
        (custom("dirs = { A = \"../up\" }"), "cache.entries.mine.dirs.A"),
        (custom("dirs = { A = \"a/b\" }"), "cache.entries.mine.dirs.A"),
        (custom("dirs = { A = \"a b\" }"), "cache.entries.mine.dirs.A"),
        (custom("set = { A = \"one\\ntwo\" }"), "cache.entries.mine.set.A"),
        (custom("set = { A = \"sk-abcdefghijklmnop\" }"), "cache.entries.mine.set.A"),
        (custom("requires = \"/bin/tool\"\ndirs = { A = \"a\" }"), "cache.entries.mine.requires"),
        (custom("requires = \"tool\""), "needs dirs or set"),
        (custom("dirs = { A = \"a\" }\nprune = [\"-x\"]"), "cache.entries.mine.prune[0]"),
        (
            "[cache]\npresets = [\"python\"]\n[cache.entries.mine]\ndirs = { PIP_CACHE_DIR = \"p\" }\n".into(),
            "also set by cache entry python",
        ),
        (
            "[cache.entries.a]\ndirs = { SAME = \"a\" }\n[cache.entries.b]\nset = { SAME = \"b\" }\n".replace("[cache.entries.a]", "[cache]\n[cache.entries.a]"),
            "cache.entries.b.set.SAME is also set by cache entry a",
        ),
        ("[cache]\nroot = \"relative/dir\"\n".into(), "cache.root"),
        ("[cache]\nroot = \"/a/../b\"\n".into(), "cache.root must not contain .."),
        ("[cache]\nroot = \"/nonexistent/project/.cache\"\n".into(), "inside the project root"),
        ("[cache]\nroot = \"/\"\n".into(), "cache.root"),
        ("[cache]\nscope = \"team\"\n".into(), "cache.scope must be one of project, user"),
        ("[cache.rust]\nmode = \"off\"\n".into(), "cache.rust is set but rust is not in cache.presets"),
        ("[cache]\n[cache.setup_reuse]\nkey_files = [\"a\"]\n".into(), "cache.setup_reuse.paths"),
        ("[cache]\n[cache.setup_reuse]\nkey_files = [\"a\"]\npaths = [\"/abs\"]\n".into(), "cache.setup_reuse.paths[0]"),
        ("[cache]\n[cache.setup_reuse]\nkey_files = [\"../a\"]\npaths = [\"b\"]\n".into(), "cache.setup_reuse.key_files[0]"),
        ("[cache]\n[cache.setup_reuse]\nkey_files = [\"a\"]\npaths = [\"b/../c\"]\n".into(), "cache.setup_reuse.paths[0]"),
        ("cache = 1\n".into(), "cache must be a table"),
    ];
    for (extra, expected) in cases {
        // A top-level scalar must come before the first table.
        let result = if extra.starts_with("cache = ") {
            load_with(&extra, "")
        } else {
            load(&extra)
        };
        let message = result.expect_err(&extra);
        assert!(
            message.contains(expected),
            "{extra}\n=> {message}\nexpected {expected}"
        );
    }
}

#[test]
fn a_name_in_env_pass_is_refused_for_a_cache() {
    let text = format!(
        "{BASE}[env]\npass = [\"MY_CACHE\"]\n[cache.entries.mine]\ndirs = {{ MY_CACHE = \"c\" }}\n"
    );
    let message = parse_config(text.as_bytes(), Path::new("/nonexistent"))
        .err()
        .map(|e| format!("{e:?}"))
        .unwrap();
    assert!(
        message.contains("already passed by [env] pass"),
        "{message}"
    );
}

#[test]
fn a_home_relative_or_absolute_root_and_the_setup_reuse_resolve() {
    let config = cache(
        "[cache]\nroot = \"~/.cache/capstan/\"\nscope = \"user\"\n[cache.setup_reuse]\nkey_files = [\"Cargo.lock\"]\npaths = [\"target\", \"node_modules\"]\n",
    );
    assert_eq!(config.base.as_deref(), Some(Path::new("~/.cache/capstan")));
    assert_eq!(config.scope, CacheScope::User);
    let reuse = config.setup_reuse.expect("reuse");
    assert_eq!(reuse.key_files, ["Cargo.lock"]);
    assert_eq!(reuse.paths, ["target", "node_modules"]);
    assert!(cache("[cache]\nroot = \"/var/cache/capstan\"\n")
        .base
        .is_some());
}

#[test]
fn the_json_carries_the_resolved_section() {
    let json = load("[cache]\npresets = [\"go\"]\nmax_age_days = 9\n")
        .unwrap()
        .to_json();
    assert!(json.contains("\"cache\": {"), "{json}");
    assert!(json.contains("\"maxAgeDays\": 9"));
    assert!(json.contains("\"GOCACHE\"") || json.contains("\"env\": \"GOCACHE\""));
    assert!(json.contains("\"subdir\": \"go-build\""));
}

#[test]
fn detectable_holds_the_unlisted_presets_with_their_settings() {
    let cache = cache(
        "[cache]\ndetect = true\npresets = [\"go\"]\n[cache.rust]\nmode = \"shared-build-dir\"\n",
    );
    let names: Vec<&str> = cache.detectable.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, ["rust", "python", "node"]);
    let rust = &cache.detectable[0];
    assert_eq!(rust.mode.as_deref(), Some("shared-build-dir"));
    assert_eq!(rust.origin.as_str(), "detected");
    let off = self::cache("[cache]\npresets = [\"go\"]\n");
    assert!(off.detectable.is_empty());
}

#[test]
fn every_env_name_of_every_preset_is_an_acceptable_agent_variable_name() {
    let toml = "[cache]\npresets = [\"rust\", \"python\", \"node\", \"go\"]\n[cache.rust]\nmode = \"sccache\"\n";
    let config = cache(toml);
    assert_eq!(config.entries.len(), 4);
    for entry in &config.entries {
        for (name, _) in entry.dirs.iter().chain(entry.set.iter()) {
            let valid = !name.is_empty()
                && !name.starts_with(|c: char| c.is_ascii_digit())
                && name
                    .chars()
                    .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_');
            assert!(valid, "{} sets {name}", entry.name);
        }
    }
}
