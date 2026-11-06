//! The trust store, record and status. Every test names its own store directory; no process environment is read.
use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::{symlink, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;

use capstan_config::{
    checked_subset, local_file_tracking_with, read_config_pair, record_current, record_trust,
    trust_status_with, ConfigPair, LocalTracking, TrustChange, TrustError, TrustStatus, TrustStore,
};
use tempfile::TempDir;

fn non_root() {
    // SAFETY: geteuid has no preconditions.
    assert!(unsafe { libc::geteuid() } != 0, "run as a non-root user");
}

struct Fixture {
    _tmp: TempDir,
    root: PathBuf,
    project: PathBuf,
    store: TrustStore,
    store_dir: PathBuf,
}

fn fixture(config: &str) -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(tmp.path()).unwrap();
    let project = root.join("project");
    fs::create_dir(&project).unwrap();
    write_config(&project, config);
    let store_dir = root.join("store");
    Fixture {
        store: TrustStore::at(&store_dir),
        _tmp: tmp,
        root,
        project,
        store_dir,
    }
}

fn write_config(dir: &Path, text: &str) {
    let file = dir.join("capstan.toml");
    fs::write(&file, text).unwrap();
    fs::set_permissions(&file, fs::Permissions::from_mode(0o644)).unwrap();
}

fn write_local(dir: &Path, text: &str) {
    let file = dir.join("capstan.local.toml");
    fs::write(&file, text).unwrap();
    fs::set_permissions(&file, fs::Permissions::from_mode(0o644)).unwrap();
}

fn git(dir: &Path, args: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.name=t", "-c", "user.email=t@example.com"])
        .args([
            "-c",
            "init.defaultBranch=main",
            "-c",
            "protocol.file.allow=always",
        ])
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .output()
        .expect("git must be installed");
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn needed(status: &TrustStatus) -> (&String, &Vec<capstan_config::KeyChange>, bool) {
    match status {
        TrustStatus::Needed {
            digest,
            changes,
            invalid_record,
            ..
        } => (digest, changes, *invalid_record),
        other => panic!("expected Needed, got {other:?}"),
    }
}

fn digest_of(f: &Fixture) -> String {
    needed(&trust_status_with(&f.project, &f.store)).0.clone()
}

fn trust(f: &Fixture) {
    let digest = digest_of(f);
    record_trust(&f.project, &digest, &f.store).unwrap();
}

fn kinds(status: &TrustStatus) -> Vec<(String, TrustChange)> {
    needed(status)
        .1
        .iter()
        .map(|c| (c.key.clone(), c.change))
        .collect()
}

fn record_files(store_dir: &Path) -> Vec<PathBuf> {
    fs::read_dir(store_dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .collect()
}

const BASE: &str = "[cache]\nmax_size_gb = 5\n[roles.dev]\nmodel = \"m1\"\n";

#[test]
fn first_run_adds_every_key_then_trust_makes_it_trusted() {
    non_root();
    let f = fixture(BASE);
    let status = trust_status_with(&f.project, &f.store);
    assert_eq!(
        kinds(&status),
        vec![
            ("cache.max_size_gb".to_string(), TrustChange::Added),
            ("roles.dev.model".to_string(), TrustChange::Added)
        ]
    );
    assert!(!needed(&status).2);
    trust(&f);
    assert_eq!(
        trust_status_with(&f.project, &f.store),
        TrustStatus::Trusted
    );
}

#[test]
fn one_change_lists_just_that_key() {
    non_root();
    let f = fixture(BASE);
    trust(&f);
    write_config(
        &f.project,
        "[cache]\nmax_size_gb = 5\n[roles.dev]\nmodel = \"m2\"\n",
    );
    let status = trust_status_with(&f.project, &f.store);
    assert_eq!(
        kinds(&status),
        vec![("roles.dev.model".to_string(), TrustChange::Changed)]
    );
}

#[test]
fn removed_and_set_to_default_keys() {
    non_root();
    let f = fixture(BASE);
    trust(&f);
    write_config(&f.project, "[cache]\nmax_size_gb = 5\n");
    assert_eq!(
        kinds(&trust_status_with(&f.project, &f.store)),
        vec![("roles.dev.model".to_string(), TrustChange::Removed)]
    );
    write_config(
        &f.project,
        "[cache]\nmax_size_gb = 5\n[roles.dev]\nmodel = \"m1\"\n[limits]\nmax_workers = 4\n",
    );
    assert_eq!(
        kinds(&trust_status_with(&f.project, &f.store)),
        vec![("limits.max_workers".to_string(), TrustChange::Added)]
    );
}

#[test]
fn revert_is_trusted_and_the_record_is_untouched() {
    non_root();
    let f = fixture(BASE);
    trust(&f);
    let record = record_files(&f.store_dir).remove(0);
    let before = fs::metadata(&record).unwrap().modified().unwrap();
    write_config(&f.project, "[cache]\nmax_size_gb = 6\n");
    assert!(matches!(
        trust_status_with(&f.project, &f.store),
        TrustStatus::Needed { .. }
    ));
    write_config(&f.project, BASE);
    assert_eq!(
        trust_status_with(&f.project, &f.store),
        TrustStatus::Trusted
    );
    assert_eq!(fs::metadata(&record).unwrap().modified().unwrap(), before);
    // A Trusted record is never rewritten.
    let digest = digest_of_trusted(&f);
    record_trust(&f.project, &digest, &f.store).unwrap();
    assert_eq!(fs::metadata(&record).unwrap().modified().unwrap(), before);
}

fn digest_of_trusted(f: &Fixture) -> String {
    let pair = read_config_pair(&f.project).unwrap().unwrap();
    let subset = checked_subset(&pair).unwrap();
    // The digest the status path computes, found by making the file differ and reading the next Needed.
    fixture_like(f, &subset)
}

fn fixture_like(f: &Fixture, _subset: &BTreeMap<String, serde_json::Value>) -> String {
    // Same bytes in another folder with no record give the same digest.
    let other = f.root.join("probe");
    fs::create_dir_all(&other).unwrap();
    fs::copy(f.project.join("capstan.toml"), other.join("capstan.toml")).unwrap();
    let store = TrustStore::at(f.root.join("probe-store"));
    needed(&trust_status_with(&other, &store)).0.clone()
}

#[test]
fn trusting_b_then_a_needs_trust_again() {
    non_root();
    let f = fixture(BASE);
    let b = "[cache]\nmax_size_gb = 6\n";
    trust(&f);
    write_config(&f.project, b);
    trust(&f);
    write_config(&f.project, BASE);
    assert!(matches!(
        trust_status_with(&f.project, &f.store),
        TrustStatus::Needed { .. }
    ));
}

#[test]
fn local_file_untracked_is_left_out_and_unknown_is_checked() {
    non_root();
    let f = fixture(BASE);
    git(&f.project, &["init", "-q"]);
    write_local(
        &f.project,
        "[roles.dev]\nmodel = \"local\"\n[cache]\nmax_size_gb = 9\n",
    );
    let pair = read_config_pair(&f.project).unwrap().unwrap();
    assert_eq!(pair.tracking, LocalTracking::Untracked);
    let subset = checked_subset(&pair).unwrap();
    assert_eq!(subset["roles.dev.model"], serde_json::json!("m1"));
    assert_eq!(subset["cache.max_size_gb"], serde_json::json!(5));
    trust(&f);
    write_local(&f.project, "[roles.dev]\nmodel = \"other\"\n");
    assert_eq!(
        trust_status_with(&f.project, &f.store),
        TrustStatus::Trusted
    );

    let unknown = ConfigPair {
        tracking: LocalTracking::Unknown,
        ..pair.clone()
    };
    let subset = checked_subset(&unknown).unwrap();
    assert_eq!(subset["roles.dev.model"], serde_json::json!("local"));
    assert_eq!(subset["cache.max_size_gb"], serde_json::json!(9));
    let status = capstan_config::trust_status_of_bytes(&f.project, &unknown, &f.store);
    assert!(matches!(status, TrustStatus::Needed { .. }));

    // No repository: also left out.
    let nr = ConfigPair {
        tracking: LocalTracking::NoRepository,
        ..pair
    };
    assert_eq!(
        checked_subset(&nr).unwrap()["cache.max_size_gb"],
        serde_json::json!(5)
    );
}

#[test]
fn tracked_local_file_is_unavailable_with_the_hint() {
    non_root();
    let f = fixture(BASE);
    git(&f.project, &["init", "-q"]);
    write_local(&f.project, "[roles.dev]\nmodel = \"local\"\n");
    git(&f.project, &["add", "-f", "capstan.local.toml"]);
    let pair = read_config_pair(&f.project).unwrap().unwrap();
    assert_eq!(pair.tracking, LocalTracking::Tracked);
    match trust_status_with(&f.project, &f.store) {
        TrustStatus::Unavailable { reason, .. } => {
            assert!(
                reason.contains("git rm --cached capstan.local.toml"),
                "{reason}"
            )
        }
        other => panic!("{other:?}"),
    }
    assert!(checked_subset(&pair).is_err());
}

#[test]
fn a_file_with_no_keys_needs_no_trust() {
    non_root();
    let f = fixture("# nothing here\n");
    assert_eq!(
        trust_status_with(&f.project, &f.store),
        TrustStatus::NotNeeded
    );
    assert!(!f.store_dir.exists());
    let none = f.root.join("none");
    fs::create_dir(&none).unwrap();
    assert_eq!(trust_status_with(&none, &f.store), TrustStatus::NotNeeded);
}

#[test]
fn tampered_records_trust_nothing() {
    non_root();
    let f = fixture(BASE);
    trust(&f);
    let record = record_files(&f.store_dir).remove(0);
    let good = fs::read_to_string(&record).unwrap();
    let digest = needed_or_trusted_digest(&f);
    let tamper = |text: String, mode: u32| {
        fs::remove_file(&record).unwrap();
        fs::write(&record, text).unwrap();
        fs::set_permissions(&record, fs::Permissions::from_mode(mode)).unwrap();
        let status = trust_status_with(&f.project, &f.store);
        assert!(needed(&status).2, "expected invalid_record for {status:?}");
        assert!(!needed(&status).1.is_empty());
    };
    tamper(good.replace(&digest, &"0".repeat(64)), 0o600);
    let value_hash = good
        .lines()
        .find(|l| l.contains("\"hash\""))
        .unwrap()
        .to_string();
    tamper(
        good.replace(&value_hash, "      \"hash\": \"deadbeef\","),
        0o600,
    );
    tamper(good.clone(), 0o644);
    tamper(good.replace("\"version\": 1", "\"version\": 2"), 0o600);
    tamper(
        good.replace("\"root\": \"", "\"root\": \"/elsewhere"),
        0o600,
    );
    tamper("not json".to_string(), 0o600);
    // A symlink in place of the record.
    fs::remove_file(&record).unwrap();
    let real = f.root.join("real.json");
    fs::write(&real, &good).unwrap();
    fs::set_permissions(&real, fs::Permissions::from_mode(0o600)).unwrap();
    symlink(&real, &record).unwrap();
    assert!(needed(&trust_status_with(&f.project, &f.store)).2);
}

fn needed_or_trusted_digest(f: &Fixture) -> String {
    digest_of_trusted(f)
}

#[test]
fn missing_store_dir_is_needed_and_creates_nothing() {
    non_root();
    let f = fixture(BASE);
    let status = trust_status_with(&f.project, &f.store);
    assert!(matches!(status, TrustStatus::Needed { .. }));
    assert!(!f.store_dir.exists());
}

#[test]
fn unusable_store_dirs_are_unavailable_and_name_the_fix() {
    non_root();
    let f = fixture(BASE);
    fs::create_dir(&f.store_dir).unwrap();
    fs::set_permissions(&f.store_dir, fs::Permissions::from_mode(0o755)).unwrap();
    match trust_status_with(&f.project, &f.store) {
        TrustStatus::Unavailable { reason, store_dir } => {
            assert!(reason.contains(f.store_dir.to_str().unwrap()), "{reason}");
            assert!(reason.contains("chmod 700"), "{reason}");
            assert_eq!(store_dir, Some(f.store_dir.clone()));
        }
        other => panic!("{other:?}"),
    }
    assert!(matches!(
        record_trust(&f.project, "x", &f.store),
        Err(TrustError::DigestChanged) | Err(TrustError::Unavailable(_))
    ));
    fs::remove_dir(&f.store_dir).unwrap();
    let real = f.root.join("real-store");
    fs::create_dir(&real).unwrap();
    fs::set_permissions(&real, fs::Permissions::from_mode(0o700)).unwrap();
    symlink(&real, &f.store_dir).unwrap();
    match trust_status_with(&f.project, &f.store) {
        TrustStatus::Unavailable { reason, .. } => {
            assert!(reason.contains(f.store_dir.to_str().unwrap()), "{reason}")
        }
        other => panic!("{other:?}"),
    }
    let digest = digest_of_trusted(&f);
    match record_trust(&f.project, &digest, &f.store) {
        Err(TrustError::Unavailable(reason)) => {
            assert!(reason.contains(f.store_dir.to_str().unwrap()))
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn for_user_with_rejects_unusable_settings() {
    let unavailable = |store: TrustStore| {
        matches!(
            trust_status_with(Path::new("/nonexistent-capstan-dir"), &store),
            TrustStatus::NotNeeded
        ) && {
            let f = fixture(BASE);
            matches!(
                trust_status_with(&f.project, &store),
                TrustStatus::Unavailable { .. }
            )
        }
    };
    let home = |_| Some(PathBuf::from("/home/someone"));
    assert!(unavailable(TrustStore::for_user_with(
        home,
        Some("rel/dir".into())
    )));
    assert!(unavailable(TrustStore::for_user_with(|_| None, None)));
    assert!(unavailable(TrustStore::for_user_with(
        |_| Some(PathBuf::from("relative-home")),
        None
    )));
    assert!(unavailable(TrustStore::for_user_with(
        |_| Some(PathBuf::new()),
        None
    )));
    assert!(unavailable(TrustStore::unset()));
    // Usable choices.
    assert_eq!(
        TrustStore::for_user_with(home, None),
        TrustStore::at("/home/someone/.local/state/capstan/trust")
    );
    assert_eq!(
        TrustStore::for_user_with(home, Some("/abs/trust".into())),
        TrustStore::at("/abs/trust")
    );
}

#[test]
fn no_store_dir_to_name_gives_none() {
    non_root();
    let f = fixture(BASE);
    for store in [
        TrustStore::for_user_with(|_| None, None),
        TrustStore::unset(),
    ] {
        match trust_status_with(&f.project, &store) {
            TrustStatus::Unavailable { store_dir, reason } => {
                assert_eq!(store_dir, None);
                if store == TrustStore::unset() {
                    assert_eq!(reason, "no trust store set");
                }
            }
            other => panic!("{other:?}"),
        }
    }
}

#[test]
fn two_checkouts_have_two_records() {
    non_root();
    let f = fixture(BASE);
    let second = f.root.join("second");
    fs::create_dir(&second).unwrap();
    write_config(&second, BASE);
    trust(&f);
    assert!(matches!(
        trust_status_with(&second, &f.store),
        TrustStatus::Needed { .. }
    ));
    let digest = needed(&trust_status_with(&second, &f.store)).0.clone();
    record_trust(&second, &digest, &f.store).unwrap();
    assert_eq!(record_files(&f.store_dir).len(), 2);
}

#[test]
fn linked_worktree_and_symlinked_path_share_the_main_record() {
    non_root();
    let f = fixture(BASE);
    git(&f.project, &["init", "-q"]);
    fs::write(f.project.join("a"), "a").unwrap();
    git(&f.project, &["add", "a"]);
    git(&f.project, &["commit", "-q", "-m", "x"]);
    let wt = f.root.join("wt");
    git(
        &f.project,
        &["worktree", "add", "-q", wt.to_str().unwrap(), "-b", "wtb"],
    );
    write_config(&wt, BASE);
    trust(&f);
    assert_eq!(trust_status_with(&wt, &f.store), TrustStatus::Trusted);
    let digest = digest_of_trusted(&f);
    let written = record_trust(&wt, &digest, &f.store).unwrap();
    assert!(written.linked_worktree);
    assert_eq!(written.record_key_dir, f.project);
    let main = record_trust(&f.project, &digest, &f.store).unwrap();
    assert!(!main.linked_worktree);
    assert_eq!(record_files(&f.store_dir).len(), 1);

    let link = f.root.join("link");
    symlink(&f.project, &link).unwrap();
    assert_eq!(trust_status_with(&link, &f.store), TrustStatus::Trusted);
}

#[test]
fn submodule_separate_git_dir_and_bare_worktree_use_their_own_record() {
    non_root();
    let f = fixture(BASE);
    // Separate git dir.
    let sep = f.root.join("sep");
    fs::create_dir(&sep).unwrap();
    git(
        &sep,
        &[
            "init",
            "-q",
            "--separate-git-dir",
            f.root.join("sep.git").to_str().unwrap(),
        ],
    );
    write_config(&sep, BASE);
    let (_, _) = own_key(&sep, &f);

    // Submodule shaped link: a git dir with no commondir.
    let sub = f.root.join("sub");
    fs::create_dir(&sub).unwrap();
    let modules = f.root.join("modules-sub");
    fs::create_dir(&modules).unwrap();
    fs::write(sub.join(".git"), format!("gitdir: {}\n", modules.display())).unwrap();
    write_config(&sub, BASE);
    own_key(&sub, &f);

    // A worktree of a bare repository.
    let src = f.root.join("src");
    fs::create_dir(&src).unwrap();
    git(&src, &["init", "-q"]);
    fs::write(src.join("a"), "a").unwrap();
    git(&src, &["add", "a"]);
    git(&src, &["commit", "-q", "-m", "x"]);
    let bare = f.root.join("bare.git");
    git(
        &f.root,
        &[
            "clone",
            "-q",
            "--bare",
            src.to_str().unwrap(),
            bare.to_str().unwrap(),
        ],
    );
    let bw = f.root.join("bw");
    git(
        &bare,
        &["worktree", "add", "-q", bw.to_str().unwrap(), "-b", "bwb"],
    );
    write_config(&bw, BASE);
    own_key(&bw, &f);
}

fn own_key(dir: &Path, f: &Fixture) -> (PathBuf, bool) {
    let status = trust_status_with(dir, &f.store);
    let digest = needed(&status).0.clone();
    let written = record_trust(dir, &digest, &f.store).unwrap();
    assert_eq!(written.record_key_dir, fs::canonicalize(dir).unwrap());
    assert!(!written.linked_worktree);
    (written.record_key_dir, written.linked_worktree)
}

#[test]
fn broken_git_links_are_unavailable() {
    non_root();
    let f = fixture(BASE);
    let unavailable = |dir: &Path| match trust_status_with(dir, &f.store) {
        TrustStatus::Unavailable { .. } => {}
        other => panic!("{other:?}"),
    };
    // Malformed .git file.
    fs::write(f.project.join(".git"), "garbage\n").unwrap();
    unavailable(&f.project);
    // Missing gitdir.
    fs::write(
        f.project.join(".git"),
        "gitdir: /nonexistent-capstan-gitdir\n",
    )
    .unwrap();
    unavailable(&f.project);
    // Unreadable commondir (a directory in its place).
    let gitdir = f.root.join("gd");
    fs::create_dir_all(gitdir.join("commondir")).unwrap();
    fs::write(
        f.project.join(".git"),
        format!("gitdir: {}\n", gitdir.display()),
    )
    .unwrap();
    unavailable(&f.project);
    let digest = "0".repeat(64);
    assert!(matches!(
        record_trust(&f.project, &digest, &f.store),
        Err(TrustError::DigestChanged | TrustError::Unavailable(_))
    ));
}

#[test]
fn a_common_dir_not_named_dot_git_uses_the_own_record() {
    non_root();
    let f = fixture(BASE);
    let common = f.root.join("shared.git");
    let gitdir = common.join("worktrees").join("w");
    fs::create_dir_all(&gitdir).unwrap();
    fs::write(gitdir.join("commondir"), "../..\n").unwrap();
    fs::write(
        f.project.join(".git"),
        format!("gitdir: {}\n", gitdir.display()),
    )
    .unwrap();
    own_key(&f.project, &f);
}

#[test]
fn int_float_and_key_order() {
    non_root();
    let a = fixture("[cache]\nmax_size_gb = 1\n[limits]\nmax_workers = 2\n");
    let b = fixture("[limits]\nmax_workers = 2\n[cache]\nmax_size_gb = 1\n");
    assert_eq!(digest_of(&a), digest_of(&b));
    let c = fixture("[cache]\nmax_size_gb = 1.0\n[limits]\nmax_workers = 2\n");
    assert_ne!(digest_of(&a), digest_of(&c));
    assert_eq!(digest_of(&c), digest_of(&c));
}

#[test]
fn a_stale_digest_is_refused() {
    non_root();
    let f = fixture(BASE);
    let digest = digest_of(&f);
    write_config(&f.project, "[cache]\nmax_size_gb = 7\n");
    assert_eq!(
        record_trust(&f.project, &digest, &f.store),
        Err(TrustError::DigestChanged)
    );
    assert!(!f.store_dir.exists());
}

#[test]
fn a_fractional_value_can_be_trusted() {
    non_root();
    let f = fixture("[cache]\nmax_size_gb = 0.5\n");
    trust(&f);
    assert_eq!(
        trust_status_with(&f.project, &f.store),
        TrustStatus::Trusted
    );
    let record = fs::read_to_string(record_files(&f.store_dir).remove(0)).unwrap();
    assert!(record.contains("0.5"), "{record}");
}

#[test]
fn dates_and_big_integers_are_never_trusted() {
    non_root();
    for text in [
        "[cache]\nmax_age_days = 2020-01-01\n",
        "[limits]\nmax_workers = 99999999999999999999\n",
        "[cache]\nmax_size_gb = inf\n",
    ] {
        let f = fixture(text);
        let status = trust_status_with(&f.project, &f.store);
        assert!(matches!(status, TrustStatus::Needed { .. }), "{text}");
        let digest = needed(&status).0.clone();
        assert!(matches!(
            record_trust(&f.project, &digest, &f.store),
            Err(TrustError::Unavailable(_))
        ));
        assert!(matches!(
            trust_status_with(&f.project, &f.store),
            TrustStatus::Needed { .. }
        ));
        let pair = read_config_pair(&f.project).unwrap().unwrap();
        assert!(checked_subset(&pair).is_err());
    }
}

#[test]
fn a_write_failure_is_io_and_an_existing_store_dir_is_accepted() {
    non_root();
    let f = fixture(BASE);
    let digest = digest_of(&f);
    // The store dir was made by another writer first, with the right owner and mode.
    fs::create_dir(&f.store_dir).unwrap();
    fs::set_permissions(&f.store_dir, fs::Permissions::from_mode(0o700)).unwrap();
    // A directory sits where the record goes, so the rename cannot succeed.
    let (key, _) = (fs::canonicalize(&f.project).unwrap(), ());
    let name = sha256_hex(key.to_str().unwrap());
    fs::create_dir(f.store_dir.join(format!("{name}.json"))).unwrap();
    assert!(matches!(
        record_trust(&f.project, &digest, &f.store),
        Err(TrustError::Io(_))
    ));
    fs::remove_dir(f.store_dir.join(format!("{name}.json"))).unwrap();
    record_trust(&f.project, &digest, &f.store).unwrap();
    let mode = fs::metadata(f.store_dir.join(format!("{name}.json")))
        .unwrap()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o600);
    assert_eq!(fs::metadata(&f.store_dir).unwrap().mode() & 0o777, 0o700);
    assert!(fs::read_dir(&f.store_dir).unwrap().all(|e| !e
        .unwrap()
        .file_name()
        .to_string_lossy()
        .ends_with(".tmp")));
}

fn sha256_hex(text: &str) -> String {
    capstan_config::sha256(text)
}

#[test]
fn the_store_dir_is_created_private_with_parents() {
    non_root();
    let f = fixture(BASE);
    let nested = f.root.join("a").join("b").join("trust");
    let store = TrustStore::at(&nested);
    let digest = needed(&trust_status_with(&f.project, &store)).0.clone();
    record_trust(&f.project, &digest, &store).unwrap();
    assert_eq!(fs::metadata(&nested).unwrap().mode() & 0o777, 0o700);
    assert_eq!(trust_status_with(&f.project, &store), TrustStatus::Trusted);
    assert!(record_current(&f.project, &store).is_ok());
}

#[test]
fn unreadable_or_invalid_config_files_are_never_trusted() {
    non_root();
    let f = fixture(BASE);
    let file = f.project.join("capstan.toml");
    fs::set_permissions(&file, fs::Permissions::from_mode(0o000)).unwrap();
    assert!(matches!(
        trust_status_with(&f.project, &f.store),
        TrustStatus::Unavailable { .. }
    ));
    fs::remove_file(&file).unwrap();
    fs::create_dir(&file).unwrap();
    assert!(matches!(
        trust_status_with(&f.project, &f.store),
        TrustStatus::Unavailable { .. }
    ));
    fs::remove_dir(&file).unwrap();
    fs::write(&file, [0xff, 0xfe, 0x41]).unwrap();
    fs::set_permissions(&file, fs::Permissions::from_mode(0o644)).unwrap();
    assert!(matches!(
        trust_status_with(&f.project, &f.store),
        TrustStatus::Unavailable { .. }
    ));
    write_config(&f.project, "this is = = not toml");
    assert!(matches!(
        trust_status_with(&f.project, &f.store),
        TrustStatus::Unavailable { .. }
    ));
}

#[test]
fn a_fixture_root_through_a_symlinked_temp_dir_matches() {
    non_root();
    let f = fixture(BASE);
    trust(&f);
    let link = f.root.join("via-link");
    symlink(&f.root, &link).unwrap();
    assert_eq!(
        trust_status_with(&link.join("project"), &f.store),
        TrustStatus::Trusted
    );
}

#[test]
fn local_file_tracking_with_a_failing_stub_git_is_unknown() {
    non_root();
    let f = fixture(BASE);
    let stub = f.root.join("git-stub");
    fs::write(&stub, "#!/bin/sh\nexit 2\n").unwrap();
    fs::set_permissions(&stub, fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(
        local_file_tracking_with(&f.project, &stub),
        LocalTracking::Unknown
    );
    assert_eq!(
        local_file_tracking_with(&f.project, &f.root.join("no-such-git")),
        LocalTracking::Unknown
    );
    let ok = f.root.join("git-ok");
    fs::write(&ok, "#!/bin/sh\nexit 1\n").unwrap();
    fs::set_permissions(&ok, fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(
        local_file_tracking_with(&f.project, &ok),
        LocalTracking::Untracked
    );
}
