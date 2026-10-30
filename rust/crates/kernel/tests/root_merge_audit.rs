//! The audit of a merge into the root checkout: the command list, programs a repository's config could name, the
//! literals of every refusal and failure, the pins of every child, and the repositories a merge refuses. Real git in
//! temporary repositories.

use capstan_config::remote::{DOC_FAILURE_REASONS, DOC_REFUSAL_CODES};
use capstan_kernel::integrate::{
    GitRepo, GitSeams, Preflight, RootMerge, MERGE_AUDITED_COMMANDS, RUN_MERGE_SUBCOMMANDS,
};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

const LONG: Duration = Duration::from_secs(60);

fn forward(tmp: &Path) -> Vec<(String, String)> {
    let home = tmp.join("home");
    std::fs::create_dir_all(&home).unwrap();
    vec![
        ("HOME".into(), home.to_string_lossy().into_owned()),
        ("GIT_CONFIG_NOSYSTEM".into(), "1".into()),
    ]
}

fn git_out(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn git(dir: &Path, args: &[&str]) {
    git_out(dir, args);
}

fn write(dir: &Path, name: &str, text: &str) {
    if let Some(parent) = dir.join(name).parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(dir.join(name), text).unwrap();
}

fn commit_all(dir: &Path, message: &str) {
    git(dir, &["add", "-A"]);
    git(
        dir,
        &[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "-q",
            "-m",
            message,
        ],
    );
}

/// Writes an executable shell script.
fn script(path: &Path, body: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
    let status = Command::new("chmod").arg("755").arg(path).status().unwrap();
    assert!(status.success());
}

/// The directory holding the host's git.
fn real_git_dir() -> PathBuf {
    let path = std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin".into());
    std::env::split_paths(&path)
        .find(|dir| dir.join("git").is_file())
        .expect("git was not found on PATH")
}

fn git_version() -> String {
    let out = Command::new("git").arg("--version").output().unwrap();
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

struct Fx {
    tmp: tempfile::TempDir,
    root: PathBuf,
}

impl Fx {
    /// A repository on `main` with one commit and a local identity. The `.gitattributes` names the filter, diff and merge
    /// attributes the cases below use.
    fn new() -> Fx {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("root");
        std::fs::create_dir_all(&root).unwrap();
        git(&root, &["init", "-q", "-b", "main"]);
        git(&root, &["config", "--local", "user.name", "T"]);
        git(&root, &["config", "--local", "user.email", "t@t"]);
        write(&root, "a.txt", "base\n");
        write(&root, "b.txt", "keep\n");
        write(&root, ".gitattributes", "*.txt diff=x\n");
        commit_all(&root, "base");
        Fx { tmp, root }
    }

    fn seams(&self, git_program: Option<PathBuf>, child_path: Option<String>) -> GitSeams {
        GitSeams {
            forward_env: Some(forward(self.tmp.path())),
            git_program,
            child_path,
            skip_version_probe: true,
            ..GitSeams::default()
        }
    }

    fn repo(&self) -> GitRepo {
        GitRepo::new(&self.root).with_seams(self.seams(None, None))
    }

    fn sha(&self, rev: &str) -> String {
        git_out(&self.root, &["rev-parse", rev])
    }

    /// A side branch adding `s.txt`, then one more commit on main adding `m.txt`: (side sha, old head).
    fn diverged(&self) -> (String, String) {
        git(&self.root, &["checkout", "-q", "-b", "side"]);
        write(&self.root, "s.txt", "side\n");
        commit_all(&self.root, "side");
        let side = self.sha("HEAD");
        git(&self.root, &["checkout", "-q", "main"]);
        write(&self.root, "m.txt", "main\n");
        commit_all(&self.root, "main work");
        (side, self.sha("HEAD"))
    }
}

// ------------------------------------------------------------------------------------------------ the command list

#[test]
fn run_merge_subcommands_are_audited() {
    assert!(!RUN_MERGE_SUBCOMMANDS.is_empty());
    for sub in RUN_MERGE_SUBCOMMANDS {
        assert!(
            MERGE_AUDITED_COMMANDS.contains(sub),
            "the subcommand {sub} is in RUN_MERGE_SUBCOMMANDS and missing from MERGE_AUDITED_COMMANDS"
        );
    }
}

// ------------------------------------------------------------------------------------------------ exec keys

#[test]
fn a_merge_with_sentinel_exec_keys_runs_none_of_their_programs() {
    let fx = Fx::new();
    let (side, old) = fx.diverged();

    // Everything outside the root, so no file of it makes the tree dirty.
    let tools = fx.tmp.path().join("tools");
    let marks = fx.tmp.path().join("marks");
    std::fs::create_dir_all(&marks).unwrap();
    let marker = |name: &str| format!("touch '{}'", marks.join(name).display());
    let names = [
        "external",
        "editor",
        "gpg",
        "fsmonitor",
        "textconv",
        "submodule",
    ];
    for name in names {
        script(&tools.join(name), &marker(name));
    }
    let hooks = fx.tmp.path().join("hooks");
    for hook in [
        "applypatch-msg",
        "pre-applypatch",
        "post-applypatch",
        "pre-commit",
        "pre-merge-commit",
        "prepare-commit-msg",
        "commit-msg",
        "post-commit",
        "pre-rebase",
        "post-checkout",
        "post-merge",
        "pre-push",
        "pre-auto-gc",
        "post-rewrite",
        "reference-transaction",
        "fsmonitor-watchman",
    ] {
        script(&hooks.join(hook), &marker(&format!("hook-{hook}")));
    }
    let tool = |name: &str| tools.join(name).to_string_lossy().into_owned();
    for (key, value) in [
        ("diff.external", tool("external")),
        ("core.editor", tool("editor")),
        ("gpg.program", tool("gpg")),
        ("commit.gpgsign", "true".to_string()),
        ("core.fsmonitor", tool("fsmonitor")),
        ("core.hooksPath", hooks.to_string_lossy().into_owned()),
        ("diff.x.textconv", tool("textconv")),
        ("submodule.x.update", format!("!{}", tool("submodule"))),
    ] {
        git(&fx.root, &["config", "--local", key, &value]);
    }

    let result = fx
        .repo()
        .merge_into_root("main", "side", &side, &old, LONG)
        .unwrap();
    match &result {
        RootMerge::Merged { commit } => assert_eq!(&fx.sha("main"), commit),
        other => panic!("expected a merge, got {other:?}"),
    }
    let ran: Vec<String> = std::fs::read_dir(&marks)
        .unwrap()
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    assert!(ran.is_empty(), "programs ran: {ran:?}");
}

// ------------------------------------------------------------------------------------------------ the literals

/// `text` with comments blanked and the inside of string and character literals turned to `x`, same length, so offsets
/// carry over to `text`.
fn mask(text: &str) -> Vec<u8> {
    let src = text.as_bytes();
    let mut out = src.to_vec();
    let mut i = 0;
    while i < src.len() {
        match src[i] {
            b'/' if src.get(i + 1) == Some(&b'/') => {
                while i < src.len() && src[i] != b'\n' {
                    out[i] = b' ';
                    i += 1;
                }
            }
            b'/' if src.get(i + 1) == Some(&b'*') => {
                let mut depth = 0;
                while i < src.len() {
                    if src[i] == b'/' && src.get(i + 1) == Some(&b'*') {
                        depth += 1;
                        out[i] = b' ';
                        out[i + 1] = b' ';
                        i += 2;
                    } else if src[i] == b'*' && src.get(i + 1) == Some(&b'/') {
                        depth -= 1;
                        out[i] = b' ';
                        out[i + 1] = b' ';
                        i += 2;
                        if depth == 0 {
                            break;
                        }
                    } else {
                        if src[i] != b'\n' {
                            out[i] = b' ';
                        }
                        i += 1;
                    }
                }
            }
            b'r' if matches!(src.get(i + 1), Some(b'"') | Some(b'#'))
                && (i == 0 || !(src[i - 1].is_ascii_alphanumeric() || src[i - 1] == b'_')) =>
            {
                let mut j = i + 1;
                let mut hashes = 0;
                while src.get(j) == Some(&b'#') {
                    hashes += 1;
                    j += 1;
                }
                if src.get(j) != Some(&b'"') {
                    i += 1;
                    continue;
                }
                j += 1;
                loop {
                    if j >= src.len() {
                        break;
                    }
                    if src[j] == b'"' && src[j + 1..].iter().take(hashes).all(|b| *b == b'#') {
                        j += 1 + hashes;
                        break;
                    }
                    if src[j] != b'\n' {
                        out[j] = b'x';
                    }
                    j += 1;
                }
                i = j;
            }
            b'"' => {
                i += 1;
                while i < src.len() && src[i] != b'"' {
                    if src[i] == b'\\' {
                        out[i] = b'x';
                        i += 1;
                    }
                    if i < src.len() && src[i] != b'\n' {
                        out[i] = b'x';
                    }
                    i += 1;
                }
                i += 1;
            }
            b'\'' => {
                // A character literal ('x', '\n', '\'') and not a lifetime.
                let close = if src.get(i + 1) == Some(&b'\\') {
                    src[i + 2..]
                        .iter()
                        .position(|b| *b == b'\'')
                        .map(|p| i + 2 + p)
                } else if src.get(i + 2) == Some(&b'\'') {
                    Some(i + 2)
                } else {
                    None
                };
                match close {
                    Some(end) => {
                        for byte in &mut out[i + 1..end] {
                            *byte = b'x';
                        }
                        i = end + 1;
                    }
                    None => i += 1,
                }
            }
            _ => i += 1,
        }
    }
    out
}

/// The offset of the brace that closes the one at `open`, in masked text.
fn close_of(masked: &[u8], open: usize) -> usize {
    assert_eq!(masked[open], b'{');
    let mut depth = 0;
    for (at, byte) in masked.iter().enumerate().skip(open) {
        match byte {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return at;
                }
            }
            _ => {}
        }
    }
    panic!("an unbalanced brace in integrate.rs");
}

fn find_all(haystack: &[u8], needle: &str) -> Vec<usize> {
    let needle = needle.as_bytes();
    (0..haystack.len().saturating_sub(needle.len() - 1))
        .filter(|at| &haystack[*at..*at + needle.len()] == needle)
        .collect()
}

/// The `[start, end)` of the item that follows `start`: up to the matching brace, or to a `;` that comes first.
fn item_end(masked: &[u8], start: usize) -> usize {
    let open = masked[start..]
        .iter()
        .position(|byte| *byte == b'{' || *byte == b';')
        .map(|p| start + p)
        .expect("an item without a body");
    if masked[open] == b';' {
        open + 1
    } else {
        close_of(masked, open) + 1
    }
}

/// Every first argument passed to the constructors of a refusal or a failure in integrate.rs: (kind, literal or the
/// expression found, offset). A literal is returned without its quotes; anything else is an `Err` with its text.
#[allow(clippy::type_complexity)]
fn constructor_arguments(text: &str) -> Vec<(&'static str, Result<String, String>, usize)> {
    let masked = mask(text);
    let masked_text = std::str::from_utf8(&masked).unwrap();
    let mut skipped: Vec<(usize, usize)> = Vec::new();
    // #[cfg(test)] modules (and items) to the end of the item.
    for at in find_all(&masked, "#[cfg(test)]") {
        skipped.push((at, item_end(&masked, at + "#[cfg(test)]".len())));
    }
    // The helpers that only pass their parameter on; their callers are scanned instead.
    for name in ["failed_from", "refused_merge", "refused"] {
        let head = format!("fn {name}(");
        for at in find_all(&masked, &head) {
            skipped.push((at, item_end(&masked, at)));
        }
    }
    let pattern =
        regex::Regex::new(r"(?:\b(Refusal::new|RootMerge::failed|refused_merge|refused)\()\s*")
            .unwrap();
    let mut found = Vec::new();
    for caps in pattern.captures_iter(masked_text) {
        let whole = caps.get(0).unwrap();
        let start = whole.start();
        if masked_text[..start].ends_with("fn ")
            || (masked_text[..start].ends_with("::") && &caps[1] == "refused")
        {
            continue;
        }
        if skipped
            .iter()
            .any(|(from, to)| start >= *from && start < *to)
        {
            continue;
        }
        let kind = match &caps[1] {
            "RootMerge::failed" => "failure",
            _ => "refusal",
        };
        let rest = &text[whole.end()..];
        let argument = if let Some(inner) = rest.strip_prefix('"') {
            let end = inner.find('"').expect("an unterminated literal");
            Ok(inner[..end].to_string())
        } else {
            let end = rest.find([',', ')']).unwrap_or(rest.len());
            Err(rest[..end].trim().to_string())
        };
        found.push((kind, argument, start));
    }
    found
}

#[test]
fn every_refusal_code_and_failure_reason_in_the_merge_is_documented() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/integrate.rs");
    let text = std::fs::read_to_string(&path).unwrap();
    let found = constructor_arguments(&text);
    let line = |at: usize| text[..at].matches('\n').count() + 1;

    let (mut refusals, mut failures) = (0, 0);
    for (kind, argument, at) in &found {
        let literal = match argument {
            Ok(literal) => literal,
            Err(expression) => panic!(
                "integrate.rs line {}: the first argument must be a string literal, found `{expression}`",
                line(*at)
            ),
        };
        if *kind == "refusal" {
            refusals += 1;
            assert!(
                DOC_REFUSAL_CODES.contains(&literal.as_str()),
                "integrate.rs line {}: \"{literal}\" is not in DOC_REFUSAL_CODES",
                line(*at)
            );
        } else {
            failures += 1;
            assert!(
                DOC_FAILURE_REASONS.contains(&literal.as_str()),
                "integrate.rs line {}: \"{literal}\" is not in DOC_FAILURE_REASONS",
                line(*at)
            );
        }
    }
    // A scan that finds nothing proves nothing.
    assert!(refusals >= 10, "only {refusals} refusal sites found");
    assert!(failures >= 3, "only {failures} failure sites found");
}

#[test]
fn the_scan_rejects_what_it_cannot_read() {
    let source = r##"
fn ok() { Refusal::new(
    "dirty_tree", v); RootMerge::failed( "timeout", true); }
fn bad(code: &'static str) { RootMerge::failed(code, true); }
fn refused(code: &'static str) -> P { Refusal::new(code, vec![]) }
fn failed_from(d: D) -> R { RootMerge::failed(reason, true) }
fn call() { refused("wrong_branch", &[]); refused_merge("invalid_branch", &["{"]); }
#[cfg(test)]
mod tests { fn t() { Refusal::new(nonsense, vec![]); let c = '{'; let s = "}"; } }
fn after() { Refusal::new("merge_conflict", p); }
"##;
    let found = constructor_arguments(source);
    let shown: Vec<(&str, Result<String, String>)> =
        found.into_iter().map(|(k, a, _)| (k, a)).collect();
    assert_eq!(
        shown,
        vec![
            ("refusal", Ok("dirty_tree".to_string())),
            ("failure", Ok("timeout".to_string())),
            ("failure", Err("code".to_string())),
            ("refusal", Ok("wrong_branch".to_string())),
            ("refusal", Ok("invalid_branch".to_string())),
            ("refusal", Ok("merge_conflict".to_string())),
        ]
    );
}

// ------------------------------------------------------------------------------------------------ the pins

/// One logged git child: its arguments and its environment.
struct Call {
    args: Vec<String>,
    env: Vec<String>,
}

impl Call {
    /// The subcommand: the first argument after the global ones (`--no-replace-objects`, `-C dir`, `-c k=v`).
    fn sub(&self) -> &str {
        let mut i = 0;
        while i < self.args.len() {
            match self.args[i].as_str() {
                "-C" | "-c" => i += 2,
                "--no-replace-objects" => i += 1,
                other => return other,
            }
        }
        ""
    }

    fn pinned(&self, key_value: &str) -> bool {
        self.args
            .windows(2)
            .any(|pair| pair[0] == "-c" && pair[1] == key_value)
    }

    fn has_env(&self, entry: &str) -> bool {
        self.env.iter().any(|line| line == entry)
    }

    fn describe(&self) -> String {
        self.args.join(" ")
    }
}

/// A fake git that logs argv and the environment of every call and runs the real git.
fn logging_git(tmp: &Path) -> (PathBuf, PathBuf) {
    let log = tmp.join("calls.log");
    let program = tmp.join("logging-git");
    let real = real_git_dir().join("git");
    script(
        &program,
        &format!(
            r#"{{ echo "@@CALL"; for a in "$@"; do echo "ARG $a"; done; env | sed 's/^/ENV /'; }} >> '{log}'
exec '{real}' "$@""#,
            log = log.display(),
            real = real.display()
        ),
    );
    (program, log)
}

fn read_calls(log: &Path) -> Vec<Call> {
    let text = std::fs::read_to_string(log).unwrap();
    let mut calls: Vec<Call> = Vec::new();
    for line in text.lines() {
        if line == "@@CALL" {
            calls.push(Call {
                args: Vec::new(),
                env: Vec::new(),
            });
        } else if let (Some(call), Some(arg)) = (calls.last_mut(), line.strip_prefix("ARG ")) {
            call.args.push(arg.to_string());
        } else if let (Some(call), Some(entry)) = (calls.last_mut(), line.strip_prefix("ENV ")) {
            call.env.push(entry.to_string());
        }
    }
    calls
}

#[test]
fn every_child_of_a_full_merge_carries_its_pins() {
    let fx = Fx::new();
    let (side, old) = fx.diverged();
    let (program, log) = logging_git(fx.tmp.path());
    let repo = GitRepo::new(&fx.root).with_seams(fx.seams(Some(program), None));

    let result = repo
        .merge_into_root("main", "side", &side, &old, LONG)
        .unwrap();
    assert!(matches!(result, RootMerge::Merged { .. }), "{result:?}");

    let calls = read_calls(&log);
    assert!(!calls.is_empty());
    let mut seen = std::collections::BTreeSet::new();
    for call in &calls {
        let sub = call.sub();
        seen.insert(sub.to_string());
        // identity_value reads the user's global identity by design.
        if sub == "config" && call.args.iter().any(|arg| arg == "--get-all") {
            continue;
        }
        let what = call.describe();
        assert!(
            RUN_MERGE_SUBCOMMANDS.contains(&sub),
            "the child `git {what}` is not an audited subcommand"
        );
        for pin in [
            "commit.gpgSign=false",
            "core.fsmonitor=false",
            "core.hooksPath=/dev/null",
        ] {
            assert!(call.pinned(pin), "`git {what}` lacks the pin {pin}");
        }
        assert!(
            call.has_env("GIT_CONFIG_GLOBAL=/dev/null"),
            "`git {what}` lacks GIT_CONFIG_GLOBAL=/dev/null"
        );
        if sub == "commit-tree" {
            assert!(call.has_env("TZ=UTC"), "`git {what}` lacks TZ=UTC");
        } else {
            assert!(
                !call.env.iter().any(|line| line.starts_with("TZ=")),
                "`git {what}` carries TZ"
            );
        }
    }
    for sub in ["read-tree", "commit-tree", "update-ref"] {
        assert!(seen.contains(sub), "no {sub} child was logged: {seen:?}");
    }
}

#[test]
fn the_reads_of_step_one_carry_the_global_config_pin() {
    let fx = Fx::new();
    let (side, _) = fx.diverged();
    let (program, log) = logging_git(fx.tmp.path());
    let repo = GitRepo::new(&fx.root).with_seams(fx.seams(Some(program), None));

    let result = repo.root_merge_preflight("main", &side, LONG).unwrap();
    assert!(matches!(result, Preflight::Ready { .. }), "{result:?}");

    let reads: Vec<Call> = read_calls(&log)
        .into_iter()
        .filter(|call| {
            call.args.iter().any(|arg| {
                matches!(
                    arg.as_str(),
                    "--local" | "--worktree" | "--git-dir" | "--git-common-dir"
                )
            })
        })
        .collect();
    assert!(!reads.is_empty(), "no configuration read was logged");
    for call in &reads {
        assert!(
            call.has_env("GIT_CONFIG_GLOBAL=/dev/null"),
            "`git {}` lacks GIT_CONFIG_GLOBAL=/dev/null",
            call.describe()
        );
    }
}

// ------------------------------------------------------------------------------------------------ refused repositories

#[test]
fn a_repository_with_git_lfs_is_refused_before_any_program_runs() {
    let fx = Fx::new();
    let (side, old) = fx.diverged();
    let lfs_dir = fx.tmp.path().join("lfs-bin");
    let marker = fx.tmp.path().join("lfs-ran");
    script(
        &lfs_dir.join("git-lfs"),
        &format!("touch '{}'", marker.display()),
    );
    // The fake comes first, so a real git-lfs on the host never shadows it.
    let child_path = format!(
        "{}:{}:/usr/bin:/bin",
        lfs_dir.display(),
        real_git_dir().display()
    );
    // What `git lfs install --local` writes.
    for (key, value) in [
        ("filter.lfs.clean", "git-lfs clean -- %f"),
        ("filter.lfs.smudge", "git-lfs smudge -- %f"),
        ("filter.lfs.process", "git-lfs filter-process"),
        ("filter.lfs.required", "true"),
    ] {
        git(&fx.root, &["config", "--local", key, value]);
    }
    let repo = GitRepo::new(&fx.root).with_seams(fx.seams(None, Some(child_path)));

    let files_of = |refusal: &capstan_kernel::integrate::Refusal| {
        assert_eq!(refusal.code(), "unsafe_config");
        refusal.files().to_vec()
    };
    let preflight = repo.root_merge_preflight("main", &side, LONG).unwrap();
    let Preflight::Refused(refusal) = preflight else {
        panic!("expected a refusal, got {preflight:?}");
    };
    let files = files_of(&refusal);
    for key in [
        "filter.lfs.clean",
        "filter.lfs.smudge",
        "filter.lfs.process",
    ] {
        assert!(
            files.iter().any(|file| file.eq_ignore_ascii_case(key)),
            "{key} is not named in {files:?}"
        );
    }

    let merge = repo
        .merge_into_root("main", "side", &side, &old, LONG)
        .unwrap();
    let RootMerge::Refused(refusal) = merge else {
        panic!("expected a refusal, got {merge:?}");
    };
    assert_eq!(files_of(&refusal), files);
    assert!(!marker.exists(), "a git-lfs program ran");
    assert_eq!(fx.sha("main"), old, "the branch moved");
}

#[test]
fn a_sha256_repository_is_refused_as_an_invalid_commit() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("root");
    std::fs::create_dir_all(&root).unwrap();
    let init = Command::new("git")
        .arg("-C")
        .arg(&root)
        .args(["init", "-q", "-b", "main", "--object-format=sha256"])
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .unwrap();
    assert!(
        init.status.success(),
        "a sha256 repository could not be created with {}: {}",
        git_version(),
        String::from_utf8_lossy(&init.stderr)
    );
    git(&root, &["config", "--local", "user.name", "T"]);
    git(&root, &["config", "--local", "user.email", "t@t"]);
    write(&root, "a.txt", "base\n");
    commit_all(&root, "base");
    write(&root, "b.txt", "more\n");
    commit_all(&root, "more");
    let head = git_out(&root, &["rev-parse", "HEAD"]);
    let old = git_out(&root, &["rev-parse", "HEAD~1"]);
    assert_eq!(head.len(), 64, "{head}");

    let seams = GitSeams {
        forward_env: Some(forward(tmp.path())),
        ..GitSeams::default()
    };
    let repo = GitRepo::new(&root).with_seams(seams);

    let preflight = repo.root_merge_preflight("main", &head, LONG).unwrap();
    let Preflight::Refused(refusal) = preflight else {
        panic!("expected a refusal, got {preflight:?}");
    };
    assert_eq!(refusal.code(), "invalid_commit");

    let merge = repo
        .merge_into_root("main", "side", &head, &old, LONG)
        .unwrap();
    let RootMerge::Refused(refusal) = merge else {
        panic!("expected a refusal, got {merge:?}");
    };
    assert_eq!(refusal.code(), "invalid_commit");
    assert_eq!(git_out(&root, &["rev-parse", "main"]), head);
}
