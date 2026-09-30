# Spike: Herdr-hosted interactive agents reporting through `cstan`

Ticket PM-19. Date 2026-09-30. This document records evidence for the Capstan redesign (no Docker, worktrees, interactive persistent agents, PM asks Capstan to spawn). It is evidence and recommendations, not a decision record.

## Scope and limits

- One host only: Claude Code 2.1.285 (Sonnet 5.5, Claude Pro login). Codex and OMP were not tested.
- Herdr client 0.9.1. The experiments ran in an isolated named session (`capstan-spike`, its own server and socket); the operator's session was not touched. Herdr's agent-detection manifest was `claude.toml 2026.09.11.1`, fetched from `remote:`.
- Every experiment ran once (n = 1). Timings are single observations, not distributions.
- Scratch repository and stand-in `cstan` script lived outside this repository. The stand-in authenticates by an environment token against a credential table and appends a receipt to a file.
- Not tested: the outdated Herdr hook for Claude (`herdr integration status` reports v9 < v10; it was not updated), long-running work, agent death and replacement, several agents at once, a PM driving other agents, Codex, OMP.

## Results

### E1. Start, prompt and read an agent: works

- `herdr workspace create --cwd <repo> --label spike-main --no-focus` returns JSON with the workspace and root pane ids.
- `herdr worktree create --workspace w1 --branch spike/dev-1 --label dev-1 --no-focus` creates a real git worktree at `~/.herdr/worktrees/<repo>/<branch-slug>` on a new branch, opens a workspace for it and returns the checkout path and pane id. `--workspace` and `--cwd` are mutually exclusive; passing both prints the usage line.
- `herdr agent start dev --kind claude --pane w2:p1` failed with `agent_not_ready … blocked during startup` after 3.6 s: Claude Code shows a folder-trust dialog for every new worktree path. The dialog's default selection is **"No, exit"**. It was answered with `agent send-keys dev down` and `enter`; the state went `blocked` to `idle` in about 6 s.
- `herdr agent prompt dev "Reply with exactly the single word SPIKE_OK…" --wait --timeout 120000` returned in 4.2 s with `agent_status: done` and the Claude session id in the JSON. The reply was readable from outside with `herdr agent read dev --source recent-unwrapped --lines 80` (`● SPIKE_OK`), mixed with interface text such as `✻ Churned for 3s · done`.
- Panes inherit the environment of the process that started the Herdr server. Claude Code warned about an inherited `CLAUDE_CODE_CHILD_SESSION` marker. Whatever secrets are in the server's environment reach every agent.

### E2. An agent runs a `cstan`-style command with a per-agent credential: works

- The credential was set with `herdr pane run <pane> 'export SPIKE_TOKEN=… SPIKE_ROLE=developer'` before `agent start`; the agent inherited it.
- The agent ran `bin/cstan report --status done --commit abc123 --summary spike`. The stand-in recorded `{"result":"recorded","identity":{"role":"developer","agent":"dev"},"argv":[…]}` (6.4 s end to end). No permission prompt appeared because the operator's Claude settings use auto mode.
- A first version of the stand-in had a bug of its own (`jq --args "$@"` treats `--status` as a jq option); it was fixed with `--args --`. That is a tooling bug, not a finding.

### E3. Lifecycle states: accurate in the cases tried, but derived from the screen

- Trust dialog: `blocked` was reported, then `idle` after the answer.
- A real permission prompt (`touch /tmp/…` in default permission mode) was reported as `blocked`; `herdr agent explain` named the rule `bash_permission_prompt`. The prompt offered `1. Yes`, `2. Yes, and always allow…`, `3. Yes, and switch to auto mode`, `4. No`; the **default is "Yes"**. `agent send-keys dev2 esc` declined it, the state went to `done` and the file was not created.
- `agent explain` also showed that the ordinary `idle` state came from a screen rule (`live_prompt_box`, evidence `❯`). So Herdr's state is screen detection driven by a remotely updated manifest, not a hook report. It depends on Claude Code's interface text and on manifest updates.
- The default-mode agent did not prompt for a plain `echo`; prompts appear only for commands outside its allow rules.

### E4. Typed messages: three behaviors that matter

- Large prompt: a 12,811-byte single prompt with a sentinel at the very end arrived intact (the agent returned the last token) in 3.2 s.
- Text already in the input line: after `herdr pane send-text w2:p1 "OPERATOR_HALF_TYPED "`, a `herdr agent prompt` was appended to it. Claude received one message: `OPERATOR_HALF_TYPED Reply with only the words: PROMPT_TWO_RECEIVED`. Herdr neither rejected nor framed it. `agent send-keys dev ctrl+u` clears the line.
- Second prompt while the agent was working: both submissions returned success. The first command did run (`Ran 1 shell command`, `Worked for 17s`) but the first reply (`FIRST_DONE`) never appeared; only `SECOND_DONE` did. One reply answered two messages.

### E5. Impersonation: confirmed

The agent's credential is readable from `/proc/<pid>/environ` by any process of the same user. A separate process (the operator shell, not an agent) read it and ran `bin/cstan report --status done --commit FORGED_BY_OTHER_PROCESS`; the ledger accepted it as `developer/dev`. The credential labels a reporter; it does not authenticate one.

### E6. Bypass of the ledger: confirmed, and detectable by count

The ledger held 2 receipts while more than 8 prompts were sent directly with `herdr agent prompt`. Nothing in the ledger records them. Herdr's server log does record every call (`method="agent.prompt"`, request id `cli:agent:prompt`) but not the caller or the text, so a controller can detect unrouted prompts only by comparing counts.

### Additional finding: spawned agents load the operator's global Claude config

A spawned Claude Code loaded the operator's `~/.claude` hooks. The claw8 `PreToolUse:Write` guard blocked a file write and told the agent to run `claw8 workflow-start`. The agent did not go around the guard. A second agent started with `-- --settings '{"disableAllHooks":true}'` wrote the same file. `--bare` is not an option for this operator: it never reads OAuth and needs an API key. `--permission-mode default` is passed the same way (after `--`).

## What the redesign plan must change

1. Capstan sends every message itself (a `cstan send` path), one writer per pane, only to an agent whose state is `idle` or `done`, after clearing or checking the input line, in a framed envelope with a nonce. The agent confirms by a `cstan` receipt that names the nonce. This addresses E4 and the reviewer's typed-message finding.
2. Facts come from `cstan` reports. The controller verifies what it can itself (for example that the reported commit exists on that agent's branch and the generation matches), because E5 shows the credential proves nothing.
3. Herdr state is a hint, not authority. It comes from a remotely updated screen manifest (E3). Completion and blocking that matter must also be backed by a receipt or a timeout.
4. Never press Enter on a startup or permission prompt without reading it: the two dialogs default in opposite directions (E1, E3). Pre-trust worktrees and set per-role permission modes so prompts are rare; anything left goes to the operator.
5. Spawn agents with a controlled per-role config: hooks off through `--settings`, permission mode, allowed tools. This belongs in `capstan.toml`. Record that agents inherit the Herdr server's environment (E1).
6. Detect unrouted prompts by comparing Capstan's own send count with the `agent.prompt` count in Herdr's server log (E6), and state that this is detection, not enforcement.
7. Use `herdr worktree create` for worktrees; no custom worktree manager (E1).

## Open questions this spike did not answer

- Do Codex and OMP behave the same for start, prompt, read, state detection and dialogs?
- Does updating the Herdr Claude hook change state accuracy?
- How do long tasks, context growth and agent death look, and can a replacement agent be seeded from the ledger?
- Do several agents in one Herdr session interfere with each other's input or state?
- Can the Herdr server be started with a scrubbed environment so agents do not inherit the operator's secrets?
