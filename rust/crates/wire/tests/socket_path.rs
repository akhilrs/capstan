use std::fs;
use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::Duration;

use capstan_wire::*;

fn tmp() -> tempdir::T {
    tempdir::T::new()
}

mod tempdir {
    use std::path::{Path, PathBuf};
    use std::sync::{Mutex, MutexGuard};

    /// Held for a test's whole lifetime so a test that changes the process
    /// environment never overlaps another test in this binary.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    pub struct T(PathBuf, #[allow(dead_code)] MutexGuard<'static, ()>);
    impl T {
        pub fn new() -> T {
            static COUNT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
            let guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            let n = COUNT.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let p = std::env::temp_dir().join(format!("cw-sp-{}-{n}", std::process::id()));
            std::fs::create_dir_all(&p).unwrap();
            T(std::fs::canonicalize(p).unwrap(), guard)
        }
        pub fn path(&self) -> &Path {
            &self.0
        }
    }
    impl Drop for T {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}

/// A project root whose `<root>/.capstan/state/control.sock` is exactly `total` bytes (state not created).
fn project_with(root: &Path, total: usize) -> PathBuf {
    let suffix = "/.capstan/state/control.sock".len();
    let base = root.join("p").as_os_str().len();
    let pad = total - suffix - base;
    let dir = root.join(format!("p{}", "x".repeat(pad)));
    fs::create_dir_all(dir.join(".capstan")).unwrap();
    dir.join(".capstan/state")
}

#[test]
fn boundary_and_missing_state() {
    let t = tmp();
    let base = t.path().join("base");
    let limit = socket_path_limit();
    let at = project_with(&t.path().join("a"), limit);
    assert_eq!(control_socket_in(&at, &base), at.join(SOCKET_NAME));
    let over = project_with(&t.path().join("b"), limit + 1);
    let r = control_socket_in(&over, &base);
    let name = r.file_name().unwrap().to_str().unwrap();
    assert_eq!(r.parent().unwrap(), base.join(runtime_dir_name()));
    assert!(name.ends_with(".sock") && name.len() == 32 + 5);
    assert!(name[..32]
        .bytes()
        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)));
    assert!(r.as_os_str().len() <= limit);
    fs::create_dir_all(&over).unwrap();
    assert_eq!(control_socket_in(&over, &base), r);
}

#[test]
fn deterministic() {
    let t = tmp();
    let base = t.path().join("base");
    let limit = socket_path_limit();
    let a = project_with(&t.path().join("a"), limit + 5);
    let b = project_with(&t.path().join("b"), limit + 5);
    assert_ne!(control_socket_in(&a, &base), control_socket_in(&b, &base));
    let link = t.path().join("link");
    symlink(t.path().join("a"), &link).unwrap();
    let via = link.join(a.strip_prefix(t.path().join("a")).unwrap());
    assert_eq!(control_socket_in(&via, &base), control_socket_in(&a, &base));
    let before = control_socket_in(&a, &base);
    let keys = ["HOME", "TMPDIR", "XDG_RUNTIME_DIR"];
    let saved: Vec<_> = keys.iter().map(std::env::var_os).collect();
    for k in keys {
        std::env::set_var(k, "/nonexistent");
    }
    let after = control_socket_in(&a, &base);
    for (k, v) in keys.iter().zip(saved) {
        match v {
            Some(v) => std::env::set_var(k, v),
            None => std::env::remove_var(k),
        }
    }
    assert_eq!(after, before);
}

#[test]
fn private_dirs() {
    let t = tmp();
    let d = t.path().join("d");
    ensure_private_dir(&d).unwrap();
    assert_eq!(
        fs::metadata(&d).unwrap().permissions().mode() & 0o777,
        0o700
    );
    ensure_private_dir(&d).unwrap();
    let open = t.path().join("open");
    fs::create_dir(&open).unwrap();
    fs::set_permissions(&open, fs::Permissions::from_mode(0o755)).unwrap();
    let e = ensure_private_dir(&open).unwrap_err();
    assert!(e.contains(&open.display().to_string()));
    assert_eq!(
        fs::metadata(&open).unwrap().permissions().mode() & 0o777,
        0o755
    );
    let link = t.path().join("l");
    symlink(&d, &link).unwrap();
    assert!(ensure_private_dir(&link)
        .unwrap_err()
        .contains(&link.display().to_string()));
    let file = t.path().join("f");
    fs::write(&file, "").unwrap();
    assert!(ensure_private_dir(&file)
        .unwrap_err()
        .contains(&file.display().to_string()));
}

#[test]
fn matching() {
    let t = tmp();
    let limit = socket_path_limit();
    let long = project_with(&t.path().join("a"), limit + 3);
    let other = project_with(&t.path().join("b"), limit + 3);
    let resolved = control_socket(&long);
    assert!(socket_matches(&long, &resolved));
    assert!(!socket_matches(&long, &long.join(SOCKET_NAME)));
    assert!(!socket_matches(&long, &control_socket(&other)));
    let link = t.path().join("link");
    symlink(t.path().join("a"), &link).unwrap();
    let via = link.join(long.strip_prefix(t.path().join("a")).unwrap());
    assert!(socket_matches(&via, &resolved));
}

#[test]
fn call_refuses_an_open_runtime_directory() {
    let t = tmp();
    let base = t.path().join("base");
    fs::create_dir(&base).unwrap();
    let run = base.join(runtime_dir_name());
    fs::create_dir(&run).unwrap();
    fs::set_permissions(&run, fs::Permissions::from_mode(0o755)).unwrap();
    let sock = run.join("x.sock");
    let listener = std::os::unix::net::UnixListener::bind(&sock).unwrap();
    listener.set_nonblocking(true).unwrap();
    let e = call_in(&base, &sock, b"{}\n", Timeout::Idle(Duration::from_secs(1))).unwrap_err();
    assert!(
        matches!(&e, WireError::NotSent { kind: std::io::ErrorKind::PermissionDenied, code } if code == "EUNSAFE"),
        "{e:?}"
    );
    assert!(listener.accept().is_err());
}
