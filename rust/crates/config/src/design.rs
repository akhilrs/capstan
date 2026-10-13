//! The `[design]` section: whether a package that depends on a design package waits for the user's approval of the design.
use capstan_wire::js::Value;

use crate::error::Result;
use crate::json::object;
use crate::primitives::*;
use crate::value::Item;

/// The resolved `[design]` section. `configured` is false when the table is absent; the defaults then apply.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Design {
    pub configured: bool,
    pub require_approval: bool,
}

impl Default for Design {
    fn default() -> Self {
        Self {
            configured: false,
            require_approval: true,
        }
    }
}

impl Design {
    pub(crate) fn to_value(self) -> Value {
        object(vec![
            ("configured", Value::Bool(self.configured)),
            ("requireApproval", Value::Bool(self.require_approval)),
        ])
    }
}

/// Resolves `[design]`; an absent table is the default, which requires approval.
pub fn resolve_design(value: Option<&Item>) -> Result<Design> {
    if value.is_none() {
        return Ok(Design::default());
    }
    let table = required_table(value, "design")?;
    reject_unknown_keys(table, &["require_approval"], "design")?;
    Ok(Design {
        configured: true,
        require_approval: optional_boolean(
            table.get("require_approval"),
            "design.require_approval",
            true,
        )?,
    })
}
