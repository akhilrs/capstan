//! A fresh pane whose shell draws its own prompt (nushell, starship, a default bash) is typed into only while Herdr shows
//! that shell alone in the foreground, and the line typed into it needs no shell quoting.
use capstan_herdr::adapter::{Adapter, AdapterOptions};
use capstan_herdr::api::{
    AdapterError, CreateTabInput, HerdrError, HerdrOutput, HerdrRunner, LauncherAdapter, PaneRole,
    PrepareShellInput, RunOptions,
};
use serde_json::json;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

const IDLE_NU: &str = r#"{"result":{"process_info":{"shell_pid":83658,"foreground_process_group_id":83658,
    "foreground_processes":[{"pid":83658,"name":"nu"}]}}}"#;
const CHILD_RUNNING: &str = r#"{"result":{"process_info":{"shell_pid":83658,"foreground_process_group_id":83870,
    "foreground_processes":[{"pid":83870,"name":"sleep"}]}}}"#;

/// Answers like Herdr for one pane: `screen` until a line is typed, then the prepared bash's `❯ `.
struct Runner {
    screen: String,
    process_info: &'static str,
    calls: Mutex<Vec<Vec<String>>>,
    launch_text: Mutex<Option<String>>,
}

impl Runner {
    fn new(screen: &str, process_info: &'static str) -> Arc<Self> {
        Arc::new(Self {
            screen: screen.into(),
            process_info,
            calls: Mutex::new(Vec::new()),
            launch_text: Mutex::new(None),
        })
    }

    fn calls_of(&self, first: &str, second: &str) -> Vec<Vec<String>> {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|args| args.first().map(String::as_str) == Some(first))
            .filter(|args| args.get(1).map(String::as_str) == Some(second))
            .cloned()
            .collect()
    }

    fn typed(&self) -> bool {
        !self.calls_of("pane", "run").is_empty()
    }
}

impl HerdrRunner for Runner {
    fn run(&self, args: &[String], _options: &RunOptions) -> Result<HerdrOutput, HerdrError> {
        let typed = self.typed();
        self.calls.lock().unwrap().push(args.to_vec());
        let stdout = match (args[0].as_str(), args[1].as_str()) {
            ("tab", "create") => json!({"result": {
                "tab": {"tab_id": "w1:t2"},
                "root_pane": {"pane_id": "w1:p2"},
            }})
            .to_string(),
            ("pane", "read") if typed => "❯ ".into(),
            ("pane", "read") => self.screen.clone(),
            ("pane", "process-info") => self.process_info.into(),
            ("pane", "run") => {
                // /bin/sh reads the launch file, and the rc it names removes the private directory.
                if let Some(path) = args[3].strip_prefix("exec /bin/sh ") {
                    let path = PathBuf::from(path);
                    *self.launch_text.lock().unwrap() = std::fs::read_to_string(&path).ok();
                    let _ = std::fs::remove_dir_all(path.parent().unwrap());
                }
                String::new()
            }
            _ => json!({"result": {}}).to_string(),
        };
        Ok(HerdrOutput {
            code: 0,
            stdout,
            stderr: String::new(),
        })
    }
}

fn adapter(runner: &Arc<Runner>, temp_root: PathBuf) -> (Adapter, String) {
    let clock = Arc::new(AtomicU64::new(0));
    let now_clock = Arc::clone(&clock);
    let mut options = AdapterOptions::new(Arc::clone(runner) as Arc<dyn HerdrRunner>);
    options.temp_root = Some(temp_root);
    options.poll_ms = Some(100);
    options.now = Some(Arc::new(move || now_clock.load(Ordering::SeqCst)));
    options.sleep = Some(Arc::new(move |ms| {
        clock.fetch_add(ms, Ordering::SeqCst);
    }));
    let adapter = Adapter::new(options);
    let tab = adapter
        .create_tab(CreateTabInput {
            workspace_id: "w1",
            cwd: "/work",
            label: "watch",
            role: PaneRole::Worker,
            record_folder: false,
        })
        .unwrap();
    (adapter, tab.pane_id)
}

fn run_in_fresh_pane(
    screen: &str,
    process_info: &'static str,
) -> (Arc<Runner>, Result<(), AdapterError>) {
    let runner = Runner::new(screen, process_info);
    let directory = tempfile::tempdir().unwrap();
    let (adapter, pane_id) = adapter(&runner, directory.path().to_path_buf());
    let result = adapter.run_in_pane(&pane_id, "cstan watch");
    (runner, result)
}

#[test]
fn a_nushell_prompt_with_the_shell_in_the_foreground_is_typed_into() {
    let (runner, result) = run_in_fresh_pane("Tanka on  main [!?]\n❯ : ", IDLE_NU);
    result.unwrap();
    assert_eq!(
        runner.calls_of("pane", "run"),
        vec![vec![
            "pane".to_string(),
            "run".into(),
            "w1:p2".into(),
            "cstan watch".into()
        ]]
    );
    assert_eq!(
        runner.calls_of("pane", "process-info"),
        vec![vec![
            "pane".to_string(),
            "process-info".into(),
            "--pane".into(),
            "w1:p2".into()
        ]]
    );
}

#[test]
fn a_prompt_line_while_a_program_holds_the_terminal_is_refused() {
    let (runner, result) = run_in_fresh_pane("❯ : ", CHILD_RUNNING);
    assert!(
        matches!(result, Err(AdapterError::PromptUnrecognized(_))),
        "{result:?}"
    );
    assert!(!runner.typed());
}

#[test]
fn a_shell_question_is_refused_without_asking_herdr() {
    let (runner, result) = run_in_fresh_pane("Password: ", IDLE_NU);
    assert!(
        matches!(result, Err(AdapterError::PromptUnrecognized(_))),
        "{result:?}"
    );
    assert!(!runner.typed());
    assert!(runner.calls_of("pane", "process-info").is_empty());
}

#[test]
fn a_bare_prompt_symbol_needs_no_process_info() {
    let (runner, result) = run_in_fresh_pane("❯ ", CHILD_RUNNING);
    result.unwrap();
    assert!(runner.calls_of("pane", "process-info").is_empty());
}

#[test]
fn the_prepared_shell_is_started_by_a_line_any_shell_runs() {
    let runner = Runner::new("~/Workspace/Tanka> ", IDLE_NU);
    let directory = tempfile::tempdir().unwrap();
    let (adapter, pane_id) = adapter(&runner, directory.path().to_path_buf());
    let environment: BTreeMap<String, String> = [
        ("HOME", "/home/o'neil x"),
        ("PATH", "/usr/bin:/bin"),
        ("TERM", "xterm-256color"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .collect();
    adapter
        .prepare_shell(PrepareShellInput {
            pane_id: &pane_id,
            environment: &environment,
            timeout_ms: None,
        })
        .unwrap();

    let runs = runner.calls_of("pane", "run");
    assert_eq!(runs.len(), 1);
    let typed = &runs[0][3];
    let launch = typed
        .strip_prefix("exec /bin/sh ")
        .expect("the typed line runs /bin/sh");
    assert!(launch.ends_with("/launch"), "{typed}");
    assert!(
        !typed.contains(['\'', '"', '\\']) && typed.split(' ').count() == 3,
        "the typed line needs no quoting: {typed}"
    );
    let rc = format!("{}/rc", launch.strip_suffix("/launch").unwrap());
    assert_eq!(
        runner.launch_text.lock().unwrap().as_deref(),
        Some(
            format!(
                "exec env -i HOME='/home/o'\\''neil x' PATH='/usr/bin:/bin' TERM='xterm-256color' bash --noprofile \
                 --rcfile '{rc}' -i\n"
            )
            .as_str()
        )
    );
}
