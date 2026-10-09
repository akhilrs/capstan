//! The Operator service and the restart coordinator of the capstan daemon (src/operator.ts, src/restart.ts,
//! src/restart-helper.ts). `api` holds the interfaces and stubs; the modules below it are filled in by the package that
//! owns each.

pub mod api;
pub mod policy;
pub mod restart;
pub mod restart_helper;
pub mod service;
