//! Which accepted key paths the trust record checks. Only `schema_version` and the local-only paths are exempt: every
//! other accepted path (deprecated ones included) is checked, so a change to any setting that can run, limit, delay,
//! silence, hide or drop something asks for the user's trust.

use crate::resolve::local_only_paths;

/// The accepted paths outside the trust record, with the reason each is exempt. The local-only paths are added by
/// `local_only_paths`.
const NOT_TRUSTED: [(&str, &str); 1] = [(
    "schema_version",
    "names the file format, changes nothing at run time",
)];

/// The reason `accepted_path` is exempt from trust, or `None` when it is checked.
fn not_trusted_reason(accepted_path: &str) -> Option<&'static str> {
    if let Some((_, reason)) = NOT_TRUSTED.iter().find(|(path, _)| *path == accepted_path) {
        return Some(reason);
    }
    if local_only_paths().iter().any(|path| path == accepted_path) {
        return Some("honoured only from the untracked local file, never from a pulled commit");
    }
    None
}

/// Whether a change to `accepted_path` (a path of `keys::accepted_keys`, placeholders included) needs the user's trust.
pub(crate) fn is_checked(accepted_path: &str) -> bool {
    not_trusted_reason(accepted_path).is_none()
}

#[cfg(test)]
mod tests {
    use super::{is_checked, not_trusted_reason};
    use crate::keys::accepted_keys;

    const GOLDEN: &str = include_str!("trust_keys.golden");

    fn current() -> String {
        let mut lines: Vec<String> = accepted_keys()
            .into_iter()
            .map(|key| {
                let class = if is_checked(&key.path) {
                    "checked"
                } else {
                    "not_trusted"
                };
                format!("{class} {}", key.path)
            })
            .collect();
        lines.sort_by(|a, b| a.as_bytes().cmp(b.as_bytes()));
        let mut text = lines.join("\n");
        text.push('\n');
        text
    }

    #[test]
    fn golden_lists_every_accepted_path_with_its_class() {
        let current = current();
        if std::env::var_os("TRUST_KEYS_BLESS").is_some() {
            let path = concat!(env!("CARGO_MANIFEST_DIR"), "/src/trust_keys.golden");
            std::fs::write(path, &current).expect("write the golden file");
            return;
        }
        if current != GOLDEN {
            let have: Vec<&str> = GOLDEN.lines().collect();
            let want: Vec<&str> = current.lines().collect();
            let added: Vec<&&str> = want.iter().filter(|l| !have.contains(l)).collect();
            let removed: Vec<&&str> = have.iter().filter(|l| !want.contains(l)).collect();
            panic!(
                "src/trust_keys.golden differs from the accepted keys.\nnew: {added:?}\ngone: {removed:?}\nRegenerate it: TRUST_KEYS_BLESS=1 cargo test -p capstan-config --lib trust_classes"
            );
        }
    }

    #[test]
    fn only_schema_version_and_local_only_keys_are_exempt() {
        assert!(!is_checked("schema_version"));
        assert!(!is_checked("remote.merge"));
        assert!(!is_checked("ntfy.token_file"));
        assert!(!is_checked("env.pass"));
        assert!(is_checked("roles.<name>.model"));
        assert!(is_checked("anything.at.all"));
        assert!(is_checked("ntfy.events"));
        assert!(not_trusted_reason("schema_version").is_some());
    }
}
