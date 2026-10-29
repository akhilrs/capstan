//! Project discovery over fake process data and a fake probe, and the live probe over a test socket.
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use cstan_front::controllers::Proc;
use cstan_front::projects::{
    call_within, classify, discover, helper_threads, project_id, Credential, EndpointState, Live,
    ProjectProbe,
};

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
    assert_eq!(
        live_state(
            "forbidden",
            Some(r#"{"ok":false,"code":"forbidden","message":"no"}"#)
        ),
        EndpointState::Refused
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

#[test]
fn classify_maps_codes() {
    assert_eq!(classify("unauthorized"), EndpointState::Refused);
    assert_eq!(classify("forbidden"), EndpointState::Refused);
    assert_eq!(classify("unknown_command"), EndpointState::Unsupported);
    assert_eq!(classify("error"), EndpointState::Silent);
    assert_eq!(classify(""), EndpointState::Silent);
}

#[test]
fn project_id_reads_the_config() {
    let dir = tmp("pid");
    let a = project(&dir, "a");
    assert_eq!(project_id(&a), None);
    std::fs::write(a.join(".capstan/project.json"), r#"{"projectId":"proj-a"}"#).unwrap();
    assert_eq!(project_id(&a).as_deref(), Some("proj-a"));
    std::fs::write(a.join(".capstan/project.json"), "{").unwrap();
    assert_eq!(project_id(&a), None);
}

struct Rendezvous(std::sync::Barrier);

impl ProjectProbe for Rendezvous {
    fn key(&self, _: &Path) -> Option<Credential> {
        Some(Credential::new(KEY))
    }
    fn ask(&self, _: &Path, _: &Credential) -> EndpointState {
        // Every probe waits for all the others; this passes only when they run at the same time.
        self.0.wait();
        EndpointState::Ready
    }
}

#[test]
fn discover_probes_roots_in_parallel() {
    let dir = tmp("parallel");
    let roots: Vec<PathBuf> = (0..64)
        .map(|i| project(&dir, &format!("p{i:02}")))
        .collect();
    let probe = std::sync::Arc::new(Rendezvous(std::sync::Barrier::new(64)));
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(discover(&[], &roots, probe.as_ref()).len());
    });
    // A serial discovery would wait on the barrier for ever; the timeout turns that into a failure.
    assert_eq!(rx.recv_timeout(std::time::Duration::from_secs(10)), Ok(64));
}

#[test]
fn call_within_ends_a_trickling_peer_at_the_total_deadline() {
    let dir = tmp("trickle");
    let root = project(&dir, "p");
    let listener = UnixListener::bind(root.join(".capstan/state/control.sock")).unwrap();
    std::thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            let mut line = String::new();
            let _ = BufReader::new(stream.try_clone().unwrap()).read_line(&mut line);
            for _ in 0..12 {
                if stream.write_all(b" ").is_err() {
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(500));
            }
        }
    });
    let started = std::time::Instant::now();
    let answer = call_within(&root, &Credential::new(KEY), &["head".to_string()], 2_000);
    assert!(answer.is_none());
    assert!(started.elapsed() < std::time::Duration::from_millis(2_500));
}

#[test]
fn a_trickling_peer_across_many_polls_leaves_one_helper_thread() {
    let dir = tmp("flight");
    let root = project(&dir, "p");
    let listener = UnixListener::bind(root.join(".capstan/state/control.sock")).unwrap();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut stream = stream;
            std::thread::spawn(move || {
                let mut line = String::new();
                let _ = BufReader::new(stream.try_clone().unwrap()).read_line(&mut line);
                for _ in 0..40 {
                    if stream.write_all(b" ").is_err() {
                        return;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(25));
                }
            });
        }
    });
    let key = Credential::new(KEY);
    let mut most = 0;
    for _ in 0..10 {
        assert!(call_within(&root, &key, &["head".to_string()], 100).is_none());
        most = most.max(helper_threads(&root));
    }
    assert_eq!(most, 1);
}

#[test]
fn many_sequential_calls_to_a_fast_peer_all_answer() {
    let dir = tmp("sequential");
    let root = project(&dir, "p");
    let listener = UnixListener::bind(root.join(".capstan/state/control.sock")).unwrap();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut stream = stream;
            let mut line = String::new();
            let _ = BufReader::new(stream.try_clone().unwrap()).read_line(&mut line);
            let _ = writeln!(stream, r#"{{"ok":true,"result":{{"projectId":"x"}}}}"#);
        }
    });
    let key = Credential::new(KEY);
    for i in 0..200 {
        let got = call_within(&root, &key, &["head".to_string()], 2_000);
        assert!(matches!(got, Some(Ok(_))), "call {i}: {got:?}");
    }
}
