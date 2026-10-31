//! The decide-merge command (Rust only, operator only): `decide-merge <requestId> --expect <hex> --nonce <n> [--client <label>] merge`,
//! the merge of a reviewed, verified integration into the project's branch, made by the daemon itself.
//!
//! Phase A checks the gates without a lock (git runs on the handler thread, never inside a kernel call). The merge lock
//! then covers phase B (the facts again, the marker, the merge) and phase C (the row of the decision, which also confirms
//! the integration). A detached thread then runs the cleanup of the confirmed branch and sends the PM one info message;
//! a daemon that stops before it ran loses both, and `cstan cleanup` offers the branch again.

use super::decide::{fresh_config, refused};
use super::decide_merge_seams::{seamed_repo, seams, wait_hold, DecideMergeSeams};
use super::shared::{fail, ok, CommandCall, CommandEnv, CommandResponse, ErrorCode};
use super::{cleanup, reports::with_integration_state};
use crate::deps::{new_context, Deps};
use capstan_config::Remote;
use capstan_kernel::areas::message_notices::notice_parties;
use capstan_kernel::areas::remote_decisions::{
    self, DecideInput, Kind, MergeFacts, MergeGate, VerifyState,
};
use capstan_kernel::integrate::{GitRepo, IntegrateError, Preflight, Refusal, RootCall, RootMerge};
use capstan_kernel::{KernelError, KernelResult};
use serde_json::{json, Value};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::{Arc, TryLockError};
use std::time::{Duration, Instant};

/// What phase A may take (the facts and the gates).
pub const PHASE_A_MS: u64 = 15_000;
/// What phase B may take: the 15 s re-check, the preflight (30 s), the merge (120 s) and its 20 s rollback.
pub const PHASE_B_MS: u64 = 185_000;
/// What phase C may take (kernel calls only).
pub const PHASE_C_MS: u64 = 20_000;

/// The margin a slow connection and the answer need.
const MARGIN_MS: u64 = 10_000;

const _: () =
    assert!(PHASE_A_MS + PHASE_B_MS + PHASE_C_MS + MARGIN_MS <= super::DECIDE_MERGE_LIMIT_MS);

const PREFLIGHT_TIMEOUT: Duration = Duration::from_secs(30);
const MERGE_TIMEOUT: Duration = Duration::from_secs(120);
const SMALL_GIT_TIMEOUT: Duration = Duration::from_secs(10);

/// The file in the git directory that exists while a merge of this daemon runs, or after one was interrupted.
const MARKER_NAME: &str = "capstan-remote-merge";

/// How far back the first-parent walk looks for the merge commit of a merge that was made earlier.
const LOOKUP_DEPTH: usize = 1000;

/// What the hand-off thread needs to clean up and tell the PM.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct MergeDone {
    pub decision_id: String,
    pub integration_id: String,
    pub branch: String,
    pub merge_commit: Option<String>,
    pub made_commit: bool,
}

/// The handler of the decide-merge route (`decide::register` serves it). `decide-merge targets` lists the branches a
/// merge may go into and changes nothing.
pub fn decide_merge(env: &CommandEnv<'_>, call: &CommandCall<'_>) -> Option<CommandResponse> {
    if call.args.len() == 1 && call.args[0] == "targets" {
        return Some(merge_targets(env));
    }
    let input = match remote_decisions::parse(call.args) {
        Ok(input) => input,
        Err(refusal) => {
            return Some(match ErrorCode::parse(refusal.code) {
                Some(code) => fail(code, refusal.message),
                None => fail(ErrorCode::InvalidRequest, refusal.message),
            })
        }
    };
    if input.kind != Kind::Integration || input.verb != "merge" {
        return Some(fail(
            ErrorCode::InvalidRequest,
            format!(
                "{} {} is not a merge of an integration",
                input.kind.as_str(),
                input.verb
            ),
        ));
    }
    let (remote, verify_sha) = match fresh_config(env) {
        Ok(config) => (
            config.remote.clone(),
            config.verify.as_ref().map(|verify| verify.commands_sha()),
        ),
        Err(error) => {
            env.log("decide_config_failed", json!({"error": error}));
            (Remote::default(), None)
        }
    };
    let run = Run {
        deps: env.deps,
        repo: seamed_repo(env.deps, &env.deps.options.workspace_root),
        remote,
        verify_sha,
        input,
        credential: call.credential.to_string(),
        seams: seams(env.deps),
    };
    let (response, hand_off) = run.run(call);
    if let Some(hand_off) = hand_off {
        spawn_hand_off(env.deps.clone(), hand_off);
    }
    response
}

/// `{enabled, defaultBranch, checkedOut, branches}`: whether `[remote] merge` is on, the branch a merge without
/// `--into` goes into (`merge_branch`, else the remote's default branch), the branch checked out in the project's root
/// and the branches `--into` may name (none checked out in another worktree, no integration branch).
fn merge_targets(env: &CommandEnv<'_>) -> CommandResponse {
    let remote = fresh_config(env).map(|config| config.remote.clone()).unwrap_or_default();
    let repo = seamed_repo(env.deps, &env.deps.options.workspace_root);
    let default_branch = if remote.merge {
        match repo.merge_target(remote.merge_branch.as_deref()) {
            Ok(branch) => branch,
            Err(error) => return git_response(&error),
        }
    } else {
        None
    };
    match repo.merge_targets(SMALL_GIT_TIMEOUT) {
        Ok(targets) => ok(json!({
            "enabled": remote.merge,
            "defaultBranch": default_branch,
            "checkedOut": targets.checked_out,
            "branches": targets.branches,
        })),
        Err(error) => git_response(&error),
    }
}

/// What the hand-off thread does after an applied merge.
struct HandOff {
    done: MergeDone,
    /// A marker of an interrupted merge for this head was found and removed.
    interrupted: bool,
    confirmed: bool,
}

/// One call of the route.
struct Run<'a> {
    deps: &'a Deps,
    repo: GitRepo,
    remote: Remote,
    verify_sha: Option<String>,
    input: DecideInput,
    credential: String,
    seams: Option<Arc<DecideMergeSeams>>,
}

/// What phase B decided.
enum PhaseB {
    Answer(CommandResponse),
    Record {
        merge_commit: Option<String>,
        made_commit: bool,
        go: Go,
    },
}

/// The values of a `MergeGate::Go`.
#[derive(Clone)]
struct Go {
    integration_id: String,
    branch: String,
    head_sha: String,
    verify: VerifyState,
    target: String,
    already_in_head: bool,
    /// The target is the branch checked out in the project's root: the merge updates that checkout. Otherwise
    /// (`--into` another branch) it moves only the branch.
    checkout: bool,
}

impl Run<'_> {
    fn run(&self, call: &CommandCall<'_>) -> (Option<CommandResponse>, Option<HandOff>) {
        let started = Instant::now();
        if let Gated::Answer(response) = self.gate() {
            return (Some(response), None);
        }
        self.note_slow("a", started, PHASE_A_MS);
        if let Some(seams) = &self.seams {
            if let Some(after_a) = &seams.after_a {
                after_a();
            }
        }
        if call.signal.reason().is_some() {
            return (None, None);
        }
        let lock = &self.deps.commands.remote_merge;
        let _held = match lock.try_lock() {
            Ok(held) => held,
            Err(TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
            Err(TryLockError::WouldBlock) => {
                return (
                    Some(fail(
                        ErrorCode::Busy,
                        "another merge is running; try again when it is done",
                    )),
                    None,
                )
            }
        };
        let started = Instant::now();
        let ran = catch_unwind(AssertUnwindSafe(|| {
            let (response, hand_off) = match self.phase_b() {
                PhaseB::Answer(response) => (response, None),
                PhaseB::Record {
                    merge_commit,
                    made_commit,
                    go,
                } => {
                    if self.seams.as_ref().is_some_and(|s| s.crash_after_b) {
                        return (
                            Some(fail(ErrorCode::Error, "injected stop after phase B")),
                            None,
                        );
                    }
                    self.phase_c(&go, merge_commit, made_commit)
                }
            };
            (Some(response), hand_off)
        }));
        self.note_slow("b+c", started, PHASE_B_MS + PHASE_C_MS);
        match ran {
            Ok(done) => done,
            Err(_) => (
                Some(merge_failed(
                    "internal",
                    true,
                    "the merge stopped on an internal error",
                )),
                None,
            ),
        }
    }

    fn note_slow(&self, phase: &str, started: Instant, limit_ms: u64) {
        let took = started.elapsed();
        if took > Duration::from_millis(limit_ms) {
            self.deps.detail_log(
                "decide_merge_slow_phase",
                json!({"phase": phase, "ms": took.as_millis() as u64, "limitMs": limit_ms}),
            );
        }
    }

    // ------------------------------------------------------------------------------------------------ facts and gates

    /// The branch and head of the integration, when it is merged.
    fn integration(&self) -> (Option<String>, Option<String>) {
        let id = self.input.target.clone();
        let Ok(row) = self.deps.kernel.run(move |core| core.integration(&id)) else {
            return (None, None);
        };
        if row["state"] != "merged" {
            return (None, None);
        }
        let text = |key: &str| row[key].as_str().map(str::to_string);
        (text("branch"), text("headSha"))
    }

    /// The branch `--into` names, if any.
    fn into(&self) -> Option<String> {
        self.input.args.get("into").and_then(Value::as_str).map(str::to_string)
    }

    /// The branch checked out in the project's root; none on a detached or unborn HEAD.
    fn checked_out(&self) -> Option<String> {
        let out = self
            .repo
            .root_git(&RootCall {
                args: ["symbolic-ref", "-q", "--short", "HEAD"].map(String::from).to_vec(),
                timeout: SMALL_GIT_TIMEOUT,
                config_env: Vec::new(),
                network: false,
                user_config: false,
            })
            .ok()?;
        let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
        (out.code == 0 && !text.is_empty()).then_some(text)
    }

    /// A merge goes into the root's checkout unless `--into` names another branch.
    fn into_checkout(&self, target: &str) -> bool {
        self.into().is_none() || self.checked_out().as_deref() == Some(target)
    }

    /// At most four git calls: the target branch, the checkout, the tip of the integration branch, whether the head is
    /// in the target.
    fn facts(&self) -> Result<MergeFacts, IntegrateError> {
        let (branch, head) = self.integration();
        let merge_enabled = self.remote.merge;
        let target_branch = if !merge_enabled {
            None
        } else if let Some(into) = self.into() {
            Some(into)
        } else {
            self.repo
                .merge_target(self.remote.merge_branch.as_deref())?
        };
        let usable = target_branch.is_some();
        let branch_tip = match (&branch, usable) {
            (Some(branch), true) => self.repo.branch_tip(branch)?,
            _ => None,
        };
        let already_in_head = match (&head, &target_branch) {
            (Some(head), Some(target)) if !self.into_checkout(target) => {
                self.repo.is_in_branch(head, target, SMALL_GIT_TIMEOUT)?
            }
            (Some(head), Some(_)) => self.repo.is_in_head(head)?,
            _ => false,
        };
        Ok(MergeFacts {
            merge_enabled,
            target_branch,
            branch_tip,
            verify_commands_sha: self.verify_sha.clone(),
            already_in_head,
            integration_running: false,
        })
    }

    /// `merge_gates` on the kernel thread, with the state of the integration pipeline that lives there.
    fn gates(&self, mut facts: MergeFacts) -> KernelResult<MergeGate> {
        let (input, credential) = (self.input.clone(), self.credential.clone());
        self.deps.kernel.run(move |core| {
            let context = new_context(core, &credential)?;
            facts.integration_running = with_integration_state(|state| state.busy());
            remote_decisions::merge_gates(core, &context, &input, &facts)
        })
    }

    fn gate(&self) -> Gated {
        let facts = match self.facts() {
            Ok(facts) => facts,
            Err(error) => return Gated::Answer(git_response(&error)),
        };
        self.gate_with(facts)
    }

    fn gate_with(&self, facts: MergeFacts) -> Gated {
        let target = facts.target_branch.clone();
        let already_in_head = facts.already_in_head;
        match self.gates(facts) {
            Err(error) => Gated::Answer(refused(&error)),
            Ok(MergeGate::Repeat(value)) => Gated::Answer(self.answer(value)),
            Ok(MergeGate::Go {
                integration_id,
                branch,
                head_sha,
                verify,
            }) => match target {
                Some(target) => Gated::Go(Go {
                    integration_id,
                    branch,
                    head_sha,
                    verify,
                    checkout: self.into_checkout(&target),
                    target,
                    already_in_head,
                }),
                None => Gated::Answer(fail(
                    ErrorCode::MergeDisabled,
                    "merging from a remote client is not enabled for this project",
                )),
            },
        }
    }

    /// The answer with the confirm state read from the ledger.
    fn answer(&self, mut value: Value) -> CommandResponse {
        let confirm = self.confirm_state();
        if let Some(merge) = value.get_mut("merge").and_then(Value::as_object_mut) {
            merge.insert("confirm".into(), json!(confirm));
        }
        ok(value)
    }

    /// `confirmed` when the ledger shows the integration confirmed (by this call, earlier or by hand), else `failed`.
    fn confirm_state(&self) -> &'static str {
        let id = self.input.target.clone();
        match self.deps.kernel.run(move |core| core.integration(&id)) {
            Ok(row) if row["state"] == "confirmed" => "confirmed",
            _ => "failed",
        }
    }

    // ------------------------------------------------------------------------------------------------ phase B

    fn phase_b(&self) -> PhaseB {
        // 1: the facts again; a row written meanwhile answers as a repeat.
        let facts = match self.facts() {
            Ok(facts) => facts,
            Err(error) => return PhaseB::Answer(git_response(&error)),
        };
        let go = match self.gate_with(facts) {
            Gated::Answer(response) => return PhaseB::Answer(response),
            Gated::Go(fresh) => fresh,
        };
        if let Some(seams) = &self.seams {
            wait_hold(&seams.hold_b);
        }
        // 2: the head is in HEAD (or the target branch) already.
        if go.already_in_head {
            return self.record_for(go, false, None);
        }
        // `--into` a branch that is not checked out: the branch moves, no working tree is touched, so neither the
        // marker nor the index lock of the checkout matters.
        if !go.checkout {
            let merged = self
                .repo
                .merge_into_ref(&go.target, &go.branch, &go.head_sha, MERGE_TIMEOUT);
            return match merged {
                Ok(RootMerge::Merged { commit }) => self.record_for(go, true, Some(commit)),
                Ok(RootMerge::AlreadyInHead) => self.record_for(go, false, None),
                Ok(RootMerge::Refused(refusal)) => PhaseB::Answer(refusal_response(&refusal)),
                Ok(RootMerge::Failed { reason, restored }) => PhaseB::Answer(merge_failed(
                    reason,
                    restored,
                    "the merge into the branch did not finish",
                )),
                Err(error) => PhaseB::Answer(git_response(&error)),
            };
        }
        let marker = match self.git_path(MARKER_NAME) {
            Ok(path) => path,
            Err(error) => return PhaseB::Answer(git_response(&error)),
        };
        // 3
        if marker_present(&marker) {
            return PhaseB::Answer(interrupted_answer(&marker, &self.marker_head(&marker)));
        }
        // 4
        match self.git_path("index.lock") {
            Ok(path) if std::fs::symlink_metadata(&path).is_ok() => {
                return PhaseB::Answer(merge_failed(
                    "index_lock_left",
                    false,
                    &format!("{} is left from an earlier git command", path.display()),
                ))
            }
            Ok(_) => {}
            Err(error) => return PhaseB::Answer(git_response(&error)),
        }
        // 5
        let old_head =
            match self
                .repo
                .root_merge_preflight(&go.target, &go.head_sha, PREFLIGHT_TIMEOUT)
            {
                Ok(Preflight::Ready { old_head }) => old_head,
                Ok(Preflight::AlreadyInHead) => return self.record_for(go, false, None),
                Ok(Preflight::Refused(refusal)) => {
                    return PhaseB::Answer(refusal_response(&refusal))
                }
                Err(error) => return PhaseB::Answer(git_response(&error)),
            };
        // 6
        let guard = match self.create_marker(&marker, &go) {
            Ok(guard) => guard,
            Err(answer) => return PhaseB::Answer(answer),
        };
        if let Some(seams) = &self.seams {
            wait_hold(&seams.hold_after_marker);
            if seams.panic_in_b {
                panic!("injected panic in phase B");
            }
        }
        // 7
        let merged = self.repo.merge_into_root(
            &go.target,
            &go.branch,
            &go.head_sha,
            &old_head,
            MERGE_TIMEOUT,
        );
        if self.seams.as_ref().is_some_and(|s| s.crash_in_b) {
            std::mem::forget(guard);
        } else {
            drop(guard);
        }
        match merged {
            Ok(RootMerge::Merged { commit }) => self.record_for(go, true, Some(commit)),
            Ok(RootMerge::AlreadyInHead) => self.record_for(go, false, None),
            Ok(RootMerge::Refused(refusal)) => PhaseB::Answer(refusal_response(&refusal)),
            Ok(RootMerge::Failed { reason, restored }) => PhaseB::Answer(merge_failed(
                reason,
                restored,
                "the merge into the project's branch did not finish",
            )),
            Err(error) => PhaseB::Answer(git_response(&error)),
        }
    }

    fn record_for(&self, go: Go, made_commit: bool, commit: Option<String>) -> PhaseB {
        let merge_commit = if made_commit {
            commit
        } else if go.checkout {
            self.merge_commit_of(&go.head_sha, "HEAD")
        } else {
            self.merge_commit_of(&go.head_sha, &format!("refs/heads/{}", go.target))
        };
        PhaseB::Record {
            merge_commit,
            made_commit,
            go,
        }
    }

    /// `git rev-parse --git-path <name>` as a path.
    fn git_path(&self, name: &str) -> Result<PathBuf, IntegrateError> {
        let out = self.repo.root_git(&RootCall {
            args: ["rev-parse", "--git-path", name].map(String::from).to_vec(),
            timeout: SMALL_GIT_TIMEOUT,
            config_env: Vec::new(),
            network: false,
            user_config: false,
        })?;
        let text = String::from_utf8_lossy(&out.stdout);
        let text = text.trim_end_matches('\n');
        if out.code != 0 || text.is_empty() {
            return Err(unavailable("git did not print the path of its directory"));
        }
        Ok(self.repo.root().join(text))
    }

    /// The head the marker file names, for the message (`?` when it cannot be read).
    fn marker_head(&self, marker: &Path) -> String {
        std::fs::read_to_string(marker)
            .ok()
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
            .and_then(|value| value["headSha"].as_str().map(str::to_string))
            .unwrap_or_else(|| "?".to_string())
    }

    /// Creates the marker with `O_EXCL`; an existing one is an interrupted merge.
    fn create_marker(&self, marker: &Path, go: &Go) -> Result<MarkerGuard, CommandResponse> {
        let file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(marker);
        let mut file = match file {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                return Err(interrupted_answer(marker, &self.marker_head(marker)))
            }
            Err(error) => {
                return Err(merge_failed(
                    "marker_unwritable",
                    true,
                    &format!(
                        "{} could not be created: {}",
                        marker.display(),
                        error.kind()
                    ),
                ))
            }
        };
        let guard = MarkerGuard {
            path: marker.to_path_buf(),
        };
        let body = json!({
            "headSha": go.head_sha,
            "nonce": self.input.nonce,
            "integrationId": go.integration_id,
        });
        if let Err(error) = file.write_all(body.to_string().as_bytes()) {
            return Err(merge_failed(
                "marker_unwritable",
                true,
                &format!(
                    "{} could not be written: {}",
                    marker.display(),
                    error.kind()
                ),
            ));
        }
        Ok(guard)
    }

    /// The first-parent commit of `tip`, at most `LOOKUP_DEPTH` back, whose second parent is `head_sha`.
    fn merge_commit_of(&self, head_sha: &str, tip: &str) -> Option<String> {
        let depth = self
            .seams
            .as_ref()
            .and_then(|s| s.lookup_depth)
            .unwrap_or(LOOKUP_DEPTH);
        let out = self
            .repo
            .root_git(&RootCall {
                args: [
                    "rev-list",
                    "--first-parent",
                    "--parents",
                    &format!("--max-count={depth}"),
                    tip,
                ]
                .map(String::from)
                .to_vec(),
                timeout: SMALL_GIT_TIMEOUT,
                config_env: Vec::new(),
                network: false,
                user_config: false,
            })
            .ok()?;
        if out.code != 0 {
            return None;
        }
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .find_map(|line| {
                let mut parts = line.split(' ');
                let commit = parts.next()?;
                let _first = parts.next()?;
                (parts.next() == Some(head_sha)).then(|| commit.to_string())
            })
    }

    // ------------------------------------------------------------------------------------------------ phase C

    fn phase_c(
        &self,
        go: &Go,
        merge_commit: Option<String>,
        made_commit: bool,
    ) -> (CommandResponse, Option<HandOff>) {
        let merge = json!({
            "integrationId": go.integration_id,
            "branch": go.branch,
            "into": go.target,
            "mergeCommit": merge_commit,
            "alreadyInHead": !made_commit,
            "verify": match &go.verify {
                VerifyState::Passed { run_id } => json!({"state": "passed", "runId": run_id}),
                VerifyState::NotConfigured => json!({"state": "not_configured"}),
            },
            "cleanup": self.cleanup_member(),
            "push": {"state": "disabled", "reason": "push_off"},
        });
        let recorded = if self.seams.as_ref().is_some_and(|s| s.fail_record) {
            Err(KernelError::controller("injected record failure"))
        } else {
            let (input, credential) = (self.input.clone(), self.credential.clone());
            self.deps.kernel.run(move |core| {
                let context = new_context(core, &credential)?;
                remote_decisions::record_merge(core, &context, &input, merge)
            })
        };
        let value = match recorded {
            Ok(value) => value,
            Err(error) => match self.stored_result() {
                // The row is written and the settle failed: the PM confirms by hand.
                Some(value) => value,
                None => return (unrecorded(merge_commit.as_deref(), &error), None),
            },
        };
        let interrupted = !made_commit && self.retire_marker(&go.head_sha);
        let done = MergeDone {
            decision_id: value["decisionId"].as_str().unwrap_or_default().to_string(),
            integration_id: go.integration_id.clone(),
            branch: go.branch.clone(),
            merge_commit,
            made_commit,
        };
        // The settle seam stands in for a settle that failed after the row was written.
        let settle_failed = self.seams.as_ref().is_some_and(|s| s.fail_settle);
        let confirmed = !settle_failed && self.confirm_state() == "confirmed";
        let answer = if settle_failed {
            let mut value = value;
            if let Some(merge) = value.get_mut("merge").and_then(Value::as_object_mut) {
                merge.insert("confirm".into(), json!("failed"));
            }
            ok(value)
        } else {
            self.answer(value)
        };
        (
            answer,
            Some(HandOff {
                done,
                interrupted,
                confirmed,
            }),
        )
    }

    /// `{state: scheduled}` unless the project's `[cleanup]` is off.
    fn cleanup_member(&self) -> Value {
        let off =
            self.deps.options.capstan.as_ref().is_some_and(|config| {
                config.cleanup.branches == capstan_config::CleanupBranches::Off
            });
        if off {
            Value::Null
        } else {
            json!({"state": "scheduled"})
        }
    }

    /// The stored result of this call's nonce, when its row was written.
    fn stored_result(&self) -> Option<Value> {
        let dummy = MergeFacts {
            merge_enabled: false,
            target_branch: None,
            branch_tip: None,
            verify_commands_sha: None,
            already_in_head: false,
            integration_running: false,
        };
        match self.gates(dummy) {
            Ok(MergeGate::Repeat(value)) => Some(value),
            _ => None,
        }
    }

    /// Removes the marker of an interrupted merge of this head after its merge was recorded; true when one was removed.
    fn retire_marker(&self, head_sha: &str) -> bool {
        let Ok(marker) = self.git_path(MARKER_NAME) else {
            return false;
        };
        if !marker_present(&marker) || self.marker_head(&marker) != head_sha {
            return false;
        }
        std::fs::remove_file(&marker).is_ok()
    }
}

enum Gated {
    Answer(CommandResponse),
    Go(Go),
}

/// Removes the marker when it goes out of scope, on every exit of phase B.
struct MarkerGuard {
    path: PathBuf,
}

impl Drop for MarkerGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

fn marker_present(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok()
}

// ---------------------------------------------------------------------------------------------------- the hand-off

fn spawn_hand_off(deps: Deps, hand_off: HandOff) {
    let spawned = std::thread::Builder::new()
        .name("decide-merge-hand-off".into())
        .spawn({
            let deps = deps.clone();
            move || run_hand_off(&deps, hand_off)
        });
    if let Err(error) = spawned {
        deps.detail_log(
            "decide_merge_hand_off_failed",
            json!({"error": error.to_string()}),
        );
    }
}

fn run_hand_off(deps: &Deps, hand_off: HandOff) {
    if let Some(seams) = seams(deps) {
        wait_hold(&seams.block_pm_message);
    }
    let env = CommandEnv::new(deps);
    let HandOff {
        done,
        interrupted,
        confirmed,
    } = hand_off;
    if confirmed {
        // A failure is logged by the cleanup itself and does not stop the message.
        let _ = cleanup::after_confirm(&env, &done.integration_id, "operator");
    }
    let body = pm_message(&done, interrupted, confirmed);
    let credential = deps.credential().to_string();
    let sent = deps.kernel.run(move |core| {
        let Some(parties) = notice_parties(core.kernel())? else {
            return Ok(false);
        };
        let context = new_context(core, &credential)?;
        core.enqueue_message(
            &context,
            &json!({
                "recipientAgentId": parties.pm.agent_id,
                "body": body,
                "actionNeeded": false,
            }),
        )?;
        Ok(true)
    });
    match sent {
        Ok(true) => {}
        Ok(false) => env.log(
            "pm_message_skipped",
            json!({"integrationId": done.integration_id, "decisionId": done.decision_id, "reason": "no active PM"}),
        ),
        Err(error) => env.log(
            "pm_message_failed",
            json!({"integrationId": done.integration_id, "decisionId": done.decision_id, "error": error.message()}),
        ),
    }
}

fn pm_message(done: &MergeDone, interrupted: bool, confirmed: bool) -> String {
    let commit = done
        .merge_commit
        .as_deref()
        .map_or("(not found)".to_string(), str::to_string);
    let mut text = if done.made_commit {
        format!(
            "The user merged integration {} (branch {}) into the project's branch from a remote client: merge commit {commit}.",
            done.integration_id, done.branch
        )
    } else {
        format!(
            "The user recorded the merge of integration {} (branch {}) from a remote client; the daemon did not merge it, the head was already in the project's branch (merge commit {commit}).",
            done.integration_id, done.branch
        )
    };
    if interrupted {
        text.push_str(
            " An earlier remote merge of this head had been interrupted; its marker was removed.",
        );
    }
    if confirmed {
        text.push_str(
            " The integration is confirmed; the branch cleanup follows the project's settings.",
        );
    } else {
        text.push_str(&format!(
            " The integration could not be confirmed: run integrate confirm {} yourself.",
            done.integration_id
        ));
    }
    text
}

// ------------------------------------------------------------------------------------------------------ the answers

fn unavailable(message: impl Into<String>) -> IntegrateError {
    IntegrateError::Integration {
        code: "git_unavailable".into(),
        message: message.into(),
    }
}

/// The text for people of each refusal code of the merge.
fn refusal_text(code: &str) -> &'static str {
    match code {
        "unsafe_config" => "the repository's git configuration is not safe for a merge",
        "invalid_commit" => "the commit to merge is not a valid commit",
        "invalid_branch" => "the branch is not a valid branch name",
        "wrong_branch" => "the project's checkout is not on the branch the merge goes into",
        "no_identity" => "git has no committer identity in this repository",
        "dirty_tree" => "the project's working tree has uncommitted changes to tracked files",
        "merge_conflict" => "the integration does not merge cleanly into the project's branch",
        "git_too_old" => "git is older than the merge needs",
        _ => "the merge was refused",
    }
}

/// The answer for a code of the merge: one of the eight refusals, a failure reason of `merge_failed` (not restored) or
/// `git_unavailable`; any other code is a plain error.
#[doc(hidden)]
pub fn refusal_for(code: &str) -> CommandResponse {
    match code {
        "unsafe_config" | "invalid_commit" | "invalid_branch" | "wrong_branch" | "no_identity"
        | "dirty_tree" | "merge_conflict" | "git_too_old" => match ErrorCode::parse(code) {
            Some(parsed) => fail(parsed, refusal_text(code)),
            None => fail(ErrorCode::Error, "the command failed"),
        },
        "merge_failed" | "timeout" | "index_lock_left" => merge_failed(
            code,
            false,
            "the merge into the project's branch did not finish",
        ),
        "git_unavailable" => fail(ErrorCode::GitUnavailable, "git is not available"),
        _ => fail(ErrorCode::Error, "the command failed"),
    }
}

fn refusal_response(refusal: &Refusal) -> CommandResponse {
    let CommandResponse::Err { code, message } = refusal_for(refusal.code()) else {
        return fail(ErrorCode::Error, "the command failed");
    };
    let mut text = message;
    if !refusal.files().is_empty() {
        text.push_str(&format!(": {}", refusal.files().join(", ")));
        if refusal.omitted() > 0 {
            text.push_str(&format!(" and {} more", refusal.omitted()));
        }
    }
    fail(code, text)
}

/// A merge that started and did not end well: the reason and whether the tree was restored are in the message.
fn merge_failed(reason: &str, restored: bool, detail: &str) -> CommandResponse {
    fail(
        ErrorCode::MergeFailed,
        format!("the merge failed: reason {reason}, restored {restored}: {detail}"),
    )
}

fn interrupted_answer(marker: &Path, head_sha: &str) -> CommandResponse {
    merge_failed(
        "interrupted",
        false,
        &format!(
            "an earlier remote merge of {head_sha} was interrupted and left {}; no remote merge runs until you check git status and delete that file",
            marker.display()
        ),
    )
}

fn unrecorded(merge_commit: Option<&str>, error: &KernelError) -> CommandResponse {
    let made = match merge_commit {
        Some(commit) => format!("the merge was made as commit {commit}"),
        None => "the integration is merged into the project's branch".to_string(),
    };
    fail(
        ErrorCode::MergeUnrecorded,
        format!(
            "{made}, but its decision could not be recorded ({}); send the same call again to record it, or confirm the integration by hand",
            error.message()
        ),
    )
}

/// An error of git: a refusal code of the merge keeps its code, the rest is `git_unavailable` with git's own text.
fn git_response(error: &IntegrateError) -> CommandResponse {
    match error {
        IntegrateError::Integration { code, message } => {
            match ErrorCode::parse(code).filter(|_| {
                matches!(
                    code.as_str(),
                    "unsafe_config"
                        | "invalid_commit"
                        | "invalid_branch"
                        | "wrong_branch"
                        | "no_identity"
                        | "dirty_tree"
                        | "merge_conflict"
                        | "git_too_old"
                )
            }) {
                Some(code) => fail(code, message.clone()),
                None => fail(ErrorCode::GitUnavailable, message.clone()),
            }
        }
        IntegrateError::Git(message) => fail(ErrorCode::GitUnavailable, message.clone()),
        IntegrateError::Kernel(error) => refused(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use capstan_kernel::areas::remote_decisions::MERGE_ERROR_CODES;

    fn code_of(response: &CommandResponse) -> &'static str {
        response.code_str()
    }

    #[test]
    fn the_eight_refusals_map_to_themselves() {
        for code in [
            "unsafe_config",
            "invalid_commit",
            "invalid_branch",
            "wrong_branch",
            "no_identity",
            "dirty_tree",
            "merge_conflict",
            "git_too_old",
        ] {
            let response = refusal_for(code);
            assert_eq!(code_of(&response), code);
            assert!(MERGE_ERROR_CODES.contains(&code_of(&response)));
        }
    }

    #[test]
    fn a_failure_reason_is_merge_failed_with_reason_and_restored_in_the_message() {
        for reason in ["merge_failed", "timeout", "index_lock_left"] {
            let response = refusal_for(reason);
            assert_eq!(code_of(&response), "merge_failed");
            assert!(MERGE_ERROR_CODES.contains(&code_of(&response)));
            let CommandResponse::Err { message, .. } = response else {
                panic!("an error");
            };
            assert!(message.contains(&format!("reason {reason}")), "{message}");
            assert!(message.contains("restored false"), "{message}");
        }
        let CommandResponse::Err { message, .. } = merge_failed("timeout", true, "x") else {
            panic!("an error");
        };
        assert!(message.contains("restored true"), "{message}");
    }

    #[test]
    fn git_unavailable_keeps_gits_text() {
        let text = "fatal: detected dubious ownership in repository at '/x'";
        let response = git_response(&unavailable(text));
        assert_eq!(code_of(&response), "git_unavailable");
        assert!(MERGE_ERROR_CODES.contains(&code_of(&response)));
        assert_eq!(response, fail(ErrorCode::GitUnavailable, text));
        assert_eq!(
            git_response(&IntegrateError::Git("git could not run".into())),
            fail(ErrorCode::GitUnavailable, "git could not run")
        );
    }

    #[test]
    fn a_refusal_code_of_git_keeps_its_code() {
        let error = IntegrateError::Integration {
            code: "invalid_branch".into(),
            message: "the branch x is not a valid branch name".into(),
        };
        assert_eq!(code_of(&git_response(&error)), "invalid_branch");
    }

    #[test]
    fn a_refusal_lists_its_files() {
        let refusal = Refusal::new("merge_conflict", vec![b"a.txt".to_vec(), b"b.txt".to_vec()]);
        let CommandResponse::Err { code, message } = refusal_response(&refusal) else {
            panic!("an error");
        };
        assert_eq!(code, ErrorCode::MergeConflict);
        assert!(message.contains("a.txt, b.txt"), "{message}");
    }
}
