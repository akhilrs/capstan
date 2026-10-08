use capstan_ledger::{LedgerError, ProjectLock};
use std::fs;
use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::Path;

fn err(r: Result<ProjectLock, LedgerError>) -> String {
    match r {
        Ok(_) => panic!("acquired"),
        Err(e) => e.to_string(),
    }
}

#[test]
fn second_holder_is_refused_and_release_is_seen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("controller.lock");
    let mut first = ProjectLock::acquire(&path).unwrap();
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    first.assert_held().unwrap();
    assert!(matches!(
        ProjectLock::acquire(&path),
        Err(LedgerError::ProjectLockHeld)
    ));
    assert_eq!(
        err(ProjectLock::acquire(&path)),
        "another cooperating controller owns this project"
    );
    first.close();
    assert_eq!(
        first.assert_held().unwrap_err().to_string(),
        "controller ownership has been released"
    );
    let again = ProjectLock::acquire(&path).unwrap();
    drop(again);
    ProjectLock::acquire(&path).unwrap();
}

#[test]
fn lock_file_records_the_owner() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("controller.lock");
    drop(ProjectLock::acquire(&path).unwrap());
    let conn = rusqlite::Connection::open(&path).unwrap();
    let (pid, started, token): (i64, String, String) = conn
        .query_row("SELECT pid, started_at, token FROM owner", [], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })
        .unwrap();
    assert_eq!(pid, i64::from(std::process::id()));
    assert_eq!(started.len(), 24);
    assert_eq!(token.len(), 32);
}

#[test]
fn refuses_unsafe_lock_files_with_the_node_texts() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("elsewhere");
    fs::write(&target, b"").unwrap();
    let link = dir.path().join("link.lock");
    symlink(&target, &link).unwrap();
    assert_eq!(
        err(ProjectLock::acquire(&link)),
        "controller lock path must identify a regular file"
    );

    let open = dir.path().join("open.lock");
    fs::write(&open, b"").unwrap();
    fs::set_permissions(&open, fs::Permissions::from_mode(0o644)).unwrap();
    assert_eq!(
        err(ProjectLock::acquire(&open)),
        "controller lock file must be private to the current user"
    );

    let junk = dir.path().join("junk.lock");
    fs::write(&junk, b"this is not a database, it is much longer than a sqlite header ........................................").unwrap();
    fs::set_permissions(&junk, fs::Permissions::from_mode(0o600)).unwrap();
    assert_eq!(
        err(ProjectLock::acquire(&junk)),
        "controller lock file is not a lock database: remove it with the daemon stopped"
    );

    let as_dir = dir.path().join("dir.lock");
    fs::create_dir(&as_dir).unwrap();
    assert!(ProjectLock::acquire(&as_dir).is_err());
}

#[test]
fn refuses_relative_paths_and_bad_directories() {
    assert_eq!(
        err(ProjectLock::acquire(Path::new("controller.lock"))),
        "lock path must be absolute"
    );
    let dir = tempfile::tempdir().unwrap();
    let real = dir.path().join("real");
    fs::create_dir(&real).unwrap();
    let link = dir.path().join("linked");
    symlink(&real, &link).unwrap();
    assert_eq!(
        err(ProjectLock::acquire(&link.join("controller.lock"))),
        "controller state directory must be a real directory"
    );
}

#[test]
fn assert_held_notices_a_removed_or_replaced_lock_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("controller.lock");
    let lock = ProjectLock::acquire(&path).unwrap();
    fs::remove_file(&path).unwrap();
    assert_eq!(
        lock.assert_held().unwrap_err().to_string(),
        "controller lock path was removed"
    );
    fs::write(&path, b"").unwrap();
    assert_eq!(
        lock.assert_held().unwrap_err().to_string(),
        "controller lock path no longer identifies the owned inode"
    );
}
