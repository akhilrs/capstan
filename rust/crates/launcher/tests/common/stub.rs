//! `StubHerdr`: the Rust form of `StubAdapter` of test/launcher-stubs.ts behind the recording proxy of
//! test/launcher-parity-export.ts. It answers the way the Node stub does, records every call as `{m, a}` (the method name
//! and its arguments as Node passes them), and throws a scripted failure after recording a call and before the stub is
//! reached.

use super::normal;
use capstan_herdr::api::*;
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::Mutex;

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// A scripted failure, as a sequence writes it.
#[derive(Clone, Debug)]
pub struct Failure {
    pub kind: String,
    pub code: Option<String>,
    pub message: String,
}

impl Failure {
    pub fn from_value(value: &Value) -> Failure {
        Failure {
            kind: value["kind"].as_str().unwrap_or("").to_string(),
            code: value["code"].as_str().map(str::to_string),
            message: value["message"].as_str().unwrap_or("").to_string(),
        }
    }

    fn error(&self) -> AdapterError {
        let message = self.message.clone();
        match self.kind.as_str() {
            "herdr" => AdapterError::Herdr(HerdrError::new(
                self.code.clone().unwrap_or_else(|| "herdr_error".into()),
                message,
            )),
            "pane_gone" => AdapterError::PaneGone(message),
            "agent_pane_mismatch" => AdapterError::AgentPaneMismatch(message),
            "pane_lost" => AdapterError::PaneLost(message),
            "phase" => AdapterError::Phase(message),
            "prompt_unrecognized" => AdapterError::PromptUnrecognized(message),
            "shell_not_ready" => AdapterError::ShellNotReady(message),
            other => panic!("unknown failure kind {other}"),
        }
    }
}

#[derive(Clone, Debug)]
struct Pane {
    pane_id: String,
    width: i64,
    height: i64,
}

struct State {
    scratch: String,
    prompts_dir: PathBuf,
    worktree_base: String,
    real_git: String,
    project_root: String,
    calls: Vec<Value>,
    failures: HashMap<String, VecDeque<Failure>>,
    metadata: Vec<(String, BTreeMap<String, String>)>,
    entries: HashMap<String, Option<String>>,
    agent_panes: HashMap<String, String>,
    counter: u64,
    start_status: StartStatus,
    dialog_handled: bool,
    observation: HerdrState,
    pm_workspace: Option<String>,
    tab_panes: Vec<Pane>,
    layout_size: (i64, i64),
    zoomed: bool,
    strays: HashMap<String, Vec<PaneAtPath>>,
    placed: u64,
    identities: HashMap<String, PaneIdentity>,
    gone: BTreeSet<String>,
    working: BTreeSet<String>,
    screens: HashMap<String, String>,
    prompt_count: usize,
    tokens: HashMap<String, String>,
}

pub struct StubHerdr {
    state: Mutex<State>,
}

fn role_name(role: PaneRole) -> &'static str {
    role.as_str()
}

impl StubHerdr {
    pub fn new(
        scratch: &str,
        prompts_dir: PathBuf,
        worktree_base: &str,
        real_git: &str,
        project_root: &str,
    ) -> Self {
        Self {
            state: Mutex::new(State {
                scratch: scratch.to_string(),
                prompts_dir,
                worktree_base: worktree_base.to_string(),
                real_git: real_git.to_string(),
                project_root: project_root.to_string(),
                calls: Vec::new(),
                failures: HashMap::new(),
                metadata: Vec::new(),
                entries: HashMap::new(),
                agent_panes: HashMap::new(),
                counter: 0,
                start_status: StartStatus::Started,
                dialog_handled: true,
                observation: HerdrState::Idle,
                pm_workspace: None,
                tab_panes: Vec::new(),
                layout_size: (200, 50),
                zoomed: false,
                strays: HashMap::new(),
                placed: 10,
                identities: HashMap::new(),
                gone: BTreeSet::new(),
                working: BTreeSet::new(),
                screens: HashMap::new(),
                prompt_count: 0,
                tokens: HashMap::new(),
            }),
        }
    }

    /// The calls recorded since the last `take_calls`.
    pub fn take_calls(&self) -> Vec<Value> {
        std::mem::take(&mut lock(&self.state).calls)
    }

    pub fn token_of(&self, agent: &str) -> Option<String> {
        lock(&self.state).tokens.get(agent).cloned()
    }

    /// Applies a `stub` step of a sequence.
    pub fn apply(&self, step: &Value) {
        let mut state = lock(&self.state);
        let pane = step["pane"].as_str().unwrap_or("").to_string();
        match step["do"].as_str().unwrap_or("") {
            "fail" => {
                let times = step["times"].as_u64().unwrap_or(1);
                let queue = state
                    .failures
                    .entry(step["method"].as_str().unwrap_or("").to_string())
                    .or_default();
                for _ in 0..times {
                    queue.push_back(Failure::from_value(&step["error"]));
                }
            }
            "identity" => {
                state.identities.insert(
                    pane,
                    PaneIdentity {
                        terminal_id: step["terminalId"].as_str().map(str::to_string),
                        agent: step["agent"].as_str().map(str::to_string),
                        project: step["project"].as_str().map(str::to_string),
                    },
                );
            }
            "gone" => {
                state.gone.insert(pane);
            }
            "forget_registry" => {
                state.entries.clear();
                state.agent_panes.clear();
            }
            "observation" => {
                state.observation = HerdrState::of(step["value"].as_str().unwrap_or("idle"))
            }
            "start_status" => {
                state.start_status = if step["value"] == "blocked_at_startup" {
                    StartStatus::BlockedAtStartup
                } else {
                    StartStatus::Started
                }
            }
            "dialog_handled" => state.dialog_handled = step["value"] == true,
            "screen" => {
                state
                    .screens
                    .insert(pane, step["text"].as_str().unwrap_or("").to_string());
            }
            "working" => {
                state.working.insert(pane);
            }
            "zoomed" => state.zoomed = step["value"] == true,
            "strays" => {
                let directory = step["directory"]
                    .as_str()
                    .unwrap_or("")
                    .replace("<worktrees>", &state.worktree_base);
                let panes = step["panes"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|p| PaneAtPath {
                        pane_id: p["paneId"].as_str().unwrap_or("").to_string(),
                        workspace_id: p["workspaceId"].as_str().unwrap_or("").to_string(),
                    })
                    .collect();
                state.strays.insert(directory, panes);
            }
            "layout_size" => {
                state.layout_size = (
                    step["width"].as_i64().unwrap_or(200),
                    step["height"].as_i64().unwrap_or(50),
                )
            }
            "setup" | "teardown" => {}
            other => panic!("unknown stub action {other}"),
        }
    }

    /// `enter` for a call that needs nothing of the state.
    fn record(&self, method: &str, args: Value) -> Result<(), AdapterError> {
        self.enter(method, args).map(drop)
    }

    /// Records a call and takes a scripted failure for it.
    fn enter(
        &self,
        method: &str,
        args: Value,
    ) -> Result<std::sync::MutexGuard<'_, State>, AdapterError> {
        let mut state = lock(&self.state);
        let recorded = normal(&json!({"m": method, "a": args}), &state.scratch);
        state.calls.push(recorded);
        if let Some(failure) = state
            .failures
            .get_mut(method)
            .and_then(|queue| queue.pop_front())
        {
            return Err(failure.error());
        }
        Ok(state)
    }
}

fn strings(map: &BTreeMap<String, String>) -> Value {
    let mut out = Map::new();
    for (key, value) in map {
        out.insert(key.clone(), json!(value));
    }
    Value::Object(out)
}

impl PaneRegistry for StubHerdr {
    fn pane_for_agent(&self, agent_id: &str) -> Option<String> {
        lock(&self.state).agent_panes.get(agent_id).cloned()
    }

    fn pane_entry(&self, pane_id: &str) -> Option<PaneEntry> {
        lock(&self.state)
            .entries
            .get(pane_id)
            .map(|agent| PaneEntry {
                role: PaneRole::Worker,
                phase: PanePhase::Started,
                kind: String::new(),
                agent: agent.clone(),
                worktree_path: None,
                workspace_id: None,
            })
    }

    fn agent_observation(&self, agent_id: &str) -> AdapterResult<HerdrState> {
        let state = self.enter("agentObservation", json!([agent_id]))?;
        Ok(state.observation)
    }
}

impl LauncherAdapter for StubHerdr {
    fn create_workspace(&self, input: CreateWorkspaceInput<'_>) -> AdapterResult<WorkspaceCreated> {
        let mut state = self.enter(
            "createWorkspace",
            json!([{"cwd": input.cwd, "label": input.label, "role": role_name(input.role)}]),
        )?;
        state.counter += 1;
        let workspace_id = format!("w{}", state.counter);
        let pane_id = format!("{workspace_id}:p1");
        state.entries.insert(pane_id.clone(), None);
        if input.role == PaneRole::Pm {
            state.pm_workspace = Some(workspace_id.clone());
            let (width, height) = state.layout_size;
            state.tab_panes = vec![Pane {
                pane_id: pane_id.clone(),
                width,
                height,
            }];
        }
        Ok(WorkspaceCreated {
            tab_id: format!("{workspace_id}:t1"),
            workspace_id,
            pane_id,
        })
    }

    fn create_worktree(&self, input: CreateWorktreeInput<'_>) -> AdapterResult<WorktreeCreated> {
        let mut state = self.enter(
            "createWorktree",
            json!([{"workspaceId": input.workspace_id, "branch": input.branch, "label": input.label, "base": input.base}]),
        )?;
        state.counter += 1;
        let workspace_id = format!("w{}", state.counter);
        let pane_id = format!("{workspace_id}:p1");
        state.entries.insert(pane_id.clone(), None);
        let agent = input.label.rsplit(" · ").next().unwrap_or("").to_string();
        let path = format!("{}/{agent}", state.worktree_base);
        // Herdr makes the worktree on disk; the stub only names it.
        let mut command = std::process::Command::new(&state.real_git);
        command
            .args(["worktree", "add", "-b", input.branch, &path])
            .envs(super::engine::git_environment())
            .current_dir(&state.project_root);
        if let Some(base) = input.base {
            command.arg(base);
        }
        let output = command.output().expect("git runs");
        assert!(
            output.status.success(),
            "git worktree add: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        Ok(WorktreeCreated {
            workspace_id,
            pane_id,
            path,
            branch: input.branch.to_string(),
        })
    }

    fn create_tab(&self, input: CreateTabInput<'_>) -> AdapterResult<TabCreated> {
        let mut state = self.enter(
            "createTab",
            json!([{"workspaceId": input.workspace_id, "cwd": input.cwd, "label": input.label, "role": role_name(input.role)}]),
        )?;
        state.counter += 1;
        let pane_id = format!("{}:p{}", input.workspace_id, state.counter + 100);
        state.entries.insert(pane_id.clone(), None);
        if input.role == PaneRole::Pm {
            let (width, height) = state.layout_size;
            state.tab_panes = vec![Pane {
                pane_id: pane_id.clone(),
                width,
                height,
            }];
        }
        Ok(TabCreated {
            tab_id: format!("{}:t{}", input.workspace_id, state.counter),
            pane_id,
        })
    }

    fn pane_layout(&self, pane_id: &str) -> AdapterResult<PaneLayoutView> {
        let state = self.enter("paneLayout", json!([pane_id]))?;
        let workspace = state.pm_workspace.clone().unwrap_or_else(|| "w1".into());
        Ok(PaneLayoutView {
            tab_id: format!("{workspace}:t1"),
            workspace_id: workspace,
            zoomed: state.zoomed,
            panes: state
                .tab_panes
                .iter()
                .map(|p| LayoutPane {
                    pane_id: p.pane_id.clone(),
                    width: p.width,
                    height: p.height,
                })
                .collect(),
        })
    }

    fn place_pane(&self, input: PlacePaneInput<'_>) -> AdapterResult<PaneAtPath> {
        let mut state = self.enter(
            "placePane",
            json!([{
                "paneId": input.pane_id,
                "tabId": input.tab_id,
                "targetPaneId": input.target_pane_id,
                "direction": match input.direction { SplitDirection::Right => "right", SplitDirection::Down => "down" },
                "keep": input.keep,
                "worktreePath": input.worktree_path,
            }]),
        )?;
        let index = state
            .tab_panes
            .iter()
            .position(|p| p.pane_id == input.target_pane_id)
            .expect("the target pane is in the tab");
        let workspace = state.pm_workspace.clone().expect("a PM workspace");
        state.placed += 1;
        let pane_id = format!("{workspace}:p{}", state.placed);
        let kept = |n: i64| (n as f64 * input.keep).floor() as i64;
        let target = state.tab_panes[index].clone();
        let next = match input.direction {
            SplitDirection::Right => {
                state.tab_panes[index].width = kept(target.width);
                Pane {
                    pane_id: pane_id.clone(),
                    width: target.width - kept(target.width),
                    height: target.height,
                }
            }
            SplitDirection::Down => {
                state.tab_panes[index].height = kept(target.height);
                Pane {
                    pane_id: pane_id.clone(),
                    width: target.width,
                    height: target.height - kept(target.height),
                }
            }
        };
        state.tab_panes.push(next);
        state.entries.remove(input.pane_id);
        state.entries.insert(pane_id.clone(), None);
        Ok(PaneAtPath {
            pane_id,
            workspace_id: workspace,
        })
    }

    fn panes_at_path(&self, directory: &str) -> AdapterResult<Vec<PaneAtPath>> {
        let state = self.enter("panesAtPath", json!([directory]))?;
        Ok(state.strays.get(directory).cloned().unwrap_or_default())
    }

    fn prepare_shell(&self, input: PrepareShellInput<'_>) -> AdapterResult<()> {
        self.record(
            "prepareShell",
            json!([{"paneId": input.pane_id, "environment": strings(input.environment)}]),
        )?;
        Ok(())
    }

    fn start_agent(&self, input: StartAgentInput<'_>) -> AdapterResult<StartStatus> {
        let mut state = self.enter(
            "startAgent",
            json!([{
                "name": input.name,
                "kind": input.kind,
                "paneId": input.pane_id,
                "args": input.args,
                "environment": input.environment.map(strings),
                "timeoutMs": input.timeout_ms,
            }]),
        )?;
        state
            .entries
            .insert(input.pane_id.to_string(), Some(input.name.to_string()));
        state
            .agent_panes
            .insert(input.name.to_string(), input.pane_id.to_string());
        if let Some(token) = input.environment.and_then(|e| e.get("CAPSTAN_TOKEN")) {
            state.tokens.insert(input.name.to_string(), token.clone());
        }
        Ok(state.start_status)
    }

    fn answer_trust_dialog(&self, input: TrustDialogInput<'_>) -> AdapterResult<DialogOutcome> {
        let handled = {
            let state = self.enter(
                "answerTrustDialog",
                json!([{"paneId": input.pane_id, "timeoutMs": input.timeout_ms, "log": "<fn>"}]),
            )?;
            state.dialog_handled
        };
        if !handled {
            return Ok(DialogOutcome::Unhandled {
                reason: "path_mismatch".into(),
            });
        }
        for key in ["down", "enter"] {
            (input.log)(&KeyLogEntry {
                pane: input.pane_id.to_string(),
                key: key.to_string(),
                reason: "trust".into(),
            });
        }
        Ok(DialogOutcome::Handled {
            keys: vec!["down".into(), "enter".into()],
        })
    }

    fn close_pane(&self, pane_id: &str) -> AdapterResult<()> {
        let mut state = self.enter("closePane", json!([pane_id]))?;
        state.entries.remove(pane_id);
        state.tab_panes.retain(|p| p.pane_id != pane_id);
        state.agent_panes.retain(|_, pane| pane != pane_id);
        Ok(())
    }

    fn pane_identity(&self, pane_id: &str) -> AdapterResult<Option<PaneIdentity>> {
        let state = self.enter("paneIdentity", json!([pane_id]))?;
        if state.gone.contains(pane_id) {
            return Ok(None);
        }
        if let Some(listed) = state.identities.get(pane_id) {
            return Ok(Some(listed.clone()));
        }
        let tokens = state
            .metadata
            .iter()
            .rev()
            .find(|(target, _)| target == pane_id)
            .map(|(_, tokens)| tokens);
        Ok(Some(PaneIdentity {
            terminal_id: Some(format!("term:{pane_id}")),
            agent: tokens.and_then(|t| t.get("agent").cloned()),
            project: tokens.and_then(|t| t.get("project").cloned()),
        }))
    }

    fn adopt_pane(&self, input: AdoptPaneInput<'_>) -> AdapterResult<()> {
        let mut state = self.enter(
            "adoptPane",
            json!([{
                "paneId": input.pane_id,
                "role": role_name(input.role),
                "agent": input.agent,
                "workspaceId": input.workspace_id,
                "worktreePath": input.worktree_path,
            }]),
        )?;
        state
            .entries
            .insert(input.pane_id.to_string(), Some(input.agent.to_string()));
        state
            .agent_panes
            .insert(input.agent.to_string(), input.pane_id.to_string());
        Ok(())
    }

    fn adopt_shell_pane(&self, pane_id: &str, workspace_id: Option<&str>) -> AdapterResult<()> {
        let mut state = self.enter("adoptShellPane", json!([pane_id, workspace_id]))?;
        state.entries.insert(pane_id.to_string(), None);
        Ok(())
    }

    fn report_metadata(
        &self,
        target: MetadataTarget<'_>,
        tokens: &BTreeMap<String, String>,
    ) -> AdapterResult<()> {
        let (key, id) = match target {
            MetadataTarget::Pane(id) => ("paneId", id),
            MetadataTarget::Workspace(id) => ("workspaceId", id),
        };
        let mut state = self.enter("reportMetadata", json!([{ key: id }, strings(tokens)]))?;
        state.metadata.push((id.to_string(), tokens.clone()));
        Ok(())
    }

    fn rename_workspace(&self, workspace_id: &str, label: &str) -> AdapterResult<()> {
        self.record("renameWorkspace", json!([workspace_id, label]))?;
        Ok(())
    }

    fn rename_tab(&self, tab_id: &str, label: &str) -> AdapterResult<()> {
        self.record("renameTab", json!([tab_id, label]))?;
        Ok(())
    }

    fn forget_pane(&self, pane_id: &str) {
        let mut state = lock(&self.state);
        let recorded = normal(&json!({"m": "forgetPane", "a": [pane_id]}), &state.scratch);
        state.calls.push(recorded);
        if let Some(failure) = state
            .failures
            .get_mut("forgetPane")
            .and_then(|queue| queue.pop_front())
        {
            panic!("forgetPane cannot fail: {failure:?}");
        }
        state.entries.remove(pane_id);
        state.agent_panes.retain(|_, pane| pane != pane_id);
    }

    fn run_in_pane(&self, pane_id: &str, command: &str) -> AdapterResult<()> {
        self.record("runInPane", json!([pane_id, command]))?;
        Ok(())
    }

    fn write_prompt_file(&self, text: &str) -> AdapterResult<String> {
        let mut state = self.enter("writePromptFile", json!([text]))?;
        state.prompt_count += 1;
        let file = state
            .prompts_dir
            .join(format!("prompt-{}.md", state.prompt_count));
        std::fs::write(&file, text).expect("the prompt file is written");
        Ok(file.to_string_lossy().into_owned())
    }

    fn read_screen(
        &self,
        pane_id: &str,
        _ansi: bool,
        lines: Option<usize>,
    ) -> AdapterResult<String> {
        let state = self.enter("readScreen", json!([pane_id, {"lines": lines}]))?;
        Ok(state.screens.get(pane_id).cloned().unwrap_or_default())
    }

    fn capture_prompt(&self, pane_id: &str) -> AdapterResult<CaptureOutcome> {
        self.record("capturePrompt", json!([pane_id]))?;
        Ok(CaptureOutcome::Refused(RelayRefusal::PromptUnrecognized))
    }

    fn answer_prompt(&self, input: AnswerPromptInput<'_>) -> AdapterResult<RelayOutcome> {
        self.record(
            "answerPrompt",
            json!([{"paneId": input.pane_id, "promptSha": input.prompt_sha}]),
        )?;
        Ok(RelayOutcome::Typed {
            keys: vec!["enter".into()],
            input_readable: None,
        })
    }

    fn interrupt_working(&self, pane_id: &str, log: KeyLogger<'_>) -> AdapterResult<bool> {
        let working = {
            let state = self.enter(
                "interruptWorking",
                json!([{"paneId": pane_id, "log": "<fn>"}]),
            )?;
            state.working.contains(pane_id)
        };
        if !working {
            return Ok(false);
        }
        log(&KeyLogEntry {
            pane: pane_id.to_string(),
            key: "esc".into(),
            reason: "interrupt a paused worker".into(),
        });
        Ok(true)
    }
}
