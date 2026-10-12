//! Id prefixes (`areas::id_prefix`): a unique prefix of at least four characters is the full id, an ambiguous one lists
//! its candidates, a short one that would have matched is refused, and an exact or unknown id is left alone. The ids are
//! rows of a scratch table, since the ids of real records are random.

use crate::fixture::Fixture;
use capstan_kernel::areas::id_prefix::{refusal, resolve_id, Resolved};
use capstan_kernel::helpers::execute;

fn resolved(f: &Fixture, id: &str) -> Resolved {
    resolve_id(f.core().kernel(), &[("scratch_ids", "scratch_id")], id).unwrap()
}

#[test]
fn prefixes_resolve_git_style() {
    let f = Fixture::new();
    let kernel = f.core().kernel();
    execute(
        &kernel.database,
        "CREATE TABLE scratch_ids (project_id TEXT NOT NULL, scratch_id TEXT NOT NULL)",
        [],
    )
    .unwrap();
    for id in ["abcd1111", "abcd2222", "abce3333", "wxyz", "wxyz9"] {
        execute(
            &kernel.database,
            "INSERT INTO scratch_ids VALUES (?, ?)",
            [&kernel.project_id, id],
        )
        .unwrap();
    }
    // Unique.
    assert_eq!(resolved(&f, "abce"), Resolved::Full("abce3333".into()));
    assert_eq!(resolved(&f, "abcd1"), Resolved::Full("abcd1111".into()));
    // Ambiguous: the candidates are listed, sorted.
    let ambiguous = resolved(&f, "abcd");
    assert_eq!(
        ambiguous,
        Resolved::Ambiguous(vec!["abcd1111".into(), "abcd2222".into()])
    );
    let message = refusal("abcd", &ambiguous);
    assert!(
        message.contains("ambiguous") && message.contains("abcd2222"),
        "{message}"
    );
    // Too short, but only when something starts with it.
    assert_eq!(resolved(&f, "abc"), Resolved::TooShort);
    assert!(refusal("abc", &Resolved::TooShort).contains("at least 4"));
    assert_eq!(resolved(&f, "xyz"), Resolved::Unchanged);
    // Unknown and exact ids are left alone; an exact id wins over longer ids it prefixes.
    assert_eq!(resolved(&f, "zzzzzz"), Resolved::Unchanged);
    assert_eq!(resolved(&f, "wxyz"), Resolved::Unchanged);
    assert_eq!(resolved(&f, "abcd1111"), Resolved::Unchanged);
    // Characters LIKE would treat as wildcards match nothing.
    assert_eq!(resolved(&f, "%%%%"), Resolved::Unchanged);
}
