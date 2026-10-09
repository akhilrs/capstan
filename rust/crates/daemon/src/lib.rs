//! The capstan controller daemon: the Rust port of src/daemon.ts and the command layer of src/commands. The kernel
//! (`capstan-kernel`) owns the ledger; this crate serves it. `deps` holds the one-kernel-thread handle and the bundle every
//! thread is built on, `handlers` the routes, access rules and command handlers, `server` the control socket, `loops` the
//! background work, `run` the process's run function, `reviews` the review start and recovery.

pub mod deps;
pub mod handlers;
pub mod loops;
pub mod reviews;
pub mod run;
pub mod server;
