# DEC-007: One crate for per-OS process information (macOS port)

**Tracker:** PM-321
**Status:** accepted 2026-10-29 after plan review (PASS at round 3).
**Decision:** Add `rust/crates/process` (`capstan-process`), a crate that depends only on `libc` and holds every process fact the daemon, the operator and the test helper read from the OS: a process's start time, the list of pids, whether a process's environment carries an entry, and a process's working directory. Linux keeps its `/proc` reads, moved here unchanged. macOS uses libproc (`proc_listallpids`, `proc_pidinfo`) and `sysctl(KERN_PROCARGS2)` from the pinned `libc`.

## Context

Three production paths read `/proc` with no fallback and silently return nothing on macOS:

- `capstan_operator::service::process_start_time` (`/proc/<pid>/stat` field 22). On macOS it returns `None`, which turns off the operator's pid-reuse guard: after a restart the operator could signal a process group whose leader pid was reused.
- `capstan_daemon::verify::scratch::marked_pids` (`/proc/<pid>/environ`). On macOS it finds nothing, so a verify run's stray processes survive.
- `capstan_testkit::kill_processes_in` (`/proc/<pid>/cwd`). On macOS the test reaper does nothing.

`herdr/src/process_activity.rs` and `cstan/src/controllers.rs` already have non-Linux paths (`ps`, `lsof`) that pass on macOS; they are not changed.

## Options

1. **Put the macOS code next to each caller.** Three copies of libproc code; testkit would carry its own.
2. **Put it in the operator crate.** testkit and the operator tests would depend on the operator crate, which sits far above them.
3. **One small crate below everything (chosen).** One implementation per OS, testable on its own, usable by testkit.

## Consequences

- Start-time values differ by OS: clock ticks since boot on Linux (unchanged), microseconds since the epoch on macOS. A value is only ever compared with another value from the same function on the same machine (equality for the operator's `leaderStart`, ordering for the verify reaper's `min_start`). The ledger stores the value as an opaque string, so no migration is needed. Earlier macOS ledgers hold `null`, which keeps today's behaviour.
- The macOS start time is wall-clock based. A backwards clock step between a verify run's start and its reap can make a marked process look older than the run, so the reaper skips it. The failure is a missed kill, never a wrong kill.
- Reading another process's environment on macOS is possible only for the same user, as on Linux, and only for binaries that are not Apple-signed: macOS 26 with SIP hides the environment of Apple's own binaries (`/bin/sh`, `/usr/bin/env`, `sleep`). So the verify reaper's marker search finds escaped third-party tool daemons (sccache, node, cargo-built tools) on macOS, but a detached Apple system binary (for example `setsid sh -c 'sleep 60'`) survives the run. The recorded process group is still killed. Accepted by the user on 2026-10-29; a fuller macOS reaping design is the follow-up PM-359.
- Adding another OS means adding one module here.
