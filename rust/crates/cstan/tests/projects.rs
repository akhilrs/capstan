//! Project discovery over fake process data and a fake probe, and the live probe over a test socket.
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use cstan_front::controllers::Proc;
use cstan_front::projects::{discover, Credential, EndpointState, Live, ProjectProbe};

const KEY: &str = "k3y-0123456789abcdef0123456789abcdef-secret";

struct Fake {
    keys: HashMap<PathBuf, &'static str>,
    states: HashMap<PathBuf, EndpointState>,
    asked: Mutex<Vec<PathBuf>>,
}

impl ProjectProbe for Fake {
    fn key(&self, root: &Path) -> Option<Credential> {
        self.keys.get(root).map(|k| Credential::new(*k))
    }
    fn ask(&self, root: &Path, _: &Credential) -> EndpointState {
        self.asked.lock().unwrap().push(root.to_path_buf());
        self.states
            .get(root)
            .copied()
            .unwrap_or(EndpointState::Silent)
    }
}

fn proc(pid: u32, root: &Path) -> Proc {
    Proc {
        pid,
        project: Some(root.to_path_buf()),
        project_gone: false,
        started_at: None,
        rss_bytes: None,
        cpu_percent: None,
    }
}

fn project(dir: &Path, name: &str) -> PathBuf {
    let root = dir.join(name);
    std::fs::create_dir_all(root.join(".capstan/state")).unwrap();
    std::fs::canonicalize(root).unwrap()
}

fn tmp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("cstan-projects-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::canonicalize(dir).unwrap()
}

fn fake(keys: &[&PathBuf], states: &[(&PathBuf, EndpointState)]) -> Fake {
    Fake {
        keys: keys.iter().map(|p| ((*p).clone(), KEY)).collect(),
        states: states.iter().map(|(p, s)| ((*p).clone(), *s)).collect(),
        asked: Mutex::new(Vec::new()),
    }
}

#[test]
fn one_endpoint_per_root_sorted() {
    let dir = tmp("sorted");
    let (a, b) = (project(&dir, "a"), project(&dir, "b"));
    let probe = fake(
        &[&a, &b],
        &[(&a, EndpointState::Ready), (&b, EndpointState::Ready)],
    );
    let found = discover(&[proc(9, &b), proc(7, &a), proc(5, &b)], &[], &probe);
    assert_eq!(found.len(), 2);
    assert_eq!(found[0].root, a);
    assert_eq!(found[1].root, b);
    assert_eq!(found[1].pid, Some(5));
    assert_eq!(found[0].socket, a.join(".capstan/state/control.sock"));
}

#[test]
fn extra_root_without_process_is_silent() {
    let dir = tmp("extra");
    let a = project(&dir, "a");
    let probe = fake(&[&a], &[]);
    let found = discover(&[], std::slice::from_ref(&a), &probe);
    assert_eq!(found[0].state, EndpointState::Silent);
    assert_eq!(found[0].pid, None);
}

#[test]
fn removed_root_is_gone() {
    let dir = tmp("gone");
    let a = project(&dir, "a");
    std::fs::remove_dir_all(&a).unwrap();
    let probe = fake(&[&a], &[]);
    let found = discover(&[proc(1, &a)], &[], &probe);
    assert_eq!(found[0].state, EndpointState::Gone);
    assert!(probe.asked.lock().unwrap().is_empty());
}

#[test]
fn no_key_means_no_call() {
    let dir = tmp("nokey");
    let a = project(&dir, "a");
    let probe = fake(&[], &[]);
    let found = discover(&[proc(1, &a)], &[], &probe);
    assert_eq!(found[0].state, EndpointState::NoKey);
    assert!(probe.asked.lock().unwrap().is_empty());
}

#[test]
fn probe_states_pass_through() {
    let dir = tmp("states");
    let (a, b) = (project(&dir, "a"), project(&dir, "b"));
    let probe = fake(
        &[&a, &b],
        &[
            (&a, EndpointState::Refused),
            (&b, EndpointState::Unsupported),
        ],
    );
    let found = discover(&[], &[a.clone(), b.clone()], &probe);
    assert_eq!(found[0].state, EndpointState::Refused);
    assert_eq!(found[1].state, EndpointState::Unsupported);
}

#[test]
fn symlinked_root_and_target_are_one() {
    let dir = tmp("link");
    let a = project(&dir, "a");
    let link = dir.join("link");
    std::os::unix::fs::symlink(&a, &link).unwrap();
    let probe = fake(&[&a], &[(&a, EndpointState::Ready)]);
    let found = discover(&[proc(3, &link)], std::slice::from_ref(&a), &probe);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].root, a);
    assert_eq!(found[0].pid, Some(3));
}

#[test]
fn debug_never_shows_the_key() {
    let dir = tmp("debug");
    let a = project(&dir, "a");
    let probe = fake(&[&a], &[(&a, EndpointState::Ready)]);
    let found = discover(&[proc(1, &a)], &[], &probe);
    assert!(!format!("{found:?}").contains(KEY));
    let key = Credential::new(KEY);
    assert!(!format!("{key:?}").contains(KEY));
    assert!(!format!("{key}").contains(KEY));
    assert_eq!(format!("{key:?}"), "<redacted>");
}

fn listing(dir: &Path) -> Vec<String> {
    fn walk(dir: &Path, out: &mut Vec<String>) {
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            out.push(entry.path().display().to_string());
            if entry.path().is_dir() {
                walk(&entry.path(), out);
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, &mut out);
    out.sort();
    out
}

/// Serves one JSON line per connection until the listener is dropped with the thread.
fn serve(root: &Path, answer: &'static str) -> std::thread::JoinHandle<()> {
    let listener = UnixListener::bind(root.join(".capstan/state/control.sock")).unwrap();
    std::thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            let mut line = String::new();
            let _ = BufReader::new(stream.try_clone().unwrap()).read_line(&mut line);
            let _ = writeln!(stream, "{answer}");
        }
    })
}

fn live_state(name: &str, answer: Option<&'static str>) -> EndpointState {
    let dir = tmp(name);
    let root = project(&dir, "p");
    let server = answer.map(|a| serve(&root, a));
    let found = Live.ask(&root, &Credential::new(KEY));
    if let Some(server) = server {
        server.join().unwrap();
    }
    found
}

#[test]
fn live_probe_reads_the_answer() {
    assert_eq!(
        live_state("ok", Some(r#"{"ok":true,"result":{"projectId":"x"}}"#)),
        EndpointState::Ready
    );
    assert_eq!(
        live_state(
            "unauth",
            Some(r#"{"ok":false,"code":"unauthorized","message":"no"}"#)
        ),
        EndpointState::Refused
    );
    assert_eq!(
        live_state(
            "unknown",
            Some(r#"{"ok":false,"code":"unknown_command","message":"no"}"#)
        ),
        EndpointState::Unsupported
    );
    assert_eq!(live_state("silent", None), EndpointState::Silent);
}

#[test]
fn discover_creates_nothing() {
    let dir = tmp("readonly");
    let a = project(&dir, "a");
    let before = listing(&dir);
    let found = discover(&[proc(1, &a)], &[], &Live);
    assert_eq!(found[0].state, EndpointState::NoKey);
    assert_eq!(listing(&dir), before);
}
