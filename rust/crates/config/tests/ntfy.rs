//! The `[ntfy]` section: defaults, every key and rule, the per-event styles, the JSON and the secret reader.
mod common;

use capstan_config::*;
use common::{load_trusted, temp_store};
use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::Path;
use std::str::FromStr;

const BASE: &str = "schema_version = 1\n[nexora]\ntrack = \"never\"\n[hosts.claude]\nkind = \"claude\"\n[roles.pm]\nkind = \"PM\"\nhost = \"claude\"\n";

fn load(extra: &str) -> std::result::Result<RoleConfig, String> {
    let text = format!("{BASE}{extra}");
    parse_config(text.as_bytes(), Path::new("/nonexistent")).map_err(|e| match e {
        ConfigError::Invalid(m) => m,
        other => format!("{other:?}"),
    })
}

fn on(body: &str) -> String {
    format!("[ntfy]\nenabled = true\nserver = \"https://h.example\"\ntopic = \"t\"\n{body}")
}

fn refused(extra: &str) -> String {
    load(extra).expect_err(extra)
}

#[test]
fn absent_section_is_default_and_bad_one_fails_the_full_path() {
    assert_eq!(load("").unwrap().ntfy, Ntfy::default());
    assert!(!Ntfy::default().enabled);
    assert!(
        refused("[ntfy]\nenabled = true\nserver = \"https://h.example\"\n").contains("ntfy.topic")
    );
    let dir = tempfile::tempdir().unwrap();
    let (_holder, store) = temp_store();
    let file = dir.path().join("capstan.toml");
    std::fs::write(&file, BASE).unwrap();
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
    // The connection keys are honoured only from the untracked local file.
    let local = dir.path().join("capstan.local.toml");
    std::fs::write(&local, on("retries = 4\n")).unwrap();
    std::fs::set_permissions(&local, std::fs::Permissions::from_mode(0o600)).unwrap();
    let config = load_trusted(dir.path(), &store).unwrap();
    assert_eq!(config.ntfy.retries, 4);
    assert_eq!(config.ntfy.server, "https://h.example");
}

#[test]
fn server_rules() {
    for bad in [
        "http://localhost.evil.example",
        "http://127.0.0.1.evil.example",
        "http://u:p@h.example",
        "https://h.example/?x",
        "https://h.example/#y",
        "http://h.example",
        "http://[::2]",
        "ftp://h.example",
    ] {
        let e = refused(&format!("[ntfy]\nserver = \"{bad}\"\n"));
        assert!(e.contains("ntfy.server"), "{bad}: {e}");
    }
    let ok = |s: &str| {
        load(&format!("[ntfy]\nserver = \"{s}\"\n"))
            .unwrap()
            .ntfy
            .server
    };
    assert_eq!(ok("http://[::1]:8080"), "http://[::1]:8080");
    assert_eq!(ok("http://localhost:80"), "http://localhost:80");
    assert_eq!(ok("https://h.example/ntfy/"), "https://h.example/ntfy");
}

#[test]
fn secret_keys_are_refused_without_echo() {
    for key in ["token", "password", "secret"] {
        let e = refused(&format!("[ntfy]\n{key} = \"tk_SENTINEL\"\n"));
        assert!(e.contains("token_env") && e.contains("token_file"), "{e}");
        assert!(!e.contains("tk_SENTINEL"));
        let e = refused(&format!(
            "[ntfy.event.user_question]\n{key} = \"tk_SENTINEL\"\n"
        ));
        assert!(e.contains("token_env") && !e.contains("tk_SENTINEL"), "{e}");
    }
}

#[test]
fn auth_rules() {
    assert!(refused(&on("token_env = \"A\"\ntoken_file = \"/x\"\n")).contains("one authentication"));
    assert!(refused(&on(
        "token_env = \"A\"\nuser = \"u\"\npassword_env = \"B\"\n"
    ))
    .contains("one authentication"));
    assert!(refused(&on("token_env = \"lower\"\n")).contains("^[A-Z_][A-Z0-9_]*$"));
    assert!(refused(&on("user = \"u\"\n")).contains("ntfy.user"));
    assert!(refused(&on("password_env = \"P\"\n")).contains("ntfy.user"));
    assert!(refused(&on("user = \"bad user\"\npassword_env = \"P\"\n")).contains("ntfy.user"));
    for (key, value) in [("token_env", "TOK"), ("password_env", "PW")] {
        let extra = format!(
            "[env]\npass = [\"{value}\"]\n{}",
            on(&format!("user = \"u\"\n{key} = \"{value}\"\n"))
        );
        let extra = if key == "token_env" {
            extra.replace("user = \"u\"\n", "")
        } else {
            extra
        };
        let e = refused(&extra);
        assert!(e.contains("[env] pass") && e.contains(key), "{e}");
    }
    let c = load(&on("user = \"u@x\"\npassword_env = \"PW\"\n"))
        .unwrap()
        .ntfy;
    assert_eq!(
        c.auth,
        NtfyAuth::BasicEnv {
            user: "u@x".into(),
            password_env: "PW".into()
        }
    );
    for token in ["token_env = \"A\"\n", "token_file = \"/x\"\n"] {
        let e = refused(&on(&format!("user = \"u\"\n{token}")));
        assert!(e.contains("ntfy.user"), "{e}");
    }
    assert_eq!(load(&on("")).unwrap().ntfy.auth, NtfyAuth::None);
}

#[test]
fn value_rules() {
    assert!(refused("[ntfy]\nzz = 1\n").contains("topic"));
    assert!(refused("[ntfy.event.nope]\npriority = 1\n").contains("user_question"));
    assert!(refused("[ntfy.event.test]\npriority = 1\n").contains("allowed"));
    assert!(refused("[ntfy]\nevents = [\"test\"]\n").contains("ntfy.events[0]"));
    assert!(refused("[ntfy]\nevents = [\"nope\"]\n").contains("agent_lost"));
    assert!(
        refused("[ntfy.event.user_question]\nclick = \"http://x.example\"\n").contains("https://")
    );
    assert!(
        refused("[ntfy.event.user_question]\ntags = [\"a\",\"b\",\"c\",\"d\",\"e\",\"f\"]\n")
            .contains("5")
    );
    assert!(refused("[ntfy.event.user_question]\ntags = [\"a b\"]\n").contains("tags[0]"));
    assert!(refused("[ntfy.event.user_question]\npriority = \"max\"\n").contains("priority"));
    assert!(refused("[ntfy.event.user_question]\npriority = 6\n").contains("priority"));
    for ok in ["\"urgent\"", "5"] {
        load(&format!("[ntfy.event.user_question]\npriority = {ok}\n")).unwrap();
    }
    assert!(
        refused(&format!("[ntfy]\ntitle_prefix = \"{}\"\n", "x".repeat(41)))
            .contains("title_prefix")
    );
    assert!(refused("[ntfy]\ntitle_prefix = \"a\\nb\"\n").contains("title_prefix"));
    load(&format!("[ntfy]\ntitle_prefix = \"{}\"\n", "x".repeat(40))).unwrap();
    for bad in ["0", "61"] {
        assert!(refused(&format!("[ntfy]\ntimeout_seconds = {bad}\n")).contains("timeout_seconds"));
    }
    assert!(refused("[ntfy]\nretries = 6\n").contains("retries"));
    assert!(refused("[ntfy]\nca_file = \"rel.pem\"\n").contains("absolute"));
    assert!(refused("[ntfy]\ntopic = \"a b\"\n").contains("ntfy.topic"));
}

#[test]
fn existence_checks_only_when_enabled() {
    let off = "[ntfy]\nca_file = \"/nonexistent/ca.pem\"\ntoken_file = \"/nonexistent/tok\"\n";
    load(off).unwrap();
    let e = refused(&on("ca_file = \"/nonexistent/ca.pem\"\n"));
    assert!(e.contains("ntfy.ca_file"), "{e}");
    let e = refused(&on("token_file = \"/nonexistent/tok\"\n"));
    assert!(e.contains("ntfy.token_file"), "{e}");
    let e = refused(&on("user = \"u\"\npassword_file = \"/nonexistent/pw\"\n"));
    assert!(e.contains("ntfy.password_file"), "{e}");
}

#[test]
fn defaults_and_partial_tables() {
    use NtfyEvent::*;
    let high = |tag: &str| NtfyEventStyle {
        priority: 4,
        tags: vec![tag.into()],
        click: None,
    };
    let default = |tag: &str| NtfyEventStyle {
        priority: 3,
        tags: vec![tag.into()],
        click: None,
    };
    for (e, want) in [
        (UserQuestion, high("question")),
        (OperatorProposal, high("question")),
        (PromptWaiting, high("question")),
        (AgentBlocked, high("warning")),
        (AgentLost, high("warning")),
        (PmStuck, high("warning")),
        (IntegrationConflicted, high("warning")),
        (DesignWaiting, default("art")),
        (PlanSignedOff, default("white_check_mark")),
        (AgentStalled, default("hourglass")),
        (VerifyFailed, default("x")),
        (
            Test,
            NtfyEventStyle {
                priority: 2,
                tags: vec!["test".into()],
                click: None,
            },
        ),
    ] {
        assert_eq!(NtfyEventStyle::default_for(e), want, "{e:?}");
    }
    assert_eq!(NTFY_EVENTS.len(), 11);
    assert_eq!(NTFY_EVENTS.last(), Some(&"pm_stuck"));
    assert!(Ntfy::default().events.contains(&PmStuck));
    for name in NTFY_EVENTS {
        assert_eq!(NtfyEvent::from_str(name).unwrap().as_str(), *name);
    }
    let c = load("[ntfy.event.agent_lost]\npriority = \"urgent\"\n")
        .unwrap()
        .ntfy;
    let s = c.style(AgentLost);
    assert_eq!((s.priority, s.tags), (5, vec!["warning".to_string()]));
    assert_eq!(c.style(UserQuestion), high("question"));
    let c = load("[ntfy.event.agent_lost]\ntags = [\"x\"]\nclick = \"https://c.example/p\"\n")
        .unwrap()
        .ntfy;
    let s = c.style(AgentLost);
    assert_eq!(
        (s.priority, s.click.as_deref()),
        (4, Some("https://c.example/p"))
    );
}

#[test]
fn json_holds_names_and_paths_only() {
    std::env::set_var("NTFY_JSON_TOK", "tok_SENTINEL");
    let dir = tempfile::tempdir().unwrap();
    let ca = dir.path().join("ca.pem");
    std::fs::write(&ca, "x").unwrap();
    let forms = [
        "token_env = \"NTFY_JSON_TOK\"\n".to_string(),
        "token_file = \"/etc/ntfy-token\"\n".to_string(),
        "user = \"u\"\npassword_env = \"NTFY_JSON_TOK\"\n".to_string(),
        "user = \"u\"\npassword_file = \"/etc/ntfy-pw\"\n".to_string(),
    ];
    for form in forms {
        let body = format!("{form}ca_file = \"{}\"\n", ca.display());
        let text = format!("[ntfy]\nserver = \"https://h.example\"\ntopic = \"t\"\n{body}");
        let json = load(&text).unwrap().to_json();
        assert!(json.contains("\"ntfy\"") && json.contains(&ca.display().to_string()));
        assert!(!json.contains("tok_SENTINEL"));
    }
    assert!(!load("").unwrap().to_json().contains("ntfy"));
}

fn file(dir: &Path, name: &str, mode: u32, body: &str) -> std::path::PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, body).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
    path
}

fn facts(mode: u32, uid: u32, symlink: bool) -> capstan_config::ntfy::SecretFileFacts {
    capstan_config::ntfy::SecretFileFacts {
        path: "/p".into(),
        is_symlink: symlink,
        is_regular: !symlink,
        uid,
        mode,
    }
}

#[test]
fn check_secret_file_rules() {
    assert!(check_secret_file(&facts(0o600, 7, false), 7).is_ok());
    for mode in [0o640, 0o604, 0o700, 0o400] {
        assert!(
            matches!(
                check_secret_file(&facts(mode, 7, false), 7),
                Err(SecretError::BadMode(_))
            ),
            "{mode:o}"
        );
    }
    assert!(matches!(
        check_secret_file(&facts(0o600, 8, false), 7),
        Err(SecretError::NotOwner(_))
    ));
    assert!(matches!(
        check_secret_file(&facts(0o600, 7, true), 7),
        Err(SecretError::Symlink(_))
    ));
}

#[test]
fn read_secret_reads_files_anew_and_refuses_bad_ones() {
    let dir = tempfile::tempdir().unwrap();
    let no_env = |_: &str| None;
    let good = file(dir.path(), "good", 0o600, "  s3cret\n");
    let auth = NtfyAuth::TokenFile(good.clone());
    let got = read_secret(&auth, &no_env).unwrap().unwrap();
    assert_eq!(got.expose(), "s3cret");
    assert_eq!(format!("{got:?}"), "[redacted]");
    assert_eq!(format!("{got}"), "[redacted]");
    std::fs::write(&good, "changed").unwrap();
    assert_eq!(
        read_secret(&auth, &no_env).unwrap().unwrap().expose(),
        "changed"
    );

    let loose = file(dir.path(), "loose", 0o640, "s3cret");
    let e = read_secret(&NtfyAuth::TokenFile(loose.clone()), &no_env).unwrap_err();
    assert!(matches!(e, SecretError::BadMode(_)));
    assert!(e.to_string().contains(loose.to_str().unwrap()) && !e.to_string().contains("s3cret"));
    let link = dir.path().join("link");
    symlink(&good, &link).unwrap();
    let e = read_secret(&NtfyAuth::TokenFile(link.clone()), &no_env).unwrap_err();
    assert!(matches!(e, SecretError::Symlink(_)));
    assert!(e.to_string().contains(link.to_str().unwrap()));
    let gone = dir.path().join("gone");
    assert!(matches!(
        read_secret(&NtfyAuth::TokenFile(gone), &no_env),
        Err(SecretError::MissingFile(_))
    ));
    let empty = file(dir.path(), "empty", 0o600, " \n");
    assert_eq!(
        read_secret(&NtfyAuth::TokenFile(empty), &no_env),
        Err(SecretError::Empty)
    );
}

#[test]
fn read_secret_from_env() {
    let env = |name: &str| (name == "SET").then(|| " v \n".to_string());
    assert_eq!(read_secret(&NtfyAuth::None, &env), Ok(None));
    let got = read_secret(&NtfyAuth::TokenEnv("SET".into()), &env)
        .unwrap()
        .unwrap();
    assert_eq!(got.expose(), "v");
    let e = read_secret(
        &NtfyAuth::BasicEnv {
            user: "u".into(),
            password_env: "NOPE".into(),
        },
        &env,
    )
    .unwrap_err();
    assert_eq!(e, SecretError::MissingEnv("NOPE".into()));
    assert!(e.to_string().contains("NOPE"));
    let long = |_: &str| Some("x".repeat(4097));
    assert_eq!(
        read_secret(&NtfyAuth::TokenEnv("A".into()), &long),
        Err(SecretError::Empty)
    );
}

#[test]
fn pm_stuck_minutes_is_off_or_two_to_two_hundred_forty() {
    assert_eq!(load("").unwrap().notifications.pm_stuck_minutes, 15);
    for bad in ["1", "241", "-1", "\"15\"", "1.5"] {
        let text = refused(&format!("[notifications]\npm_stuck_minutes = {bad}\n"));
        assert!(text.contains("pm_stuck_minutes"), "{bad}: {text}");
    }
    for good in [0i64, 2, 15, 240] {
        let c = load(&format!("[notifications]\npm_stuck_minutes = {good}\n")).unwrap();
        assert_eq!(c.notifications.pm_stuck_minutes, good);
        assert!(c.to_json().contains(&format!("\"pmStuckMinutes\": {good}")));
    }
}
