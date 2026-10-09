//! The restart helper (src/restart-helper.ts), started detached by the restart coordinator as `<binary> __restart-helper
//! <plan>`. It restarts exactly the daemon command recorded in its plan file (`current_exe` plus its arguments for
//! cstan-daemon), so no selection logic lives here. A stub until its package.

/// Runs the helper for the plan file in `args` (the arguments after `__restart-helper`) and returns its exit code: 0 the
/// new build answered, 1 rolled back, 2 down. The stub reports that it is not implemented.
pub fn run(args: &[String]) -> i32 {
    let _ = args;
    eprintln!("cstan-daemon: __restart-helper is not implemented yet");
    2
}
