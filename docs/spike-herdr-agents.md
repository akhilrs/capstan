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
- `herdr agent start dev --kind claude --pane w2:p1` failed with `agent_not_ready … blocked during startup` after 3.6 s: Claude Code showed a folder-trust dialog for the new worktree path (one path was tried; whether it appears for every new path, and whether pre-trusting a path avoids it, was not tested). The dialog's default selection is **"No, exit"**. It was answered with `agent send-keys dev down` and `enter`; the state went `blocked` to `idle` in about 6 s.
- `herdr agent prompt dev "Reply with exactly the single word SPIKE_OK…" --wait --timeout 120000` returned in 4.2 s with `agent_status: done` and the Claude session id in the JSON. The reply was readable from outside with `herdr agent read dev --source recent-unwrapped --lines 80` (`● SPIKE_OK`), mixed with interface text such as `✻ Churned for 3s · done`.
- Panes inherit the environment of the process that started the Herdr server. Claude Code warned about an inherited `CLAUDE_CODE_CHILD_SESSION` marker. One inherited marker was observed. That a child process inherits its parent's environment is standard behavior, so secrets in the server's environment should be assumed to reach agents, but only this one variable was seen.

### E2. An agent runs a `cstan`-style command with a per-agent credential: works in auto permission mode

- The credential was set with `herdr pane run <pane> 'export SPIKE_TOKEN=… SPIKE_ROLE=developer'` before `agent start`; the agent inherited it.
- The agent ran `bin/cstan report --status done --commit abc123 --summary spike`. The stand-in looked the environment token up in its credential table and recorded `{"result":"recorded","identity":{"role":"developer","agent":"dev"},"argv":[…]}` (6.4 s end to end).
- No permission prompt appeared because the operator's Claude settings use auto mode. In default permission mode E3 shows that commands outside the allow rules prompt, so a real `cstan` would have to be allow-listed for each role. Auto mode is also a security point: an agent that inherits it runs commands without asking, and spawned agents inherit it unless the per-role config says otherwise.
- A first version of the stand-in had a bug of its own (`jq --args "$@"` treats `--status` as a jq option); it was fixed with `--args --`. That is a tooling bug, not a finding.

### E3. Lifecycle states: accurate in the cases tried, but derived from the screen

- Trust dialog: `blocked` was reported, then `idle` after the answer.
- A real permission prompt (`touch /tmp/…` in default permission mode) was reported as `blocked`; `herdr agent explain` named the rule `bash_permission_prompt`. The prompt offered `1. Yes`, `2. Yes, and always allow…`, `3. Yes, and switch to auto mode`, `4. No`; the **default is "Yes"**. `agent send-keys dev2 esc` declined it, the state went to `done` and the file was not created.
- `agent explain` also showed that the ordinary `idle` state came from a screen rule (`live_prompt_box`, evidence `❯`). With the Claude hook that is installed here (outdated, v9 against v10, never updated), the states I observed came from screen detection driven by a remotely updated manifest. Whether an updated hook would report state directly, and change accuracy, was not tested. Screen detection depends on Claude Code's interface text and on manifest updates.
- The default-mode agent did not prompt for a plain `echo`; prompts appear only for commands outside its allow rules.

### E4. Typed messages: three behaviors that matter

- Large prompt: a 12,811-byte single prompt with a sentinel at the very end arrived intact (the agent returned the last token) in 3.2 s.
- Text already in the input line: after `herdr pane send-text w2:p1 "OPERATOR_HALF_TYPED "`, a `herdr agent prompt` was appended to it. Claude received one message: `OPERATOR_HALF_TYPED Reply with only the words: PROMPT_TWO_RECEIVED`. Herdr neither rejected nor framed it. `agent send-keys dev ctrl+u` clears the line.
- Second prompt while the agent was working: both submissions returned success. The first command did run (`Ran 1 shell command`, `Worked for 17s`) but the first reply (`FIRST_DONE`) never appeared; only `SECOND_DONE` did. I read this as one reply answering two messages; that is my interpretation of a single observation, not a confirmed mechanism.

### E5. Impersonation: confirmed

An agent's environment-variable token is readable from `/proc/<pid>/environ` by any process of the same user. A separate process (the operator shell, not another agent) read it and ran `bin/cstan report --status done --commit FORGED_BY_OTHER_PROCESS`; the ledger accepted it as `developer/dev`. So an environment token labels a reporter and does not authenticate one. Only this delivery method was tried. A descriptor, file permissions or a separate user per agent were not tested, and no agent-to-agent read was attempted.

### E6. Bypass of the ledger: confirmed, and detectable by count

By the time of this check 8 prompts had been sent directly with `herdr agent prompt`, and the ledger held 2 receipts, both the agent's own reports. Nothing in the ledger records the prompts. Herdr's server log showed start and completion lines for the `agent.prompt` calls I looked at (`method="agent.prompt"`, request id `cli:agent:prompt`) but no caller and no text. Input can also enter through `pane send-text`, `send-keys` and `pane run` (E4 used `send-text`); I did not check whether those appear in the log. Counting `agent.prompt` calls would therefore give at most partial detection, and it rests on a single run.

### Additional finding: spawned agents load the operator's global Claude config

A spawned Claude Code loaded the operator's `~/.claude` hooks. The claw8 `PreToolUse:Write` guard blocked a file write and told the agent to run `claw8 workflow-start`. The agent did not go around the guard. A second agent started with `-- --settings '{"disableAllHooks":true}'` wrote the same file. Only hooks were checked: the rest of `~/.claude` (CLAUDE.md rules, MCP servers, permission rules) was not examined and may still apply. Disabling all hooks also removes any guard the operator would want to keep. `--bare` is not an option for this operator: it never reads OAuth and needs an API key. `--permission-mode default` is passed the same way (after `--`).

## What the redesign plan must change

1. Capstan sends every message itself (a `cstan send` path), one writer per pane, only to an agent whose state is `idle` or `done`, after clearing or checking the input line, in a framed envelope with a nonce. The agent confirms by a `cstan` receipt that names the nonce. The state check only lowers the chance of a collision, because the state can change between the check and the send (E4); the nonce receipt, not the state check, is what shows a message was delivered and understood. This addresses E4 and the typed-message concern raised in the external review of the redesign plan.
2. Facts come from `cstan` reports. The controller verifies what it can itself (for example that the reported commit exists on that agent's branch and the generation matches), because E5 shows an environment-variable token cannot be trusted as proof of identity. Other ways to deliver a credential were not tried.
3. Herdr state is a hint, not authority. With the installed hook it came from a remotely updated screen manifest (E3). Completion and blocking that matter must also be backed by a receipt or a timeout.
4. Never press Enter on a startup or permission prompt without reading it: the two dialogs default in opposite directions (E1, E3). Pre-trust worktrees and set per-role permission modes so prompts are rare; anything left goes to the operator.
5. Spawn agents with a controlled per-role config: hooks off through `--settings` (which also drops guards the operator may want; the rest of `~/.claude` was not examined), permission mode, allowed tools. This belongs in `capstan.toml`. Record that agents inherit the Herdr server's environment (E1).
6. Try to detect unrouted prompts by comparing Capstan's own send count with the `agent.prompt` count in Herdr's server log (E6). This is partial detection at best: other input paths (`send-text`, `send-keys`, `pane run`) were not checked, and it is not enforcement.
7. Prefer `herdr worktree create` for worktrees over a custom manager (E1). It worked for one create and one forced remove; branch clashes, cleanup after failures and several agents at once were not tested.

## Open questions this spike did not answer

- Do Codex and OMP behave the same for start, prompt, read, state detection and dialogs?
- Does updating the Herdr Claude hook change state accuracy?
- How do long tasks, context growth and agent death look, and can a replacement agent be seeded from the ledger?
- Do several agents in one Herdr session interfere with each other's input or state?
- Can the Herdr server be started with a scrubbed environment so agents do not inherit the operator's secrets?
