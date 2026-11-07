//! The trust gate of `load_role_config_with`. No test reads the process environment.
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use capstan_config::{
    load_role_config_with, read_config_pair, record_current, trust_status_of_bytes, ConfigError,
    TrustStatus, TrustStore,
};
use tempfile::TempDir;

const BASE: &str = "schema_version = 1\n[nexora]\ntrack = \"never\"\n[hosts.claude]\nkind = \"claude\"\n[roles.pm]\nkind = \"PM\"\nhost = \"claude\"\n";
const CHECKED: &str = "[cache]\nmax_size_gb = 1\n";

fn with(extra: &str) -> String {
    format!("{BASE}{extra}")
}

struct Fixture {
    _tmp: TempDir,
    project: PathBuf,
    store: TrustStore,
}

fn fixture(config: &str) -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(tmp.path()).unwrap();
    let project = root.join("project");
    fs::create_dir(&project).unwrap();
    write(&project, "capstan.toml", config);
    Fixture {
        store: TrustStore::at(root.join("store")),
        _tmp: tmp,
        project,
    }
}

fn write(dir: &Path, name: &str, text: &str) {
    let file = dir.join(name);
    fs::write(&file, text).unwrap();
    fs::set_permissions(&file, fs::Permissions::from_mode(0o644)).unwrap();
}

#[test]
fn a_refused_text_comes_back_as_the_parse_error() {
    let bad_toml = fixture("this is = = not toml\n");
    match load_role_config_with(&bad_toml.project, &bad_toml.store).unwrap_err() {
        ConfigError::Invalid(_) | ConfigError::Parse(_) => {}
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn the_gate_refuses_needed_and_unavailable_and_loads_trusted() {
    let f = fixture(&with(CHECKED));
    match load_role_config_with(&f.project, &f.store).unwrap_err() {
        ConfigError::Untrusted(text) => {
            assert!(
                text.starts_with(
                    "capstan.toml has 5 key(s) not trusted on this machine: cache.max_size_gb, "
                ),
                "{text}"
            );
            assert!(text.contains("(trust store "), "{text}");
        }
        other => panic!("unexpected {other:?}"),
    }
    match load_role_config_with(&f.project, &TrustStore::unset()).unwrap_err() {
        ConfigError::Untrusted(text) => assert!(
            text.contains("(no trust store: no trust store set)"),
            "{text}"
        ),
        other => panic!("unexpected {other:?}"),
    }
    record_current(&f.project, &f.store).unwrap();
    load_role_config_with(&f.project, &f.store).unwrap();
}

#[test]
fn a_rewrite_after_the_read_does_not_change_what_was_judged() {
    let f = fixture(&with(CHECKED));
    record_current(&f.project, &f.store).unwrap();
    let pair = read_config_pair(&f.project).unwrap().unwrap();
    write(
        &f.project,
        "capstan.toml",
        &with("[cache]\nmax_size_gb = 9\n"),
    );
    assert_eq!(
        trust_status_of_bytes(&f.project, &pair, &f.store),
        TrustStatus::Trusted
    );
    assert!(matches!(
        load_role_config_with(&f.project, &f.store),
        Err(ConfigError::Untrusted(_))
    ));
}
