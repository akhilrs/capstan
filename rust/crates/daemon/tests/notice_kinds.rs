//! The notice kinds the kernel classes and the ones `[messages] notice_class` may name are one table.

use capstan_config::MESSAGE_NOTICE_KINDS;
use capstan_kernel::areas::messaging::NOTICE_KINDS;

#[test]
fn the_kernels_notice_kinds_and_the_configs_have_the_same_names_and_overridable_flags() {
    let kernel: Vec<(&str, bool)> = NOTICE_KINDS
        .iter()
        .map(|(name, _, overridable)| (*name, *overridable))
        .collect();
    assert_eq!(kernel, MESSAGE_NOTICE_KINDS.to_vec());
}
