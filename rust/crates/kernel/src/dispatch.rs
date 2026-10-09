//! Running an operation by its `ControllerCore` method name with JSON arguments: what a parity sequence does. Each
//! area file decodes the arguments of its own operations (`areas/<area>.rs` `dispatch`); this file chains them.

use crate::areas;
use crate::errors::{KernelError, KernelResult};
use crate::kernel::Kernel;
use crate::types::MutationContext;
use serde_json::Value;

/// Unwraps an argument or returns its error from the dispatching function.
macro_rules! arg {
    ($e:expr) => {
        match $e {
            Ok(value) => value,
            Err(error) => return Some(Err(error)),
        }
    };
}
pub(crate) use arg;

/// What `sweepFindings` calls for a finding it could not sweep.
pub type ErrorHandler<'a> = &'a dyn Fn(&str, &KernelError);

/// What `abandonRunningOperatorRuns` asks about a restart proposal.
pub type RestartFilter<'a> = &'a dyn Fn(&str) -> bool;

/// The result of an area function, as the dispatcher returns it.
pub fn call(result: KernelResult<Value>) -> KernelResult<Value> {
    result
}

/// The arguments of one call. A missing argument reads as JSON `null`.
pub struct Args<'a>(&'a [Value]);

impl<'a> Args<'a> {
    pub fn new(args: &'a [Value]) -> Self {
        Args(args)
    }

    fn at(&self, index: usize) -> &'a Value {
        static NULL: Value = Value::Null;
        self.0.get(index).unwrap_or(&NULL)
    }

    pub fn ctx(&self, index: usize) -> KernelResult<MutationContext> {
        let value = self.at(index);
        let text = |key: &str| {
            value
                .get(key)
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string()
        };
        // A number that is not an integer reads as 0, which `mutate` refuses with the Node message.
        let number = |key: &str| value.get(key).and_then(Value::as_i64).unwrap_or(0);
        Ok(MutationContext {
            credential: text("credential"),
            request_id: text("requestId"),
            idempotency_key: text("idempotencyKey"),
            expected_version: number("expectedVersion"),
            input_revision: number("inputRevision"),
        })
    }

    pub fn str(&self, index: usize, name: &str) -> KernelResult<&'a str> {
        self.at(index)
            .as_str()
            .ok_or_else(|| KernelError::type_error(format!("{name} must be a string")))
    }

    pub fn opt_str(&self, index: usize, name: &str) -> KernelResult<Option<&'a str>> {
        match self.at(index) {
            Value::Null => Ok(None),
            _ => self.str(index, name).map(Some),
        }
    }

    pub fn i64(&self, index: usize, name: &str) -> KernelResult<i64> {
        self.at(index)
            .as_i64()
            .ok_or_else(|| KernelError::type_error(format!("{name} must be an integer")))
    }

    pub fn opt_i64(&self, index: usize, name: &str) -> KernelResult<Option<i64>> {
        match self.at(index) {
            Value::Null => Ok(None),
            _ => self.i64(index, name).map(Some),
        }
    }

    pub fn f64(&self, index: usize, name: &str) -> KernelResult<f64> {
        self.at(index)
            .as_f64()
            .ok_or_else(|| KernelError::type_error(format!("{name} must be a number")))
    }

    pub fn opt_f64(&self, index: usize, name: &str) -> KernelResult<Option<f64>> {
        match self.at(index) {
            Value::Null => Ok(None),
            _ => self.f64(index, name).map(Some),
        }
    }

    pub fn bool(&self, index: usize, name: &str) -> KernelResult<bool> {
        self.at(index)
            .as_bool()
            .ok_or_else(|| KernelError::type_error(format!("{name} must be a boolean")))
    }

    pub fn opt_bool(&self, index: usize, name: &str) -> KernelResult<Option<bool>> {
        match self.at(index) {
            Value::Null => Ok(None),
            _ => self.bool(index, name).map(Some),
        }
    }

    pub fn value(&self, index: usize) -> &'a Value {
        self.at(index)
    }

    pub fn opt_value(&self, index: usize) -> Option<&'a Value> {
        match self.at(index) {
            Value::Null => None,
            other => Some(other),
        }
    }

    pub fn opt_str_list(&self, index: usize) -> KernelResult<Option<Vec<String>>> {
        match self.at(index) {
            Value::Null => Ok(None),
            Value::Array(items) => Ok(Some(
                items
                    .iter()
                    .map(|v| {
                        v.as_str()
                            .map(str::to_string)
                            .ok_or_else(|| KernelError::type_error("expected a list of strings"))
                    })
                    .collect::<KernelResult<Vec<_>>>()?,
            )),
            _ => Err(KernelError::type_error("expected a list of strings")),
        }
    }
}

/// Runs `op`; an operation no area knows is a `TypeError`.
pub fn dispatch(kernel: &Kernel, op: &str, args: &[Value]) -> KernelResult<Value> {
    if let Some(result) = areas::actors::dispatch(kernel, op, args) {
        return result;
    }
    if let Some(result) = areas::agents::dispatch(kernel, op, args) {
        return result;
    }
    if let Some(result) = areas::panes::dispatch(kernel, op, args) {
        return result;
    }
    if let Some(result) = areas::messages::dispatch(kernel, op, args) {
        return result;
    }
    if let Some(result) = areas::message_notices::dispatch(kernel, op, args) {
        return result;
    }
    if let Some(result) = areas::pauses::dispatch(kernel, op, args) {
        return result;
    }
    if let Some(result) = areas::findings::dispatch(kernel, op, args) {
        return result;
    }
    if let Some(result) = areas::prompt_relay::dispatch(kernel, op, args) {
        return result;
    }
    if let Some(result) = areas::reports::dispatch(kernel, op, args) {
        return result;
    }
    if let Some(result) = areas::reviews::dispatch(kernel, op, args) {
        return result;
    }
    if let Some(result) = areas::integrations::dispatch(kernel, op, args) {
        return result;
    }
    if let Some(result) = areas::plans::dispatch(kernel, op, args) {
        return result;
    }
    if let Some(result) = areas::plan_packages::dispatch(kernel, op, args) {
        return result;
    }
    if let Some(result) = areas::links::dispatch(kernel, op, args) {
        return result;
    }
    if let Some(result) = areas::operator_proposals::dispatch(kernel, op, args) {
        return result;
    }
    if let Some(result) = areas::operator_grants::dispatch(kernel, op, args) {
        return result;
    }
    if let Some(result) = areas::operator_runs::dispatch(kernel, op, args) {
        return result;
    }
    if let Some(result) = areas::status::dispatch(kernel, op, args) {
        return result;
    }
    if let Some(result) = areas::reconcile::dispatch(kernel, op, args) {
        return result;
    }
    if let Some(result) = areas::messaging::dispatch(kernel, op, args) {
        return result;
    }
    Err(KernelError::type_error(format!("unknown operation {op}")))
}
