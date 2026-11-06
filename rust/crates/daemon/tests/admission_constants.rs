//! The `[daemon]` connection defaults agree with the daemon's own constants, and a run without a config never folds
//! a send.
use capstan_config::{daemon_limits, DaemonTuning};

#[test]
fn default_max_connections_equals_the_daemon_constant() {
    assert_eq!(
        DaemonTuning::default().max_connections,
        capstan_daemon::handlers::MAX_CONNECTIONS as i64
    );
}

#[test]
fn no_config_never_folds_a_send() {
    let limits = daemon_limits(None);
    assert_eq!(limits.repeat_window_seconds, 0);
    assert_eq!(
        limits.max_connections,
        DaemonTuning::default().max_connections
    );
}
