//! Pane and layout operations: creating, placing, renaming, closing and adopting panes (src/herdr/adapter-panes.ts).

use super::*;
use std::collections::BTreeMap;
use std::path::Component;

struct ListedPane {
    pane_id: String,
    workspace_id: String,
    /// None when Herdr gave no absolute directory for the pane.
    cwd: Option<String>,
}

fn is_absolute(value: &str) -> bool {
    value.starts_with('/')
}

/// `path.resolve` for an absolute path: `.` and `..` and repeated separators removed, no filesystem access.
fn lexical_resolve(value: &str) -> String {
    let mut parts: Vec<String> = Vec::new();
    for component in Path::new(value).components() {
        match component {
            Component::Normal(part) => parts.push(part.to_string_lossy().into_owned()),
            Component::ParentDir => {
                parts.pop();
            }
            _ => {}
        }
    }
    format!("/{}", parts.join("/"))
}

fn trim_slashes(text: &str) -> String {
    let trimmed = text.trim_end_matches('/');
    if trimmed.is_empty() {
        "/".to_string()
    } else {
        trimmed.to_string()
    }
}

/// The real path of a directory, or its lexical form when it does not exist.
fn canonical(value: &str) -> String {
    match std::fs::canonicalize(value) {
        Ok(real) => trim_slashes(&real.to_string_lossy()),
        Err(_) => trim_slashes(&lexical_resolve(value)),
    }
}

impl Adapter {
    pub fn create_worktree_of(
        &self,
        input: &CreateWorktreeInput<'_>,
    ) -> AdapterResult<WorktreeCreated> {
        require_match(input.workspace_id, &WORKSPACE_PATTERN, "workspace id")?;
        require_match(input.branch, &BRANCH_PATTERN, "branch")?;
        require_label(input.label)?;
        let mut args = strings(&[
            "worktree",
            "create",
            "--workspace",
            input.workspace_id,
            "--branch",
            input.branch,
            "--label",
            input.label,
        ]);
        if let Some(base) = input.base {
            args.push("--base".into());
            args.push(require_match(base, &BRANCH_PATTERN, "base")?.to_string());
        }
        args.push("--no-focus".into());
        let result = self.json(&args)?;
        let pane = record(result.get("root_pane"), "root_pane")?;
        let workspace = record(result.get("workspace"), "workspace")?;
        let worktree = record(workspace.get("worktree"), "worktree")?;
        let pane_id = require_match_value(pane.get("pane_id"), &PANE_PATTERN, "pane id")?;
        let workspace_id = require_match_value(
            workspace.get("workspace_id"),
            &WORKSPACE_PATTERN,
            "workspace id",
        )?;
        let checkout = match worktree.get("checkout_path").and_then(Value::as_str) {
            Some(path) if is_absolute(path) => path,
            _ => {
                return Err(herdr_failure(
                    "bad_output",
                    "herdr did not report a checkout path",
                ));
            }
        };
        self.set_entry(
            pane_id,
            PaneEntry {
                role: PaneRole::Worker,
                phase: PanePhase::Fresh,
                kind: "shell".into(),
                agent: None,
                worktree_path: Some(checkout.to_string()),
                workspace_id: Some(workspace_id.to_string()),
            },
        );
        Ok(WorktreeCreated {
            workspace_id: workspace_id.to_string(),
            pane_id: pane_id.to_string(),
            path: checkout.to_string(),
            branch: input.branch.to_string(),
        })
    }

    /// The panes of the tab that holds `pane_id`, with their sizes in terminal cells.
    pub fn pane_layout_of(&self, pane_id: &str) -> AdapterResult<PaneLayoutView> {
        require_match(pane_id, &PANE_PATTERN, "pane id")?;
        let result = self.json(&strings(&["pane", "layout", "--pane", pane_id]))?;
        let layout = record(result.get("layout"), "layout")?;
        let Some(Value::Array(panes)) = layout.get("panes") else {
            return Err(herdr_failure(
                "bad_output",
                "herdr did not report layout panes",
            ));
        };
        let tab_id = require_match_value(layout.get("tab_id"), &TAB_PATTERN, "tab id")?;
        let workspace_id = require_match_value(
            layout.get("workspace_id"),
            &WORKSPACE_PATTERN,
            "workspace id",
        )?;
        // Anything but an explicit false counts as zoomed: no split on a layout we cannot read.
        let zoomed = layout.get("zoomed") != Some(&Value::Bool(false));
        let mut out = Vec::new();
        for entry in panes {
            let pane = record(Some(entry), "layout pane")?;
            let rect = record(pane.get("rect"), "layout rect")?;
            let size = |key: &str| {
                rect.get(key)
                    .and_then(Value::as_f64)
                    .map_or(i64::MIN, |v| v as i64)
            };
            out.push(LayoutPane {
                pane_id: require_match_value(pane.get("pane_id"), &PANE_PATTERN, "pane id")?
                    .to_string(),
                width: size("width"),
                height: size("height"),
            });
        }
        Ok(PaneLayoutView {
            tab_id: tab_id.to_string(),
            workspace_id: workspace_id.to_string(),
            zoomed,
            panes: out,
        })
    }

    fn list_panes(&self) -> AdapterResult<Vec<ListedPane>> {
        let result = self.json(&strings(&["pane", "list"]))?;
        let Some(Value::Array(panes)) = result.get("panes") else {
            return Err(herdr_failure("bad_output", "herdr did not report panes"));
        };
        // A pane whose id this adapter cannot read is skipped: it cannot be ours. A pane without a usable directory is
        // kept, so it still counts as existing.
        let mut listed = Vec::new();
        for entry in panes {
            let parsed = (|| -> AdapterResult<ListedPane> {
                let pane = record(Some(entry), "pane")?;
                let pane_id = require_match_value(pane.get("pane_id"), &PANE_PATTERN, "pane id")?;
                require_match_value(pane.get("tab_id"), &TAB_PATTERN, "tab id")?;
                let workspace_id = require_match_value(
                    pane.get("workspace_id"),
                    &WORKSPACE_PATTERN,
                    "workspace id",
                )?;
                Ok(ListedPane {
                    pane_id: pane_id.to_string(),
                    workspace_id: workspace_id.to_string(),
                    cwd: pane
                        .get("cwd")
                        .and_then(Value::as_str)
                        .filter(|cwd| is_absolute(cwd))
                        .map(str::to_string),
                })
            })();
            if let Ok(pane) = parsed {
                listed.push(pane);
            }
        }
        Ok(listed)
    }

    /// The panes whose working directory is exactly `directory` (compared as real paths), with their workspaces.
    pub fn panes_at_path_of(&self, directory: &str) -> AdapterResult<Vec<PaneAtPath>> {
        if !is_absolute(directory) {
            return Ok(Vec::new());
        }
        let wanted = canonical(directory);
        Ok(self
            .list_panes()?
            .into_iter()
            .filter(|pane| {
                pane.cwd
                    .as_deref()
                    .is_some_and(|cwd| canonical(cwd) == wanted)
            })
            .map(|pane| PaneAtPath {
                pane_id: pane.pane_id,
                workspace_id: pane.workspace_id,
            })
            .collect())
    }

    /// Moves a registered worker pane into another tab as a split. Herdr gives the moved pane a new id; the registry
    /// follows it. A move that errors is ambiguous (Herdr may have done it), so the panes are listed again: the old pane
    /// still there means nothing moved; exactly one new pane at the worktree path means it moved; anything else is an
    /// error.
    pub fn place_pane_of(&self, input: &PlacePaneInput<'_>) -> AdapterResult<PaneAtPath> {
        require_match(input.pane_id, &PANE_PATTERN, "pane id")?;
        require_match(input.tab_id, &TAB_PATTERN, "tab id")?;
        require_match(input.target_pane_id, &PANE_PATTERN, "target pane id")?;
        if !is_absolute(input.worktree_path) {
            return Err(invalid("worktree path must be absolute"));
        }
        if !(input.keep >= 0.1 && input.keep <= 0.9) {
            return Err(invalid("split ratio is not acceptable"));
        }
        let Some(entry) = self.entry(input.pane_id) else {
            return Err(AdapterError::UnknownPane("pane is not registered".into()));
        };
        if entry.role != PaneRole::Worker {
            return Err(AdapterError::Phase(
                "only a worker pane can be placed".into(),
            ));
        }
        let before: Vec<String> = self.list_panes()?.into_iter().map(|p| p.pane_id).collect();
        let direction = match input.direction {
            SplitDirection::Right => "right",
            SplitDirection::Down => "down",
        };
        let attempt = (|| -> AdapterResult<PaneAtPath> {
            let result = self.json(&strings(&[
                "pane",
                "move",
                input.pane_id,
                "--tab",
                input.tab_id,
                "--split",
                direction,
                "--target-pane",
                input.target_pane_id,
                "--ratio",
                &input.keep.to_string(),
                "--no-focus",
            ]))?;
            let move_result = record(result.get("move_result"), "move_result")?;
            let pane = record(move_result.get("pane"), "pane")?;
            Ok(PaneAtPath {
                pane_id: require_match_value(pane.get("pane_id"), &PANE_PATTERN, "pane id")?
                    .to_string(),
                workspace_id: require_match_value(
                    pane.get("workspace_id"),
                    &WORKSPACE_PATTERN,
                    "workspace id",
                )?
                .to_string(),
            })
        })();
        let moved = match attempt {
            Ok(moved) => moved,
            Err(error) => {
                let Ok(after) = self.list_panes() else {
                    return Err(AdapterError::PaneLost(
                        "the pane move failed and the panes could not be listed to check it".into(),
                    ));
                };
                if after.iter().any(|p| p.pane_id == input.pane_id) {
                    return Err(error);
                }
                let wanted = canonical(input.worktree_path);
                let found: Vec<&ListedPane> = after
                    .iter()
                    .filter(|p| {
                        !before.contains(&p.pane_id)
                            && p.cwd.as_deref().is_some_and(|cwd| canonical(cwd) == wanted)
                    })
                    .collect();
                if found.len() != 1 {
                    return Err(AdapterError::PaneLost(
                        "the pane move failed and the pane cannot be found again".into(),
                    ));
                }
                PaneAtPath {
                    pane_id: found[0].pane_id.clone(),
                    workspace_id: found[0].workspace_id.clone(),
                }
            }
        };
        self.delete_entry(input.pane_id);
        self.set_entry(
            &moved.pane_id,
            PaneEntry {
                workspace_id: Some(moved.workspace_id.clone()),
                ..entry
            },
        );
        Ok(moved)
    }

    pub fn create_workspace_of(
        &self,
        input: &CreateWorkspaceInput<'_>,
    ) -> AdapterResult<WorkspaceCreated> {
        if !is_absolute(input.cwd) {
            return Err(invalid("workspace directory must be absolute"));
        }
        require_label(input.label)?;
        let result = self.json(&strings(&[
            "workspace",
            "create",
            "--cwd",
            input.cwd,
            "--label",
            input.label,
            "--no-focus",
        ]))?;
        let pane = record(result.get("root_pane"), "root_pane")?;
        let workspace = record(result.get("workspace"), "workspace")?;
        let pane_id = require_match_value(pane.get("pane_id"), &PANE_PATTERN, "pane id")?;
        let workspace_id = require_match_value(
            workspace.get("workspace_id"),
            &WORKSPACE_PATTERN,
            "workspace id",
        )?;
        let tab_id = require_match_value(
            record(result.get("tab"), "tab")?.get("tab_id"),
            &TAB_PATTERN,
            "tab id",
        )?;
        self.set_entry(
            pane_id,
            PaneEntry {
                role: input.role,
                phase: PanePhase::Fresh,
                kind: "shell".into(),
                agent: None,
                worktree_path: None,
                workspace_id: Some(workspace_id.to_string()),
            },
        );
        Ok(WorkspaceCreated {
            workspace_id: workspace_id.to_string(),
            pane_id: pane_id.to_string(),
            tab_id: tab_id.to_string(),
        })
    }

    /// A new tab with its own root pane inside an existing workspace.
    pub fn create_tab_of(&self, input: &CreateTabInput<'_>) -> AdapterResult<TabCreated> {
        require_match(input.workspace_id, &WORKSPACE_PATTERN, "workspace id")?;
        if !is_absolute(input.cwd) {
            return Err(invalid("tab directory must be absolute"));
        }
        require_label(input.label)?;
        let result = self.json(&strings(&[
            "tab",
            "create",
            "--workspace",
            input.workspace_id,
            "--cwd",
            input.cwd,
            "--label",
            input.label,
            "--no-focus",
        ]))?;
        let pane = record(result.get("root_pane"), "root_pane")?;
        let tab = record(result.get("tab"), "tab")?;
        let pane_id = require_match_value(pane.get("pane_id"), &PANE_PATTERN, "pane id")?;
        let tab_id = require_match_value(tab.get("tab_id"), &TAB_PATTERN, "tab id")?;
        self.set_entry(
            pane_id,
            PaneEntry {
                role: input.role,
                phase: PanePhase::Fresh,
                kind: "shell".into(),
                agent: None,
                worktree_path: None,
                workspace_id: Some(input.workspace_id.to_string()),
            },
        );
        Ok(TabCreated {
            tab_id: tab_id.to_string(),
            pane_id: pane_id.to_string(),
        })
    }

    pub fn remove_worktree_of(&self, workspace_id: &str, force: bool) -> AdapterResult<()> {
        require_match(workspace_id, &WORKSPACE_PATTERN, "workspace id")?;
        // A worker placed as a pane shares the PM's workspace id; removing "its" worktree workspace would reach the PM.
        for (_, entry) in self.entries() {
            if entry.workspace_id.as_deref() == Some(workspace_id) && entry.role == PaneRole::Pm {
                return Err(AdapterError::Phase("that workspace holds the PM".into()));
            }
        }
        let mut args = strings(&["worktree", "remove", "--workspace", workspace_id]);
        if force {
            args.push("--force".into());
        }
        self.json(&args)?;
        for (pane_id, entry) in self.entries() {
            if entry.workspace_id.as_deref() == Some(workspace_id) && entry.role == PaneRole::Worker
            {
                self.delete_entry(&pane_id);
            }
        }
        Ok(())
    }

    /// Display-only metadata for the operator's sidebar; the caller treats a failure as non-fatal.
    pub fn report_metadata_of(
        &self,
        target: MetadataTarget<'_>,
        tokens: &BTreeMap<String, String>,
    ) -> AdapterResult<()> {
        let (kind, id) = match target {
            MetadataTarget::Pane(pane_id) => {
                ("pane", require_match(pane_id, &PANE_PATTERN, "pane id")?)
            }
            MetadataTarget::Workspace(workspace_id) => (
                "workspace",
                require_match(workspace_id, &WORKSPACE_PATTERN, "workspace id")?,
            ),
        };
        let mut args = strings(&[kind, "report-metadata", id, "--source", "capstan"]);
        // The map is sorted, Node keeps insertion order; the launcher's only calls send `project`, `role`, `agent` in
        // that order, so those come first and any other token follows sorted.
        let order = ["project", "role", "agent"];
        let known = order.iter().filter_map(|n| tokens.get_key_value(*n));
        let others = tokens.iter().filter(|(n, _)| !order.contains(&n.as_str()));
        for (name, value) in known.chain(others) {
            require_match(name, &TOKEN_NAME, "token name")?;
            args.push("--token".into());
            args.push(format!("{name}={}", require_label(value)?));
        }
        self.run_checked(&args)
    }

    /// Names a tab; display only.
    pub fn rename_tab_of(&self, tab_id: &str, label: &str) -> AdapterResult<()> {
        require_match(tab_id, &TAB_PATTERN, "tab id")?;
        self.run_checked(&strings(&["tab", "rename", tab_id, require_label(label)?]))
    }

    /// Gives an existing workspace its current label (an upgraded project still holds the old one).
    pub fn rename_workspace_of(&self, workspace_id: &str, label: &str) -> AdapterResult<()> {
        require_match(workspace_id, &WORKSPACE_PATTERN, "workspace id")?;
        self.run_checked(&strings(&[
            "workspace",
            "rename",
            workspace_id,
            require_label(label)?,
        ]))
    }

    /// Who a pane belongs to as Herdr shows it now: its terminal id and the `agent` and `project` tokens Capstan
    /// reported for it. None when Herdr has no such pane. Herdr reuses short pane ids, so this is what tells an agent's
    /// pane from a newer pane that was given the same id.
    pub fn pane_identity_of(&self, pane_id: &str) -> AdapterResult<Option<PaneIdentity>> {
        require_match(pane_id, &PANE_PATTERN, "pane id")?;
        let result = match self.json(&strings(&["pane", "get", pane_id])) {
            Ok(result) => result,
            Err(error) if is_not_found(&error.code) => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        let pane = record(result.get("pane"), "pane")?;
        let text = |value: Option<&Value>| {
            value
                .and_then(Value::as_str)
                .filter(|text| !text.is_empty())
                .map(str::to_string)
        };
        let tokens = pane.get("tokens").and_then(Value::as_object);
        Ok(Some(PaneIdentity {
            terminal_id: text(pane.get("terminal_id")),
            agent: text(tokens.and_then(|t| t.get("agent"))),
            project: text(tokens.and_then(|t| t.get("project"))),
        }))
    }

    pub fn close_pane_of(&self, pane_id: &str) -> AdapterResult<()> {
        require_match(pane_id, &PANE_PATTERN, "pane id")?;
        self.run_checked(&strings(&["pane", "close", pane_id]))?;
        self.delete_entry(pane_id);
        Ok(())
    }

    /// Registers a pane an earlier adapter instance created (a daemon that restarted), after Herdr confirms the pane
    /// exists and that the agent name still points at it. Nothing is registered otherwise.
    pub fn adopt_pane_of(&self, input: &AdoptPaneInput<'_>) -> AdapterResult<()> {
        require_match(input.pane_id, &PANE_PATTERN, "pane id")?;
        if !is_agent_name(input.agent) {
            return Err(invalid("agent name is not acceptable"));
        }
        if self.entry(input.pane_id).is_some() {
            return Err(AdapterError::Phase("the pane is already registered".into()));
        }
        self.assert_pane_exists(input.pane_id)?;
        let state = match self.agent_state_of(input.agent) {
            Ok(state) => state,
            Err(AdapterError::Herdr(error))
                if self.slug().is_some() && error.code == "agent_not_found" =>
            {
                // An agent started before names carried the project is still known by its bare id: name it the new way
                // when it is on the recorded pane.
                let legacy = self.agent_state_by_herdr_name(input.agent)?;
                if legacy.pane_id != input.pane_id {
                    return Err(AdapterError::AgentPaneMismatch(
                        "the agent name points at another pane than the recorded one".into(),
                    ));
                }
                self.run_checked(&strings(&[
                    "agent",
                    "rename",
                    input.pane_id,
                    &self.herdr_name(input.agent)?,
                ]))?;
                self.agent_state_of(input.agent)?
            }
            Err(error) => return Err(error),
        };
        if state.pane_id != input.pane_id {
            return Err(AdapterError::AgentPaneMismatch(
                "the agent name points at another pane than the recorded one".into(),
            ));
        }
        if !HOST_KINDS.contains(&state.kind.as_str()) {
            return Err(AdapterError::UnsupportedHost(format!(
                "the agent is a {} agent, which this adapter does not drive",
                state.kind
            )));
        }
        self.set_entry(
            input.pane_id,
            PaneEntry {
                role: input.role,
                phase: PanePhase::Started,
                kind: state.kind,
                agent: Some(input.agent.to_string()),
                worktree_path: input.worktree_path.map(str::to_string),
                workspace_id: input.workspace_id.map(str::to_string),
            },
        );
        Ok(())
    }

    /// Registers the fallback watch pane of an earlier instance; it never takes input from the adapter again.
    pub fn adopt_shell_pane_of(
        &self,
        pane_id: &str,
        workspace_id: Option<&str>,
    ) -> AdapterResult<()> {
        require_match(pane_id, &PANE_PATTERN, "pane id")?;
        self.assert_pane_exists(pane_id)?;
        self.set_entry(
            pane_id,
            PaneEntry {
                role: PaneRole::Worker,
                phase: PanePhase::Started,
                kind: "shell".into(),
                agent: None,
                worktree_path: None,
                workspace_id: workspace_id.map(str::to_string),
            },
        );
        Ok(())
    }

    fn assert_pane_exists(&self, pane_id: &str) -> AdapterResult<()> {
        match self.json(&strings(&["pane", "get", pane_id])) {
            Ok(_) => Ok(()),
            Err(error) if is_not_found(&error.code) => {
                Err(AdapterError::PaneGone("Herdr has no such pane".into()))
            }
            Err(error) => Err(error.into()),
        }
    }
}

/// `/not_found|no_such/.test(code)`.
fn is_not_found(code: &str) -> bool {
    code.contains("not_found") || code.contains("no_such")
}
