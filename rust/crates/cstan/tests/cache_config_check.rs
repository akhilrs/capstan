//! `cstan config check` on a `capstan.toml` with a `[cache]` section: a valid one passes and prints the resolved cache, an
//! invalid entry exits non-zero and says why.
use std::path::PathBuf;
use std::process::{Command, Stdio};

const BASE: &str = "schema_version = 1\n[nexora]\ntrack = \"never\"\n[hosts.claude]\nkind = \"claude\"\n[roles.pm]\nkind = \"PM\"\nhost = \"claude\"\n";

fn check(label: &str, cache: &str) -> std::process::Output {
    let dir: PathBuf = std::env::temp_dir().join(format!(
        "capstan-cache-check-{label}-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("capstan.toml"), format!("{BASE}{cache}")).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_cstan"))
        .args(["config", "check"])
        .current_dir(&dir)
        .env_clear()
        .env("PATH", "/usr/local/bin:/usr/bin:/bin")
        .stdin(Stdio::null())
        .output()
        .expect("cstan runs");
    std::fs::remove_dir_all(&dir).ok();
    output
}

#[test]
fn a_valid_cache_section_passes_and_is_printed() {
    let output = check(
        "valid",
        "[cache]\npresets = [\"rust\", \"node\"]\n[cache.entries.bundler]\ndirs = { BUNDLE_PATH = \"bundle\" }\n",
    );
    let out = String::from_utf8_lossy(&output.stdout);
    assert_eq!(
        output.status.code(),
        Some(0),
        "{out}{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(out.contains("\"cache\""), "{out}");
    assert!(
        out.contains("\"SCCACHE_DIR\"") && out.contains("\"BUNDLE_PATH\""),
        "{out}"
    );
}

#[test]
fn an_invalid_cache_entry_exits_non_zero_with_the_reason() {
    let output = check(
        "invalid",
        "[cache.entries.bad]\ndirs = { API_TOKEN = \"x\" }\n",
    );
    assert_eq!(output.status.code(), Some(3));
    let err = String::from_utf8_lossy(&output.stderr);
    assert!(err.contains("cache.entries.bad.dirs.API_TOKEN"), "{err}");
}
