mod common;
use std::fs;
use std::os::unix::fs::PermissionsExt;

use capstan_cache::{resolve_root, Origin, Runtime};
use capstan_config::{CacheEntry, CacheOrigin, CacheScope};
use common::*;

#[test]
fn root_resolution_follows_config_xdg_home_and_scope() {
    let dir = tempfile::tempdir().unwrap();
    let t = dir.path();
    let mut cfg = config(&t.join("unused"));
    cfg.base = None;
    let xdg = t.join("xdg");
    let e = env(&[
        ("XDG_CACHE_HOME", xdg.to_str().unwrap()),
        ("HOME", t.join("home").to_str().unwrap()),
    ]);
    assert_eq!(
        resolve_root(&cfg, "p1", &e),
        xdg.join("capstan/projects/p1")
    );
    let home_only = env(&[("HOME", t.join("home").to_str().unwrap())]);
    assert_eq!(
        resolve_root(&cfg, "p1", &home_only),
        t.join("home/.cache/capstan/projects/p1")
    );
    cfg.scope = CacheScope::User;
    assert_eq!(resolve_root(&cfg, "p1", &e), xdg.join("capstan/shared"));
    cfg.base = Some(t.join("cfg"));
    assert_eq!(resolve_root(&cfg, "p1", &e), t.join("cfg/shared"));
    cfg.scope = CacheScope::Project;
    let hostile = resolve_root(&cfg, "../evil/x", &e);
    assert_eq!(hostile.parent().unwrap(), t.join("cfg/projects"));
    assert!(!hostile.file_name().unwrap().to_string_lossy().contains('/'));
}

#[test]
fn the_root_is_created_private() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = config(&dir.path().join("base"));
    let root = resolve_root(&cfg, "p", &env(&[]));
    for path in [
        &root,
        root.parent().unwrap(),
        root.parent().unwrap().parent().unwrap(),
    ] {
        assert_eq!(
            fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o700,
            "{path:?}"
        );
    }
}

fn sccache_entry() -> CacheEntry {
    let mut e = custom("rust", "SCCACHE_DIR", "sccache");
    e.origin = CacheOrigin::Preset;
    e.mode = Some("sccache".into());
    e.requires = Some("sccache".into());
    e.set = vec![
        ("RUSTC_WRAPPER".into(), "sccache".into()),
        ("SCCACHE_CACHE_SIZE".into(), "20G".into()),
    ];
    e
}

fn stub_path(dir: &std::path::Path, tool: &str) -> String {
    let bin = dir.join("bin");
    fs::create_dir_all(&bin).unwrap();
    let file = bin.join(tool);
    fs::write(&file, "#!/bin/sh\n").unwrap();
    fs::set_permissions(&file, fs::Permissions::from_mode(0o755)).unwrap();
    bin.to_string_lossy().into_owned()
}

#[test]
fn an_absent_tool_disables_one_entry_with_one_warning() {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = config(&dir.path().join("base"));
    let mut shared = custom("rust2", "CARGO_BUILD_BUILD_DIR", "cargo-build");
    shared.origin = CacheOrigin::Preset;
    shared.mode = Some("shared-build-dir".into());
    cfg.entries = vec![
        sccache_entry(),
        custom("extra", "EXTRA_DIR", "extra"),
        shared,
    ];
    let rt = Runtime::from_config(&cfg, dir.path(), "p", &env(&[]), "/nonexistent-bin");
    let rust = rt.entry("rust").unwrap();
    assert!(!rust.enabled && rust.env.is_empty());
    assert_eq!(rt.warnings.len(), 1, "{:?}", rt.warnings);
    assert!(
        rt.warnings[0].contains("sccache")
            && rt.warnings[0].contains("install")
            && rt.warnings[0].contains("mode")
    );
    assert!(rt.entry("extra").unwrap().enabled);
    assert!(
        rt.entry("rust2").unwrap().enabled,
        "shared-build-dir needs no sccache"
    );
    let with = Runtime::from_config(
        &cfg,
        dir.path(),
        "p",
        &env(&[]),
        &stub_path(dir.path(), "sccache"),
    );
    assert!(with.entry("rust").unwrap().enabled && with.warnings.is_empty());
}

#[test]
fn sccache_size_defaults_to_half_the_total_and_env_dirs_are_absolute() {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = config(&dir.path().join("base"));
    cfg.max_size_bytes = 10 * (1 << 30);
    cfg.entries = vec![sccache_entry()];
    let rt = Runtime::from_config(
        &cfg,
        dir.path(),
        "p",
        &env(&[]),
        &stub_path(dir.path(), "sccache"),
    );
    let env = &rt.entry("rust").unwrap().env;
    assert!(
        env.contains(&("SCCACHE_CACHE_SIZE".into(), "5G".into())),
        "{env:?}"
    );
    let sccache_dir = &env.iter().find(|(k, _)| k == "SCCACHE_DIR").unwrap().1;
    assert_eq!(sccache_dir, &rt.root.join("rust/sccache").to_string_lossy());
}

#[test]
fn detection_finds_root_and_one_level_down_and_stops_at_64() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("proj");
    fs::create_dir_all(project.join("a/deeper")).unwrap();
    fs::write(project.join("go.mod"), "").unwrap();
    fs::write(project.join("a/package.json"), "").unwrap();
    fs::write(project.join("a/deeper/Cargo.toml"), "").unwrap();
    fs::create_dir_all(project.join("node_modules/x")).unwrap();
    fs::write(project.join("node_modules/pyproject.toml"), "").unwrap();
    // 62 more directories sort between "a" and the 65th; the 65th holds a python marker.
    for i in 0..63 {
        fs::create_dir_all(project.join(format!("d{i:02}"))).unwrap();
    }
    fs::create_dir_all(project.join("z65")).unwrap();
    fs::write(project.join("z65/requirements-dev.txt"), "").unwrap();
    let mut cfg = config(&dir.path().join("base"));
    cfg.detect = true;
    for name in ["rust", "python", "node", "go"] {
        let mut e = custom(name, &format!("{}_DIR", name.to_uppercase()), name);
        e.origin = CacheOrigin::Detected;
        cfg.detectable.push(e);
    }
    let rt = runtime(&cfg, &project);
    let names: Vec<&str> = rt.entries.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(
        names,
        ["node", "go"],
        "rust is two levels down, python in the 65th dir or skipped"
    );
    assert!(rt.entries.iter().all(|e| e.origin == Origin::Detected));
    // Wildcard markers match one level down too.
    fs::create_dir_all(project.join("a")).unwrap();
    fs::write(project.join("a/requirements-dev.txt"), "").unwrap();
    assert!(runtime(&cfg, &project).entry("python").is_some());
}

#[test]
fn a_detected_preset_that_clashes_with_an_explicit_entry_is_skipped_with_one_warning() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("proj");
    fs::create_dir_all(&project).unwrap();
    fs::write(project.join("go.mod"), "").unwrap();
    let mut cfg = config(&dir.path().join("base"));
    cfg.detect = true;
    cfg.entries = vec![custom("mine", "GOCACHE", "go")];
    let mut go = custom("go", "GOCACHE", "go-build");
    go.dirs.push(("GOMODCACHE".into(), "go-mod".into()));
    go.origin = CacheOrigin::Detected;
    cfg.detectable = vec![go];
    let rt = runtime(&cfg, &project);
    assert!(rt.entry("go").is_none());
    assert_eq!(rt.warnings.len(), 1);
    assert!(
        rt.warnings[0].contains("go")
            && rt.warnings[0].contains("mine")
            && rt.warnings[0].contains("GOCACHE")
    );
}

#[test]
fn the_setup_entry_is_internal_and_command_environment_is_an_allowlist() {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = config(&dir.path().join("base"));
    cfg.entries = vec![custom("extra", "EXTRA_DIR", "e")];
    cfg.setup_reuse = Some(capstan_config::SetupReuse {
        key_files: vec!["k".into()],
        paths: vec!["p".into()],
    });
    let rt = runtime(&cfg, dir.path());
    let setup = rt.entry("setup").unwrap();
    assert_eq!(setup.origin, Origin::Internal);
    assert!(setup.env.is_empty() && setup.enabled);
    let base = env(&[
        ("PATH", "/bin"),
        ("HOME", "/h"),
        ("GITHUB_TOKEN", "t"),
        ("LANG", "C"),
    ]);
    let map = rt.command_environment(rt.entry("extra").unwrap(), &base);
    let keys: Vec<&str> = map.keys().map(String::as_str).collect();
    assert_eq!(keys, ["EXTRA_DIR", "HOME", "LANG", "PATH"]);
    let _: CacheOrigin = CacheOrigin::Custom;
}

#[test]
fn no_environment_value_other_than_entry_dirs_is_written_under_the_root() {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = config(&dir.path().join("base"));
    cfg.entries = vec![custom("extra", "EXTRA_DIR", "e")];
    let secret = "s3cr3t-value-9f31";
    let e = env(&[("HOME", "/h"), ("MY_SECRET_TOKEN", secret)]);
    let rt = Runtime::from_config(&cfg, dir.path(), "p", &e, "/bin");
    rt.ensure().unwrap();
    let manifest = capstan_cache::Manifest::new(
        &rt,
        capstan_cache::measure(
            &rt,
            std::time::Instant::now() + std::time::Duration::from_secs(5),
        ),
        1,
    );
    capstan_cache::write_manifest(&rt, &manifest).unwrap();
    for path in tree(&rt.root) {
        assert!(!path.contains(secret));
        let full = rt.root.join(&path);
        if full.is_file() {
            assert!(
                !String::from_utf8_lossy(&fs::read(&full).unwrap()).contains(secret),
                "{path}"
            );
        }
    }
}
