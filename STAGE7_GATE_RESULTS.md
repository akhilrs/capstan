# Stage 7 gate: Codex and OMP hosts, and removal of Docker (PM-35)

Run on 2026-10-01 in a throwaway repository with an isolated Herdr session (`capstan-s7`). The PM was the fake Claude stand-in; the two workers were the **real** Codex 0.159.2 and OMP 18.3.1. The default Herdr session was compared before and after and did not change; `~/.codex/config.toml` was restored from a backup (earlier probes of Codex wrote trust entries into it). Evidence is in `docs/stage7-gate-evidence.txt`; tokens are not in it.

## Real usage spent

Codex: 2 short turns in the gate (one per message) and 2 in the probes before it. OMP: 2 short turns in the gate and 2 in the probes (an environment count in a pane, and one `omp -p` turn that checked the prompt file is read). No other model was called.

## What the probes found (and the plan changed for)

- **A Codex worker cannot reach the daemon socket in any restricted sandbox mode.** `connect` returns EPERM under `workspace-write` (also with `network_access=true`) and under `read-only`; it works only under `danger-full-access`. Codex workers therefore run with `--sandbox danger-full-access --ask-for-approval never`, and OMP workers with `--approval-mode yolo`. Because the controller cannot restrict either, a `PM` or `Supervisor` role, any `allow`/`deny` rule, and a permission mode other than `acceptEdits`/`auto` are configuration errors on those hosts, and `cstan config check` warns about each unsandboxed role on stderr.
- The Codex trust dialog is skipped with a run-time override (`-c projects."<real path>".trust_level="trusted"`), which does not write `~/.codex/config.toml`. The dialog handler is kept as the fallback and is tested against the real dialog screen.
- OMP reads a path given to `--append-system-prompt`; OMP has no trust dialog. `Ctrl+U` clears the whole input in OMP and one line at a time in Codex.

## What ran

1. A Codex worker (`cdev-1`) and an OMP worker (`odev-1`) were spawned by `cstan spawn`. Neither showed a trust dialog; the Codex process arguments held the sandbox, trust and instruction overrides; the OMP process held `--approval-mode yolo --append-system-prompt <file>`.
2. Each agent was sent an instruction to acknowledge it with `cstan ack` and to send the PM its own id. Both messages reached `acked` through the daemon socket, and each agent's reply carried its own id (`cdev-1`, `odev-1`), which only that agent's role prompt contains.
3. **Deferral on Codex:** with `operator draft` typed in the Codex pane a message stayed `deferred` and the typed text was untouched; after the operator cleared the line the message was typed once, acknowledged and answered.
4. **Maximum deferral on OMP:** with `left over draft` typed in the OMP pane the controller waited the maximum deferral, recorded the text and cleared the line (`message_input_clears` holds it with `deferral_count` 1), delivered the message once and the agent acknowledged it and answered once.

## Gate oracle

| Check                                                                       | Result                                                                                                                                       |
| --------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------- |
| Each host starts a worker with its prompt in force                          | PASS (Codex through `developer_instructions`, OMP through the appended file): each answer held the agent id found only in its prompt         |
| A message is typed and acknowledged through `cstan` over the socket         | PASS on both: ledger `acked`                                                                                                                 |
| Text in the input line defers the message and is never merged               | PASS on Codex (the message read `deferred`, the draft was untouched); the OMP draft was cleared only after the maximum deferral and recorded |
| The trust dialog is absent or handled                                       | PASS: absent on both (Codex through the override); the Codex dialog handler is covered by a test against the real screen                     |
| Nothing Docker-related remains in the build                                 | PASS: `grep -ril docker src test scripts package.json migrations` is empty; `npm run check` is green from a clean `dist` (676 tests)         |
| The operator's default Herdr session and Codex configuration are left alone | PASS: workspace list identical before and after; `~/.codex/config.toml` restored                                                             |

## Limits and what this run does not show

- Codex and OMP workers are unsandboxed (see above). Nothing blocks a push or an edit outside the worktree; the role prompt asks, the controller does not enforce.
- The input parsers are checked against Codex 0.159.2 and OMP 18.3.1. A different TUI that changes the input line makes the parser return unreadable, which defers delivery and never types blindly.
- Not run live: a Codex or OMP worker that is lost and replaced, the Stage 5 Supervisor on these hosts (it is refused by configuration), a Codex clear of a multi-line draft (read and cleared in the probes and covered by a unit test of the parser), and an OMP update banner.
- Removed with the legacy `run` flow: `cstan run`, `pause`, `resume` and the no-argument `cancel`. The daemon still answers those with its foreground-controller refusal and the client still detects a foreground controller; both are dead paths kept to limit this change, and the old runtime-session rows can no longer be created.
- The test of the old containment path for a never-started M1 command was removed with the bridge (its remaining behaviour is covered by the two tests that match the new refusal text).
