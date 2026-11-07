//! The trust gate in the controller: its start-up refuses an untrusted project with the gate's text, and a decision read
//! from the configuration at call time never applies a value from an untrusted file.

mod common;

use capstan_daemon::run::daemon_options_with;
use common::merge::MergeProject;
use common::project::write_project;
use common::project::Identity;
use serde_json::Value;

fn code(answer: &Value) -> String {
    assert_eq!(answer["ok"], false, "{answer}");
    answer["code"].as_str().unwrap().to_string()
}

#[test]
fn daemon_options_with_refuses_an_untrusted_project_and_carries_the_store_of_a_trusted_one() {
    let dir = common::private_tempdir();
    let identity = Identity {
        project_id: "ptrust".into(),
        name: "Trust".into(),
        credential: format!("operator-{}", "t".repeat(40)),
    };
    write_project(dir.path(), &identity);
    common::write_config_untrusted(
        dir.path(),
        "schema_version = 1\n\n[hosts.claude]\nkind = \"claude\"\n\n[roles.pm]\nkind = \"PM\"\nhost = \"claude\"\n\n[verify]\ncommands = [\"echo ok\"]\n",
    );
    let (_holder, store) = common::test_trust_store();

    let error = daemon_options_with(dir.path(), store.clone())
        .err()
        .expect("an untrusted project does not start");
    assert!(error.to_string().contains("cstan trust"), "{error}");

    common::trusted_project(dir.path(), &store);
    let (options, _) =
        daemon_options_with(dir.path(), store.clone()).expect("a trusted project starts");
    assert_eq!(options.trust_store, store);
}

#[test]
fn decide_merge_reads_an_untrusted_change_as_a_refused_configuration_and_proceeds_once_trusted() {
    let project = MergeProject::new();
    for args in [
        &["update-ref", "refs/remotes/origin/main", "HEAD"][..],
        &[
            "symbolic-ref",
            "refs/remotes/origin/HEAD",
            "refs/remotes/origin/main",
        ][..],
    ] {
        let status = std::process::Command::new("git")
            .args(args)
            .current_dir(project.dir.path())
            .status()
            .unwrap();
        assert!(status.success());
    }
    let tracked = project.dir.path().join("capstan.toml");
    let before = project.rev("HEAD");
    let mut text = std::fs::read_to_string(&tracked).unwrap();
    text.push_str("# changed\n");
    text = text.replace(
        "commands = [\"true\"]",
        "commands = [\"true\", \"echo changed\"]",
    );
    std::fs::write(&tracked, text).unwrap();

    let answer = project.decide_merge("nonce-untrusted");
    assert_eq!(code(&answer), "merge_disabled", "{answer}");
    assert_eq!(project.rev("HEAD"), before);

    common::trusted_project(project.dir.path(), &project.trust);
    let answer = project.decide_merge("nonce-trusted");
    assert_ne!(answer["code"].as_str(), Some("merge_disabled"), "{answer}");
}
