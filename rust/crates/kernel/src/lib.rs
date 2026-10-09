//! The capstan controller kernel: the Rust port of src/controller (ControllerCore and its areas). The foundation here
//! is the storage-facing core (open, mutate, auth, bootstrap, actors); the areas fill in behind the same signatures.

pub mod areas;
pub mod auth;
pub mod bootstrap;
pub mod canonical;
pub mod core;
pub mod dispatch;
pub mod env;
pub mod errors;
#[doc(hidden)]
pub mod export_file;
pub mod helpers;
pub mod integrate;
pub mod json;
pub mod kernel;
pub mod plan_body;
pub mod records;
pub mod types;

pub use crate::core::Core;
pub use crate::env::{Env, SeededEnv, SystemEnv};
pub use crate::errors::{KernelError, KernelResult};
pub use crate::kernel::{Kernel, KernelOptions};
