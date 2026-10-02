# Investigation: real controller shutdown on 2026-10-02 (~17:13 +0530)

Status: root cause identified from logs (an operator-credential `shutdown`
on the real project's socket, issued before the tester's smoke test began).
No controller code was changed. No failing test: there is no reproduction of
cross-project interference.

## Root cause (evidence-backed)

The real controller (pid 892) did not crash and did not lose its lock. It
received a `shutdown` command over its own control socket from the real
project's operator identity, then exited cleanly. The same identity started a
new daemon 3 seconds later.

Real project `.capstan/daemon.log` (UTC; local = UTC+0530):

```
11:43:47.519Z ping      actor 62411354-… role operator  ok
11:43:47.522Z shutdown  actor 62411354-… role operator  ok
11:43:47.528Z wait      actor 7824440e-… role PM        code=shutting_down  (the PM's cstan wait)
11:43:47.543Z lock_released pid 892 ; socket_removed pid 892
11:43:50.424Z ready pid 28498            (new daemon)
11:43:51.440Z launch    actor 62411354-… role operator  ok   (Herdr launch, so CAPSTAN_LAUNCH was not off)
```

So 17:13:47 +0530 is the shutdown. The `ping` then `shutdown` pair is exactly
what `stopDaemon()` sends (`src/client.ts`, `stopDaemon`). Actor
`62411354-…` is the real project's operator (it appears in every operator call
in that log since 07:29Z).

The tester's temporary project is a different actor and a later time. From
`/tmp/tmp.A4aeSK75hz/.capstan/daemon.log`:

```
11:44:39.508Z ready pid 811
11:44:46.537Z shutdown actor 686f28c8-… (a different operator id)
```

- `.capstan` of the temp project was created at 17:14:38 +0530, 51 seconds after the real shutdown.
- Its daemon wrote only to its own log and its own socket.
- Nothing in the real log was written by actor `686f28c8-…`.

Conclusion: the tester's temp daemon did not cause the real shutdown. The
timing is the wrong way round, and the real shutdown carries the real operator
credential, which a process started in `/tmp/tmp.A4aeSK75hz` cannot read.

## What most likely happened (not provable from logs)

An operator ran `cstan stop` and `cstan start` (a restart) in the real project:

- The 3 second gap to `ready`, followed by an operator `launch`, is the shape
  of `stop && start`.
- The same pattern appears twice earlier in the log:
  - `08:20:48Z` shutdown, `08:20:51Z` ready (pid 13056 to 31370)
  - `08:26:20Z` shutdown, a restart follows.
- The new daemon (pid 28498) started at 17:13:49 and a `status --watch`
  (pid 28734) started at the same minute, both in the real project directory.

The logs record who (the operator credential) and when, not which terminal.
What would confirm it: the operator's shell history, or the Herdr pane that ran
`cstan stop`. The PM's note that the operator restarted "later" does not match
the log: the restart was 3 seconds after the shutdown. The PM's `wait`
returned `shutting_down` at 11:43:47, which is why the PM saw the failure.
Re-check the order of events in the tester's account against these timestamps.

## Answers to the four questions

1. Own logs: yes. `daemon.log` has the `shutdown` command, its actor and role,
   `lock_released` and `socket_removed`. There was no signal, crash, lock loss
   or idle shutdown logged for that pid. `daemon.log` does not record the
   caller's pid or cwd, so the source terminal is not recorded
   (see "Gaps" below). `controller_events` could not be read: no `sqlite3`
   binary is installed and no reader was run against the real database, to
   keep the real state untouched.
2. Cross-project effect: not possible by design.
   - `stop` targets `<cwd>/.capstan/state/control.sock` (`loadOperator`,
     `src/cli.ts`) and authenticates with `<cwd>/.capstan/operator.key`.
   - No walk-up of parent directories, no global or XDG runtime directory
     (`~/.capstan` does not exist), no pid-based kill, no `pkill`.
   - `stopDaemon()` sends the `shutdown` command over the socket and then only
     polls `process.kill(pid, 0)` for the pid that the same socket reported.
   - The project lock is `<state>/controller.lock`, per project.
3. `cstan dash`, `start` and `ensureRunning` use the same `loadOperator(cwd)`
   path, so they cannot reach another project. They need `CAPSTAN_TOKEN` and
   `CAPSTAN_SOCKET` unset, and the tester had them unset.
   The only way to reach the real controller from another directory is to set
   `CAPSTAN_SOCKET` and `CAPSTAN_TOKEN` to the real values. Agents have them
   set; operator commands ignore them (`route.access === "operator"`).
4. Reproduction, with the worktree build (`dist/src/cli.js`) in two temp
   projects A and B (`git init`, `cstan init`, `CAPSTAN_LAUNCH=off`):
   start A (pid 6749), start B (pid 6790), `stop` in B prints
   `running: false, result: stopped`, then `ping` in A answers `pong: true`
   with pid 6749 and the process is alive. Project A is not affected.

## Gaps and a side finding

- `daemon.log` cannot say which process sent `shutdown`. Adding the peer
  credentials (`SO_PEERCRED` pid) or the caller's cwd to the log line would
  make this class of question answerable. Not done; no controller change was
  requested.
- Side finding: a state path longer than about 107 bytes makes `cstan start`
  fail with only "the controller did not answer; check the daemon log or
  retry" (unix socket path limit). It cost one failed run during this
  investigation (scratchpad path was ~150 bytes). The error does not name the
  cause. Worth a clear check in `start`.

## Ranked candidates

1. Operator-run restart in the real project at 17:13:47 (confirmed that the
   cause was an operator-credential `shutdown`; the human action itself is
   inferred). Confirm with shell history or the Herdr pane log.
2. Tester's `stop` hitting the real project: ruled out by timestamps (the temp
   project was created 51 s later), by actor id, and by the reproduction.
3. Crash, signal or lock loss: ruled out; the log has a clean
   `shutdown`, `lock_released`, `socket_removed` sequence for pid 892.
