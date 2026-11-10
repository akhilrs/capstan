//! A worker tab opened at a change folder keeps that folder as the directory its trust dialog may be answered for; a PM tab
//! does not.
use capstan_herdr::adapter::{Adapter, AdapterOptions};
use capstan_herdr::api::{
    CreateTabInput, HerdrError, HerdrOutput, HerdrRunner, LauncherAdapter, PaneRegistry, PaneRole,
    RunOptions,
};
use serde_json::json;
use std::sync::Arc;

struct Runner;

impl HerdrRunner for Runner {
    fn run(&self, args: &[String], _options: &RunOptions) -> Result<HerdrOutput, HerdrError> {
        let body = match (
            args.first().map(String::as_str),
            args.get(1).map(String::as_str),
        ) {
            (Some("tab"), Some("create")) => json!({"result": {
                "tab": {"tab_id": "w1:t2"},
                "root_pane": {"pane_id": "w1:p2"},
            }}),
            _ => json!({"result": {}}),
        };
        Ok(HerdrOutput {
            code: 0,
            stdout: body.to_string(),
            stderr: String::new(),
        })
    }
}

fn open(role: PaneRole, record_folder: bool) -> Option<String> {
    let adapter = Adapter::new(AdapterOptions::new(Arc::new(Runner)));
    let tab = adapter
        .create_tab(CreateTabInput {
            workspace_id: "w1",
            cwd: "/work/changes/developer-1",
            label: "developer-1",
            role,
            record_folder,
        })
        .unwrap();
    adapter.pane_entry(&tab.pane_id).unwrap().worktree_path
}

#[test]
fn a_worker_tab_records_its_folder_and_a_pm_tab_none() {
    assert_eq!(
        open(PaneRole::Worker, true).as_deref(),
        Some("/work/changes/developer-1")
    );
    assert_eq!(open(PaneRole::Pm, false), None);
}

#[test]
fn a_watch_tab_with_a_worker_role_records_no_folder() {
    assert_eq!(open(PaneRole::Worker, false), None);
}
