//! The hand-over to Node, with the real `cstan` binary: the Node CLI is a stub that records its arguments, environment and
//! stdin and exits as told, so every case checks what Node would have received.
use std::ffi::OsString;
use std::io::Write;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::RwLock;

const FRONT: &str = env!("CARGO_BIN_EXE_cstan");

/// Writing an executable while another test thread forks leaves the fork's child holding the write descriptor until it
/// execs, so exec'ing the file fails with ETXTBSY ("Text file busy"). Writers of executables hold this lock for
/// writing, and every spawn from the tests holds it for reading, so a fork never overlaps an open write descriptor.
static FORK_GUARD: RwLock<()> = RwLock::new(());

fn write_executable(path: &Path, contents: &str) {
    let _guard = FORK_GUARD.write().unwrap_or_else(|e| e.into_inner());
    std::fs::write(path, contents).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn copy_executable(from: &Path, to: &Path) {
    let _guard = FORK_GUARD.write().unwrap_or_else(|e| e.into_inner());
    std::fs::copy(from, to).unwrap();
}

struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Scratch {
        let path = std::env::temp_dir().join(format!("cstan-fb-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        Scratch(std::fs::canonicalize(path).unwrap())
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A Node stand-in: records argc, argv (NUL separated), the environment and stdin under `$STUB_OUT`, then exits with
/// `$STUB_EXIT` or kills itself with `$STUB_SIGNAL`.
fn write_stub(path: &Path) {
    write_executable(
        path,
        "#!/bin/sh\n\
         printf '%s\\n' \"$#\" > \"$STUB_OUT.argc\"\n\
         printf '%s\\0' \"$@\" > \"$STUB_OUT.argv\"\n\
         /usr/bin/env -0 > \"$STUB_OUT.env\"\n\
         cat > \"$STUB_OUT.stdin\"\n\
         [ -n \"$STUB_SIGNAL\" ] && kill -s \"$STUB_SIGNAL\" $$\n\
         exit \"${STUB_EXIT:-0}\"\n",
    );
}

struct Run {
    code: Option<i32>,
    signal: Option<i32>,
    stderr: Vec<u8>,
    stdout: Vec<u8>,
}

fn run(front: &Path, args: &[OsString], env: &[(&str, OsString)], stdin: &[u8], cwd: &Path) -> Run {
    let mut command = Command::new(front);
    command
        .args(args)
        .env_clear()
        .envs(env.iter().map(|(k, v)| (k, v)))
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = {
        let _guard = FORK_GUARD.read().unwrap_or_else(|e| e.into_inner());
        command.spawn().unwrap()
    };
    child.stdin.take().unwrap().write_all(stdin).unwrap();
    let output = child.wait_with_output().unwrap();
    Run {
        code: output.status.code(),
        signal: output.status.signal(),
        stderr: output.stderr,
        stdout: output.stdout,
    }
}

fn recorded(out: &Path, suffix: &str) -> Vec<u8> {
    std::fs::read(format!("{}.{suffix}", out.display())).unwrap_or_default()
}

fn os(text: &str) -> OsString {
    OsString::from(text)
}

fn split_nul(bytes: &[u8]) -> Vec<Vec<u8>> {
    let mut parts: Vec<Vec<u8>> = bytes.split(|b| *b == 0).map(<[u8]>::to_vec).collect();
    parts.pop();
    parts
}

/// The commands the front end never serves and the conditions under which it hands a served command over.
type Case = (&'static str, Vec<OsString>, Vec<(&'static str, OsString)>);

fn handed_over() -> Vec<Case> {
    let agent = |extra: &[(&'static str, OsString)]| {
        let mut env = vec![
            ("CAPSTAN_TOKEN", os("tok-0123456789abcdef0123456789abcdef")),
            ("CAPSTAN_SOCKET", os("/nonexistent/dir/control.sock")),
            ("CAPSTAN_AGENT_ID", os("developer-1")),
        ];
        env.extend_from_slice(extra);
        env
    };
    let none: Vec<(&'static str, OsString)> = Vec::new();
    vec![
        ("no arguments", vec![], none.clone()),
        ("version", vec![os("--version")], none.clone()),
        ("version word", vec![os("version")], none.clone()),
        ("help", vec![os("--help")], none.clone()),
        ("init", vec![os("init"), os("--git")], none.clone()),
        ("start", vec![os("start")], none.clone()),
        ("stop", vec![os("stop")], none.clone()),
        ("daemon", vec![os("daemon")], none.clone()),
        ("config", vec![os("config"), os("check")], none.clone()),
        ("herdr-config", vec![os("herdr-config")], none.clone()),
        ("dash", vec![os("dash"), os("--no-color")], agent(&[])),
        (
            "inspect",
            vec![os("inspect"), os("x"), os("--json")],
            agent(&[]),
        ),
        ("cancel", vec![os("cancel"), os("w-1")], agent(&[])),
        ("peek", vec![os("peek"), os("a")], agent(&[])),
        ("assign", vec![os("assign"), os("a")], agent(&[])),
        ("resolve", vec![os("resolve"), os("a")], agent(&[])),
        ("pm restart", vec![os("pm"), os("restart")], agent(&[])),
        ("launch", vec![os("launch")], agent(&[])),
        ("shutdown", vec![os("shutdown")], agent(&[])),
        (
            "restart helper",
            vec![os("__restart-helper"), os("x")],
            none.clone(),
        ),
        (
            "unknown command",
            vec![os("frobnicate"), os("--json")],
            agent(&[]),
        ),
        (
            "front version with arguments",
            vec![os("__front-version"), os("x")],
            none.clone(),
        ),
        (
            "status watch",
            vec![os("status"), os("--watch")],
            agent(&[]),
        ),
        ("status offline", vec![os("status")], none.clone()),
        ("status positional", vec![os("status"), os("x")], agent(&[])),
        ("ping positional", vec![os("ping"), os("x")], agent(&[])),
        ("ping without environment", vec![os("ping")], none.clone()),
        (
            "send without environment",
            vec![os("send"), os("pm-1"), os("x")],
            none.clone(),
        ),
        (
            "token only",
            vec![os("ping")],
            vec![("CAPSTAN_TOKEN", os("tok"))],
        ),
        (
            "socket only",
            vec![os("ping")],
            vec![("CAPSTAN_SOCKET", os("/s.sock"))],
        ),
        (
            "relative socket",
            vec![os("ping")],
            vec![
                ("CAPSTAN_TOKEN", os("tok")),
                ("CAPSTAN_SOCKET", os("rel/s.sock")),
            ],
        ),
        (
            "token with a space",
            vec![os("ping")],
            vec![
                ("CAPSTAN_TOKEN", os("to k")),
                ("CAPSTAN_SOCKET", os("/s.sock")),
            ],
        ),
        (
            "empty argument",
            vec![os("send"), os("pm-1"), os("")],
            agent(&[]),
        ),
        (
            "replacement character",
            vec![os("send"), os("pm-1"), os("a\u{fffd}b")],
            agent(&[]),
        ),
        (
            "argument that is not UTF-8",
            vec![
                os("send"),
                os("pm-1"),
                OsString::from_vec(vec![b'a', 0xff, 0xfe, b'b']),
            ],
            agent(&[]),
        ),
        (
            "oversize frame",
            vec![os("send"), os("pm-1"), os(&"x".repeat(70_000))],
            agent(&[]),
        ),
        ("no daemon", vec![os("ping")], agent(&[])),
        (
            "environment value that is not UTF-8",
            vec![os("ping")],
            vec![
                ("CAPSTAN_TOKEN", OsString::from_vec(vec![b't', 0xff])),
                ("CAPSTAN_SOCKET", os("/s.sock")),
            ],
        ),
        (
            "extra environment passes through",
            vec![os("start")],
            vec![("ODD_VALUE", os("a=b c\n\"quoted\"")), ("EMPTY", os(""))],
        ),
    ]
}

#[test]
fn node_receives_the_same_argv_and_environment_for_everything_handed_over() {
    let scratch = Scratch::new("hand");
    let stub = scratch.path().join("stub-node");
    write_stub(&stub);
    for (label, args, env) in handed_over() {
        let out = scratch.path().join("out");
        let mut full_env = env.clone();
        full_env.push(("CSTAN_NODE_CLI", stub.clone().into_os_string()));
        full_env.push(("STUB_OUT", out.clone().into_os_string()));
        full_env.push(("PATH", os("/usr/bin:/bin")));
        let result = run(
            Path::new(FRONT),
            &args,
            &full_env,
            b"typed text\n",
            scratch.path(),
        );
        assert_eq!(result.code, Some(0), "{label}: {:?}", result.stderr);
        let argv = split_nul(&recorded(&out, "argv"));
        let want: Vec<Vec<u8>> = args.iter().map(|a| a.as_bytes().to_vec()).collect();
        if !args.is_empty() {
            assert_eq!(argv, want, "{label}: argv");
        }
        assert_eq!(
            String::from_utf8_lossy(&recorded(&out, "argc")).trim(),
            args.len().to_string(),
            "{label}: argc"
        );
        let seen = split_nul(&recorded(&out, "env"));
        for (key, value) in &full_env {
            let mut want = key.as_bytes().to_vec();
            want.push(b'=');
            want.extend_from_slice(value.as_bytes());
            assert!(
                seen.contains(&want),
                "{label}: {key} was not passed unchanged"
            );
        }
        assert_eq!(recorded(&out, "stdin"), b"typed text\n", "{label}: stdin");
        for suffix in ["argc", "argv", "env", "stdin"] {
            let _ = std::fs::remove_file(format!("{}.{suffix}", out.display()));
        }
    }
}

#[test]
fn node_exit_codes_and_signals_pass_through() {
    let scratch = Scratch::new("exit");
    let stub = scratch.path().join("stub-node");
    write_stub(&stub);
    for code in [0, 2, 3, 4, 5, 7, 255] {
        let env = [
            ("CSTAN_NODE_CLI", stub.clone().into_os_string()),
            ("STUB_OUT", scratch.path().join("o").into_os_string()),
            ("STUB_EXIT", os(&code.to_string())),
            ("PATH", os("/usr/bin:/bin")),
        ];
        let result = run(Path::new(FRONT), &[os("init")], &env, b"", scratch.path());
        assert_eq!(result.code, Some(code), "exit {code}");
    }
    for (name, number) in [("TERM", 15), ("KILL", 9), ("HUP", 1)] {
        let env = [
            ("CSTAN_NODE_CLI", stub.clone().into_os_string()),
            ("STUB_OUT", scratch.path().join("o").into_os_string()),
            ("STUB_SIGNAL", os(name)),
            ("PATH", os("/usr/bin:/bin")),
        ];
        let result = run(Path::new(FRONT), &[os("init")], &env, b"", scratch.path());
        assert_eq!(result.signal, Some(number), "signal {name}");
        assert_eq!(result.code, None);
    }
}

#[test]
fn a_native_command_hands_over_without_double_sending_when_nothing_was_sent() {
    // The socket path exists as a plain file: connect fails, so nothing was sent and Node runs the command.
    let scratch = Scratch::new("notsent");
    let stub = scratch.path().join("stub-node");
    write_stub(&stub);
    let project = scratch.path().join("proj/.capstan/state");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::write(project.join("control.sock"), "").unwrap();
    let env = [
        ("CAPSTAN_TOKEN", os("tok-0123456789abcdef0123456789abcdef")),
        (
            "CAPSTAN_SOCKET",
            project.join("control.sock").into_os_string(),
        ),
        ("CSTAN_NODE_CLI", stub.clone().into_os_string()),
        ("STUB_OUT", scratch.path().join("o").into_os_string()),
        ("STUB_EXIT", os("4")),
        ("PATH", os("/usr/bin:/bin")),
    ];
    let result = run(
        Path::new(FRONT),
        &[os("send"), os("pm-1"), os("hello")],
        &env,
        b"",
        &scratch.path().join("proj"),
    );
    assert_eq!(result.code, Some(4));
    let argv = split_nul(&recorded(&scratch.path().join("o"), "argv"));
    assert_eq!(
        argv,
        vec![b"send".to_vec(), b"pm-1".to_vec(), b"hello".to_vec()]
    );
}

#[test]
fn node_is_found_beside_the_front_end_and_never_loops() {
    let scratch = Scratch::new("resolve");
    let bin = scratch.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    copy_executable(Path::new(FRONT), &bin.join("cstan"));
    let out = scratch.path().join("o");
    let base = [
        ("STUB_OUT", out.clone().into_os_string()),
        ("PATH", os("/usr/bin:/bin")),
    ];
    // Nothing beside it and nothing configured: the not-found error.
    let result = run(
        &bin.join("cstan"),
        &[os("init")],
        &base,
        b"",
        scratch.path(),
    );
    assert_eq!(result.code, Some(5));
    assert_eq!(
        String::from_utf8_lossy(&result.stderr),
        "cstan: the Node implementation was not found (install cstan-node beside cstan or set CSTAN_NODE_CLI)\n"
    );
    assert!(result.stdout.is_empty());
    // CSTAN_NODE_CLI naming the front end itself (directly or through a link) is skipped, not executed again.
    std::os::unix::fs::symlink(bin.join("cstan"), scratch.path().join("link")).unwrap();
    for target in [bin.join("cstan"), scratch.path().join("link")] {
        let mut env = base.to_vec();
        env.push(("CSTAN_NODE_CLI", target.into_os_string()));
        let result = run(&bin.join("cstan"), &[os("init")], &env, b"", scratch.path());
        assert_eq!(result.code, Some(5), "no exec loop");
    }
    // A relative CSTAN_NODE_CLI is ignored.
    let mut env = base.to_vec();
    env.push(("CSTAN_NODE_CLI", os("relative/node")));
    assert_eq!(
        run(&bin.join("cstan"), &[os("init")], &env, b"", scratch.path()).code,
        Some(5)
    );
    // cstan-node beside the front end (here reached through a link to the front end) is found.
    let stub = bin.join("cstan-node");
    write_stub(&stub);
    let result = run(
        &bin.join("cstan"),
        &[os("init"), os("a b")],
        &base,
        b"",
        scratch.path(),
    );
    assert_eq!(result.code, Some(0), "{:?}", result.stderr);
    assert_eq!(
        split_nul(&recorded(&out, "argv")),
        vec![b"init".to_vec(), b"a b".to_vec()]
    );
    // The sibling is looked for beside the real file, not beside a link to it.
    let elsewhere = scratch.path().join("elsewhere");
    std::fs::create_dir_all(&elsewhere).unwrap();
    std::os::unix::fs::symlink(bin.join("cstan"), elsewhere.join("cstan")).unwrap();
    let _ = std::fs::remove_file(format!("{}.argv", out.display()));
    let result = run(
        &elsewhere.join("cstan"),
        &[os("init")],
        &base,
        b"",
        scratch.path(),
    );
    assert_eq!(result.code, Some(0), "{:?}", result.stderr);
    // CSTAN_NODE_CLI wins over the sibling.
    let other = scratch.path().join("other-node");
    write_stub(&other);
    let mut env = base.to_vec();
    env.push(("CSTAN_NODE_CLI", other.clone().into_os_string()));
    env.push(("STUB_EXIT", os("3")));
    assert_eq!(
        run(&bin.join("cstan"), &[os("init")], &env, b"", scratch.path()).code,
        Some(3)
    );
}

#[test]
fn a_js_file_runs_under_node_from_cstan_node_or_the_path() {
    let scratch = Scratch::new("script");
    let fake_node = scratch.path().join("fake-node");
    write_executable(
        &fake_node,
        "#!/bin/sh\nprintf '%s\\0' \"$@\" > \"$STUB_OUT.argv\"\nexit 6\n",
    );
    let cli = scratch.path().join("cli.js");
    std::fs::write(&cli, "").unwrap();
    let out = scratch.path().join("o");
    for extension in ["cli.js", "cli.mjs"] {
        let script = scratch.path().join(extension);
        std::fs::write(&script, "").unwrap();
        let env = [
            ("CSTAN_NODE_CLI", script.clone().into_os_string()),
            ("CSTAN_NODE", fake_node.clone().into_os_string()),
            ("STUB_OUT", out.clone().into_os_string()),
            ("PATH", os("/usr/bin:/bin")),
        ];
        let result = run(
            Path::new(FRONT),
            &[os("init"), os("--git")],
            &env,
            b"",
            scratch.path(),
        );
        assert_eq!(result.code, Some(6));
        assert_eq!(
            split_nul(&recorded(&out, "argv")),
            vec![
                script.as_os_str().as_bytes().to_vec(),
                b"init".to_vec(),
                b"--git".to_vec()
            ]
        );
    }
    // Without CSTAN_NODE the first `node` on PATH is used.
    let path_dir = scratch.path().join("pathdir");
    std::fs::create_dir_all(&path_dir).unwrap();
    copy_executable(&fake_node, &path_dir.join("node"));
    let env = [
        ("CSTAN_NODE_CLI", cli.clone().into_os_string()),
        ("STUB_OUT", out.clone().into_os_string()),
        ("PATH", path_dir.clone().into_os_string()),
    ];
    let result = run(Path::new(FRONT), &[os("start")], &env, b"", scratch.path());
    assert_eq!(result.code, Some(6));
    assert_eq!(
        split_nul(&recorded(&out, "argv")),
        vec![cli.as_os_str().as_bytes().to_vec(), b"start".to_vec()]
    );
}

#[test]
fn the_front_version_is_printed_for_the_installer() {
    let _guard = FORK_GUARD.read().unwrap_or_else(|e| e.into_inner());
    let result = Command::new(FRONT)
        .arg("__front-version")
        .env_clear()
        .output()
        .unwrap();
    assert_eq!(result.status.code(), Some(0));
    assert_eq!(
        String::from_utf8_lossy(&result.stdout),
        format!("cstan-front {}\n", cstan_front::VERSION)
    );
    let package = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../package.json"),
    )
    .unwrap();
    let version = package
        .split("\"version\":")
        .nth(1)
        .unwrap()
        .split('"')
        .nth(1)
        .unwrap();
    if std::env::var_os("CSTAN_VERSION").is_none() {
        assert_eq!(cstan_front::VERSION, version);
    }
    assert!(!cstan_front::VERSION.is_empty());
}
