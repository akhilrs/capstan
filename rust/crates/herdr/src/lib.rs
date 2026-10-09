//! The Herdr layer of the capstan daemon (src/herdr): the runner that drives the `herdr` binary, the adapter that types
//! into and manages panes, the screen parsers and the process probe. `api` holds the interfaces and the recording
//! stubs the other crates build on; the modules below it are filled in by the package that owns each.

pub mod adapter;
pub mod api;
pub mod claude_args;
pub mod hosts;
pub mod naming;
pub mod process_activity;
pub mod prompt_relay;
pub mod runner;
pub mod screen;
