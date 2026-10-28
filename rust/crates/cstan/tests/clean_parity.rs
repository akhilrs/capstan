//! `watch::clean` strips what the kernel's `clean_text` strips: both run the same cases.
use capstan_kernel::helpers::{clean_text, CLEAN_CASES};
use capstan_wire::js::Value;
use cstan_front::watch::clean;

#[test]
fn watch_clean_matches_clean_text_on_every_case() {
    for (input, expected) in CLEAN_CASES {
        let value = Value::String((*input).into());
        assert_eq!(clean(Some(&value)), *expected, "{input:?}");
        assert_eq!(clean(Some(&value)), clean_text(input), "{input:?}");
    }
}
