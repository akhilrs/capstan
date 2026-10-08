//! The capstan ledger. This skeleton only exposes the bundled SQLite version.

/// The version of the SQLite library bundled into this build.
pub fn sqlite_version() -> &'static str {
    rusqlite::version()
}

#[cfg(test)]
mod tests {
    use super::sqlite_version;

    fn parse(version: &str) -> (u32, u32, u32) {
        let mut parts = version.split('.').map(|p| p.parse::<u32>().unwrap());
        (
            parts.next().unwrap(),
            parts.next().unwrap(),
            parts.next().unwrap(),
        )
    }

    #[test]
    fn bundled_sqlite_is_at_least_node_24_6() {
        // Node 24.6's node:sqlite embeds SQLite 3.50.4.
        assert!(
            parse(sqlite_version()) >= (3, 50, 4),
            "{}",
            sqlite_version()
        );
    }
}
