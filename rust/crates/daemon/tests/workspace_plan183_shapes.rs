//! The shapes plan-184 builds on, as plan-183 merged them. Run first: a changed signature fails here, loudly, before any
//! set test fails for a reason that is hard to read.

mod common;

use capstan_config::TrustStore;
use capstan_daemon::deps::Deps;
use capstan_daemon::handlers::integrate_seams::{install_integrate_seams, IntegrateSeams};
use capstan_daemon::loops::recover::recover_integrations;
use capstan_kernel::integrate::{integrate_repo, IntegrateResult, IntegrationDeps, MemberTarget};
use capstan_kernel::kernel::KernelOptions;
use capstan_kernel::records::GateSubject;
use capstan_kernel::{Core, KernelResult};
use common::project::{write_project, Identity};
use common::{private_tempdir, seeded_env};
use serde_json::Value;

#[test]
#[allow(clippy::type_complexity)]
fn the_plan_183_signatures_are_the_ones_set_verify_uses() {
    // GateSubject: a report or a set, compared by id.
    let subject = GateSubject::Set("s-1".into());
    assert_eq!(subject.clone(), GateSubject::Set("s-1".into()));
    assert_ne!(subject, GateSubject::Report("s-1".into()));
    // Core's gate readers, the set loop's entry points and the daemon's seam and recovery hooks.
    let _: fn(&Core, &GateSubject) -> KernelResult<Vec<Value>> = Core::subject_runs;
    let _: fn(&Core, &GateSubject) -> Result<(), String> = Core::review_gate;
    let _: fn(&Core, &str, &str) -> KernelResult<Value> = Core::set_gate;
    let _: fn(&Core, &str) -> KernelResult<Option<String>> = Core::set_of_integration;
    let _: fn(&Core, &str) -> KernelResult<Value> = Core::integration_set;
    let _: fn(
        &IntegrationDeps,
        &str,
        &[(String, String)],
        &str,
        &MemberTarget,
    ) -> IntegrateResult<Value> = integrate_repo;
    let _: fn(&Deps, IntegrateSeams) = install_integrate_seams;
    let _: fn(&Deps, &dyn Fn() -> bool) = recover_integrations;
    let seams = IntegrateSeams {
        fault: None,
        git_seams: None,
    };
    assert!(seams.fault.is_none() && seams.git_seams.is_none());
}

#[test]
fn the_gate_readers_answer_for_an_unknown_subject_as_plan_183_left_them() {
    let dir = private_tempdir();
    let identity = Identity {
        project_id: "p183".into(),
        name: "Shapes".into(),
        credential: format!("operator-{}", "s".repeat(40)),
    };
    let state = write_project(dir.path(), &identity);
    let database = capstan_ledger::resolve_database_path(&state).unwrap();
    capstan_ledger::open_database(&database, &capstan_ledger::OpenOptions::default()).unwrap();
    let _trust: TrustStore = common::kept_trust_store();
    let core = Core::open(
        &state,
        &identity.initial_project(),
        &KernelOptions {
            workspace_root: Some(dir.path().to_path_buf()),
            ..KernelOptions::default()
        },
        seeded_env("plan183-shapes"),
    )
    .expect("the kernel opens");
    // A subject that is not a report passes the review gate; an unknown set is an error, never a pass.
    assert_eq!(
        core.review_gate(&GateSubject::Report("nothing".into())),
        Ok(())
    );
    assert!(core
        .review_gate(&GateSubject::Set("nothing".into()))
        .is_err());
    assert!(core
        .subject_runs(&GateSubject::Set("nothing".into()))
        .is_err());
    assert!(core
        .subject_runs(&GateSubject::Report("nothing".into()))
        .unwrap()
        .is_empty());
    assert_eq!(core.set_of_integration("nothing").unwrap(), None);
}
