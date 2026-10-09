//! The strict flag of the daemon parity tests: a replay fails on any pending step (a route nobody ported, a socket step
//! with no server). Strict is the default now that every package of the phase has landed; `CAPSTAN_DAEMON_PARITY_STRICT=0`
//! turns it off for a run that is looking at one step.

/// Whether pending steps fail the run (on unless `CAPSTAN_DAEMON_PARITY_STRICT=0`).
pub fn strict() -> bool {
    strict_from(
        std::env::var("CAPSTAN_DAEMON_PARITY_STRICT")
            .ok()
            .as_deref(),
    )
}

/// `strict` for a given value of CAPSTAN_DAEMON_PARITY_STRICT.
pub fn strict_from(value: Option<&str>) -> bool {
    value != Some("0")
}

#[cfg(test)]
mod tests {
    use super::strict_from;

    #[test]
    fn strict_unless_zero() {
        assert!(strict_from(None));
        assert!(strict_from(Some("1")));
        assert!(strict_from(Some("")));
        assert!(strict_from(Some("true")));
        assert!(!strict_from(Some("0")));
    }
}
