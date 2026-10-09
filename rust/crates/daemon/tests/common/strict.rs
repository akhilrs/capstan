//! The strict flag of the daemon parity tests: by default a replay may leave steps pending (a route its package has not
//! ported, a socket step before the server exists); `CAPSTAN_DAEMON_PARITY_STRICT=1` makes any pending step a failure,
//! which is how the last package of the phase closes the gate.

/// Whether pending steps fail the run (`CAPSTAN_DAEMON_PARITY_STRICT=1`; any other value, or none, allows them).
pub fn strict() -> bool {
    strict_from(
        std::env::var("CAPSTAN_DAEMON_PARITY_STRICT")
            .ok()
            .as_deref(),
    )
}

/// `strict` for a given value of CAPSTAN_DAEMON_PARITY_STRICT.
pub fn strict_from(value: Option<&str>) -> bool {
    value == Some("1")
}

#[cfg(test)]
mod tests {
    use super::strict_from;

    #[test]
    fn only_one_means_strict() {
        assert!(strict_from(Some("1")));
        assert!(!strict_from(Some("0")));
        assert!(!strict_from(Some("true")));
        assert!(!strict_from(None));
    }
}
