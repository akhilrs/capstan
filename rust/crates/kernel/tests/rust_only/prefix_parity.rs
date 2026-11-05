//! The kernel's `prefix_grant_problem` (the check `apply_operator_decision` makes and `approve_choices` offers by) gives the
//! same verdict as the Operator's `policy::prefix_grant_problem` on the prefix cases of the Operator's parity fixture.
//! Update this test whenever either function changes.

use capstan_kernel::areas::operator_proposals::prefix_problem;
use serde_json::{json, Value};

#[test]
fn the_kernel_prefix_check_gives_the_operator_policy_verdicts() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../operator/tests/parity/policy.json");
    let fixture: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let cases = fixture["prefixProblems"].as_array().unwrap();
    assert!(cases.len() > 15, "{} cases", cases.len());
    for case in cases {
        let prefix = case["prefix"].as_str().unwrap();
        assert_eq!(json!(prefix_problem(prefix)), case["problem"], "{prefix:?}");
    }
}
