//! The scratch project a scenario runs in: `.capstan/` with its config and operator key (the files `cstan init` writes, the
//! ones `cstan-daemon` reads) and, for a scenario with a `repo`, a git repository with fixed authors and dates so a commit
//! has the same id here as in the Node export.

use serde_json::Value;
use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The project of a transcript scenario: its id, name and operator credential.
#[derive(Clone, Debug)]
pub struct Identity {
    pub project_id: String,
    pub name: String,
    pub credential: String,
}

impl Identity {
    pub fn of(project: &Value) -> Self {
        let text = |key: &str| project[key].as_str().expect("project field").to_string();
        Self {
            project_id: text("projectId"),
            name: text("name"),
            credential: text("credential"),
        }
    }

    /// The `InitialProject` the daemon opens.
    pub fn initial_project(&self) -> capstan_kernel::types::InitialProject {
        capstan_kernel::types::InitialProject {
            project_id: self.project_id.clone(),
            name: self.name.clone(),
            owner_credential: self.credential.clone(),
            initial_inputs: capstan_daemon::run::placeholder_inputs(),
        }
    }
}

/// Writes `.capstan/{project.json,operator.key,state/}` under `directory` (modes 0700 and 0600); returns the state directory.
pub fn write_project(directory: &Path, identity: &Identity) -> PathBuf {
    let capstan = directory.join(".capstan");
    let state = capstan.join("state");
    std::fs::create_dir(&capstan).expect("the .capstan directory");
    std::fs::set_permissions(&capstan, std::fs::Permissions::from_mode(0o700)).unwrap();
    let config = serde_json::json!({
        "schemaVersion": 1,
        "projectId": identity.project_id,
        "name": identity.name,
        "stateDirectory": state.to_string_lossy(),
        "maxSlices": 4,
        "maxRunMs": 3_600_000,
        "maxDispatches": 16,
    });
    for (name, text) in [
        ("project.json", format!("{config:#}\n")),
        ("operator.key", format!("{}\n", identity.credential)),
    ] {
        let file = capstan.join(name);
        std::fs::write(&file, text).unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    std::fs::create_dir(&state).expect("the state directory");
    std::fs::set_permissions(&state, std::fs::Permissions::from_mode(0o700)).unwrap();
    state
}

fn git(directory: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(directory)
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .env("LC_ALL", "C")
        .env("HOME", "/nonexistent")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_AUTHOR_NAME", "Transcript")
        .env("GIT_AUTHOR_EMAIL", "transcript@example.invalid")
        .env("GIT_COMMITTER_NAME", "Transcript")
        .env("GIT_COMMITTER_EMAIL", "transcript@example.invalid")
        .env("GIT_AUTHOR_DATE", "2026-01-01T00:00:00+00:00")
        .env("GIT_COMMITTER_DATE", "2026-01-01T00:00:00+00:00")
        .output()
        .expect("git runs");
    assert!(
        output.status.success(),
        "git {} failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn write_files(directory: &Path, files: &Value) {
    for (name, content) in files.as_object().into_iter().flatten() {
        let file = directory.join(name);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, content.as_str().expect("file content")).unwrap();
    }
}

/// Builds the scenario's repository in `directory` and returns the tip of every branch (`main` first), which must equal
/// the `heads` the Node export recorded.
pub fn build_repo(directory: &Path, repo: &Value) -> BTreeMap<String, String> {
    git(directory, &["init", "--quiet", "--initial-branch=main"]);
    std::fs::write(directory.join(".git/info/exclude"), ".capstan/\n").unwrap();
    write_files(directory, &repo["files"]);
    git(directory, &["add", "--all"]);
    git(
        directory,
        &["commit", "--quiet", "-m", "chore: initial commit"],
    );
    let mut heads = BTreeMap::new();
    heads.insert("main".to_string(), git(directory, &["rev-parse", "main"]));
    // Branches are made in the order the export recorded them (the object's order is the file's order).
    for (branch, spec) in repo["branches"].as_object().into_iter().flatten() {
        let from = spec["from"].as_str().unwrap_or("main");
        git(directory, &["checkout", "--quiet", "-b", branch, from]);
        write_files(directory, &spec["files"]);
        git(directory, &["add", "--all"]);
        let message = spec["message"]
            .as_str()
            .map(str::to_string)
            .unwrap_or_else(|| format!("feat: change on {branch}"));
        git(directory, &["commit", "--quiet", "-m", &message]);
        heads.insert(branch.clone(), git(directory, &["rev-parse", branch]));
    }
    git(directory, &["checkout", "--quiet", "main"]);
    heads
}
