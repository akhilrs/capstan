//! The trust gate is on for every load. No test reads the process environment; each store is a private temp dir.
use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use capstan_config::{load_role_config_with, record_current, ConfigError, TrustStore};
use tempfile::TempDir;

const BASE: &str = "schema_version = 1\n[nexora]\ntrack = \"never\"\n[hosts.claude]\nkind = \"claude\"\n[roles.pm]\nkind = \"PM\"\nhost = \"claude\"\n";

struct Fixture {
    _tmp: TempDir,
    project: PathBuf,
    store: TrustStore,
}

fn write(dir: &Path, name: &str, text: &str) {
    let file = dir.join(name);
    fs::write(&file, text).unwrap();
    fs::set_permissions(&file, fs::Permissions::from_mode(0o644)).unwrap();
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

fn untrusted(f: &Fixture, store: &TrustStore) -> String {
    match load_role_config_with(&f.project, store).unwrap_err() {
        ConfigError::Untrusted(text) => text,
        other => panic!("unexpected {other:?}"),
    }
}

fn cache(n: u32) -> String {
    format!("{BASE}[cache]\nmax_size_gb = {n}\n")
}

#[test]
fn no_record_is_untrusted_then_recorded_is_ok() {
    let f = fixture(&cache(1));
    let text = untrusted(&f, &f.store);
    assert!(text.contains("cache.max_size_gb"), "{text}");
    assert!(
        text.ends_with("run: cstan trust") || text.contains("cstan trust"),
        "{text}"
    );
    record_current(&f.project, &f.store).unwrap();
    load_role_config_with(&f.project, &f.store).unwrap();
}

#[test]
fn one_changed_key_names_only_it() {
    let f = fixture(&cache(1));
    record_current(&f.project, &f.store).unwrap();
    write(&f.project, "capstan.toml", &cache(2));
    let text = untrusted(&f, &f.store);
    assert!(
        text.contains("1 key(s) not trusted on this machine: cache.max_size_gb ("),
        "{text}"
    );
}

#[test]
fn schema_version_and_local_only_keys_do_not_matter() {
    let f = fixture(&cache(1));
    record_current(&f.project, &f.store).unwrap();
    write(
        &f.project,
        "capstan.toml",
        &cache(1).replace("schema_version = 1", "schema_version = 2"),
    );
    // a schema_version bump may be refused by the parser itself; the gate must not be what refuses it
    match load_role_config_with(&f.project, &f.store) {
        Ok(_) | Err(ConfigError::Invalid(_)) | Err(ConfigError::Parse(_)) => {}
        Err(other) => panic!("unexpected {other:?}"),
    }
    write(&f.project, "capstan.toml", &cache(1));
    write(
        &f.project,
        "capstan.local.toml",
        "[ntfy]\nenabled = true\nserver = \"https://ntfy.example\"\ntopic = \"t\"\n",
    );
    load_role_config_with(&f.project, &f.store).unwrap();
}

#[test]
fn many_changed_keys_are_capped_at_twenty() {
    let mut text = String::from(BASE);
    for n in 0..25 {
        text.push_str(&format!("[hosts.h{n:02}]\nkind = \"claude\"\n"));
    }
    let f = fixture(&text);
    let said = untrusted(&f, &f.store);
    // the 25 added hosts and the four keys of BASE
    assert!(said.contains("29 key(s)"), "{said}");
    assert!(said.contains(", and 9 more ("), "{said}");
}

#[test]
fn no_usable_store_names_the_remedy() {
    let f = fixture(&cache(1));
    for store in [
        TrustStore::unset(),
        TrustStore::for_user_dir(Some(OsString::from("relative/dir"))),
    ] {
        let text = untrusted(&f, &store);
        assert!(text.contains("no trust store"), "{text}");
        assert!(text.contains("Set CAPSTAN_TRUST_DIR"), "{text}");
        assert!(text.contains("cache.max_size_gb"), "{text}");
    }
}

#[test]
fn a_parse_error_wins_over_the_gate() {
    let f = fixture("this is = = not toml\n");
    assert!(matches!(
        load_role_config_with(&f.project, &f.store).unwrap_err(),
        ConfigError::Invalid(_) | ConfigError::Parse(_)
    ));
}

#[test]
fn a_file_with_no_checked_key_is_never_untrusted() {
    // A loadable configuration always names a host, whose kind is a checked key, so the loader's own refusal of the
    // text is what comes back here; the gate itself says NotNeeded and adds nothing.
    let f = fixture("schema_version = 1\n");
    let result = load_role_config_with(&f.project, &TrustStore::unset());
    assert!(
        !matches!(result, Err(ConfigError::Untrusted(_))),
        "{result:?}"
    );
}
