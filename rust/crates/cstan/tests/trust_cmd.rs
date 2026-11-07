//! `cstan trust`: what it lists, what it records and when it refuses.
mod support;

use std::ffi::OsString;

use capstan_config::{trust_status_with, TrustStatus};
use serde_json::Value;
use support::*;

#[test]
fn the_first_run_lists_every_checked_key_and_records_the_second_says_trusted() {
    let bed = TrustBed::new("first");
    bed.write(TRUST_FIXTURE_TOML);
    let (exit, out, err) = bed.run(&["trust"], bed.env());
    assert_eq!((exit, err.as_str()), (0, ""), "{out}");
    let added: Vec<&str> = out.lines().filter(|l| l.starts_with("+ ")).collect();
    assert_eq!(added.len(), 2, "{out}");
    assert!(added.iter().any(|l| l.contains("max_size_gb = 5")), "{out}");
    assert!(
        out.contains(&format!(
            "Trusted 2 key(s) of capstan.toml on this machine ({}).",
            bed.store_dir.display()
        )),
        "{out}"
    );
    assert_eq!(
        trust_status_with(&bed.project, &bed.store()),
        TrustStatus::Trusted
    );
    let (exit, out, _) = bed.run(&["trust"], bed.env());
    assert_eq!(
        (exit, out.as_str()),
        (0, "capstan.toml is trusted on this machine.\n")
    );
}

#[test]
fn one_change_lists_only_that_key() {
    let bed = TrustBed::new("change");
    bed.write(TRUST_FIXTURE_TOML);
    assert_eq!(bed.run(&["trust"], bed.env()).0, 0);
    bed.write("[cache]\nmax_size_gb = 7\n[roles.dev]\nmodel = \"m1\"\n");
    let (exit, out, _) = bed.run(&["trust"], bed.env());
    assert_eq!(exit, 0, "{out}");
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 2, "{out}");
    assert!(
        lines[0].starts_with("~ ") && lines[0].ends_with(": 5 -> 7"),
        "{out}"
    );
    assert!(lines[1].starts_with("Trusted 2 key(s)"), "{out}");
}

#[test]
fn check_exits_3_and_records_nothing() {
    let bed = TrustBed::new("check");
    bed.write(TRUST_FIXTURE_TOML);
    let (exit, out, err) = bed.run(&["trust", "--check"], bed.env());
    assert_eq!(exit, 3, "{out}{err}");
    assert_eq!(
        out.lines().filter(|l| l.starts_with("+ ")).count(),
        2,
        "{out}"
    );
    assert_eq!(err, "cstan: Not trusted. Run cstan trust to trust it.\n");
    assert!(!bed.store_dir.exists());
    assert!(matches!(
        trust_status_with(&bed.project, &bed.store()),
        TrustStatus::Needed { .. }
    ));
}

#[test]
fn the_json_shape() {
    let bed = TrustBed::new("json");
    bed.write(TRUST_FIXTURE_TOML);
    let (exit, out, _) = bed.run(&["trust", "--json"], bed.env());
    assert_eq!(exit, 0);
    let json: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(json["state"], "needed");
    assert_eq!(json["recorded"], true);
    assert_eq!(json["storeDir"], bed.store_dir.display().to_string());
    let changes = json["changes"].as_array().unwrap();
    assert_eq!(changes.len(), 2);
    for change in changes {
        assert_eq!(change["change"], "added");
        assert!(change["key"].is_string() && change["new"].is_string());
        assert!(change["old"].is_null());
    }
    let (exit, out, _) = bed.run(&["trust", "--json"], bed.env());
    let json: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(
        (exit, json["state"].as_str(), json["recorded"].as_bool()),
        (0, Some("trusted"), Some(false))
    );
    assert_eq!(json["changes"].as_array().unwrap().len(), 0);

    let bed = TrustBed::new("json-check");
    bed.write(TRUST_FIXTURE_TOML);
    let (exit, out, _) = bed.run(&["trust", "--check", "--json"], bed.env());
    let json: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(
        (exit, json["state"].as_str(), json["recorded"].as_bool()),
        (3, Some("needed"), Some(false))
    );
}

#[test]
fn an_unavailable_store_refuses_with_the_reason() {
    let bed = TrustBed::new("unavailable");
    bed.write(TRUST_FIXTURE_TOML);
    let env = vec![(
        OsString::from("CAPSTAN_TRUST_DIR"),
        OsString::from("relative/dir"),
    )];
    let (exit, out, err) = bed.run(&["trust"], env.clone());
    assert_eq!((exit, out.as_str()), (3, ""));
    assert!(
        err.contains("CAPSTAN_TRUST_DIR relative/dir is not an absolute path"),
        "{err}"
    );
    assert!(
        err.trim_end()
            .ends_with("Set CAPSTAN_TRUST_DIR to an absolute directory."),
        "{err}"
    );
    let (exit, out, _) = bed.run(&["trust", "--json"], env);
    let json: Value = serde_json::from_str(&out).unwrap();
    assert_eq!((exit, json["state"].as_str()), (3, Some("unavailable")));
}

#[test]
fn an_agent_shell_is_refused_before_anything_is_read() {
    for variable in ["CAPSTAN_TOKEN", "CAPSTAN_SOCKET"] {
        let bed = TrustBed::new("agent");
        bed.write(TRUST_FIXTURE_TOML);
        let mut env = bed.env();
        env.push((OsString::from(variable), OsString::from("x")));
        for args in [&["trust"][..], &["trust", "--json"][..]] {
            let (exit, out, err) = bed.run(args, env.clone());
            assert_eq!(exit, 3);
            assert_eq!(out, "");
            assert_eq!(
                err,
                "cstan: cstan trust is for the user: run it in your own terminal, not from an agent.\n"
            );
        }
        assert!(!bed.store_dir.exists());
    }
}

#[test]
fn a_secret_looking_value_is_shown_hidden() {
    let bed = TrustBed::new("secret");
    bed.write("[roles.dev]\nmodel = \"tool --token capstan-secret-1234567890\"\n");
    let (exit, out, _) = bed.run(&["trust", "--check"], bed.env());
    assert_eq!(exit, 3);
    assert!(out.contains("<hidden>"), "{out}");
    assert!(!out.contains("capstan-secret-1234567890"), "{out}");
}

#[test]
fn a_file_without_checked_keys_is_trusted_as_it_is() {
    let bed = TrustBed::new("notneeded");
    let (exit, out, _) = bed.run(&["trust"], bed.env());
    assert_eq!(
        (exit, out.as_str()),
        (0, "capstan.toml is trusted on this machine.\n")
    );
}
