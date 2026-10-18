# Research: adding a "pi" host kind to Capstan (pi.dev)

## Executive summary

- **Pi** (pi.dev) is a minimal, MIT-licensed terminal coding agent ("harness") by Earendil Inc. / Mario Zechner. npm package `@earendil-works/pi-coding-agent`, binary `pi`, source `github.com/earendil-works/pi` (older name `badlogic/pi-mono`, package scope `@mariozechner`). Version installed on this machine: **0.84.4**. Needs Node 22.19+.
- **omp (oh-my-pi) is a fork of Pi** by Can Bölük (`can1357`), with its own binary, config dir (`~/.omp/agent`) and many added features. Capstan's `omp` host is therefore *related* but not the same program; Pi's CLI and TUI cannot be assumed identical to omp 18.3.1.
- **Pi has no permission prompts at all** (stated on pi.dev) and no `--approval-mode`/yolo flag. That is good for Capstan: no approval flag is needed and no permission dialog needs relaying. The only dialog likely to block an unattended start is the **project-trust question** (answered by `--approve`/`-a`).
- Pi has a documented **RPC mode** (`--mode rpc`, JSONL over stdin/stdout) with an explicit `agent_settled` event, and a print/JSON mode. The existing Capstan host model is different: it types into a Herdr terminal pane and reads the screen. A pane-based `pi` host is the smaller change and can reuse most of the omp path; RPC would be a new architecture.
- A minimal pi host kind touches: `HOST_KINDS` (2 places), `hosts.rs` (new `pi_arguments`), `kernel.rs` (one match arm), `screen.rs` (new `pi_input_line`, possibly trust dialog), config validation/docs/starter, and tests plus screen fixtures. **The key unknown is Pi's TUI screen layout (input line shape)**, which I could not capture (see Could not verify). Estimated reuse of omp: high for launch/prompt-file/args/config/docs/tests scaffolding, none for the screen parser.

## Method

- Date of research: 2026-10-18.
- Fetched the primary pages with WebFetch (`pi.dev`, and raw files under `github.com/earendil-works/pi/packages/coding-agent/`: `README.md`, `docs/cli.md`, `docs/rpc.md`, `docs/json.md`, `docs/usage.md`, `docs/settings.md`, `docs/configuration.md`). WebFetch summarises pages with a small model, so wording below is a summary, not verbatim.
- WebSearch (standard): "pi.dev coding agent pi-mono badlogic CLI"; "oh-my-pi omp can1357 relation to pi coding agent".
- Ran `pi --version` and `pi --help` locally (the CLI is installed here) to cross-check flags; `omp --version` gave `omp/18.3.1`. Did not run Pi's TUI or any model call.
- Read this repo: `rust/crates/herdr/src/{hosts,screen,api,prompt_relay}.rs`, `adapter/{mod,input,panes}.rs`, `rust/crates/launcher/src/kernel.rs`, `rust/crates/config/src/{types.rs,starter.toml}`, `docs/reference/configuration.md`, fixture list in `rust/testdata/fixtures`.
- No Reddit or Hacker News queries were made; the question is product-documentation shaped.

## Findings

### 1. What Pi is

Consensus (pi.dev, repo README, search summaries agree):
- Minimal terminal coding agent; "ships with defaults but leaves out features like sub-agents and plan mode", which are added as extensions/packages (pi.dev).
- Licence MIT; source `github.com/earendil-works/pi` (`packages/coding-agent`); install `curl -fsSL https://pi.dev/install.sh | sh` or `npm install -g --ignore-scripts @earendil-works/pi-coding-agent` (pi.dev). Node 22.19+ (README).
- Providers: Anthropic, OpenAI, Google, Azure, Bedrock, Mistral, Groq, Cerebras, xAI, Hugging Face, Kimi, MiniMax, NVIDIA, OpenRouter, Ollama "and more"; API key or OAuth (`/login`); custom providers via `models.json` or extensions (pi.dev). `pi --help` here says the default provider is `google`.
- Four modes: interactive TUI, print/JSON, RPC, SDK (pi.dev, README, third-party write-ups).
- Relationship to omp: omp is a fork of Pi by Can Bölük; copyright notice lists both authors; installed as `@oh-my-pi/pi-coding-agent`, binary `omp`, data in `~/.omp/agent` (search summaries of cmux docs, wener.me, innfactory.ai, terminaltrove). omp adds LSP, debugger, subagents, MCP etc.

Isolated: star count ~45k and package-count claims come from third-party mirrors only.

### 2. Running Pi, sending tasks, busy/idle signals

Consensus (pi.dev, `docs/cli.md`, local `pi --help` agree):
- Print: `pi -p "prompt"` writes the final assistant text and exits. If stdin or stdout is redirected and no json/rpc mode is set, Pi uses print mode. Piped stdin is prepended to the first prompt; `@path` attaches files.
- JSON: `--mode json` writes a session header then JSONL events, exits after the supplied prompts. Events: `agent_start`, `turn_start`, `tool_execution_start/update/end`, `turn_end`, `agent_end` (has `willRetry`), `agent_settled` (has `aborted`) (`docs/json.md`).
- RPC: `pi --mode rpc --no-session`; one JSON object per LF-terminated line; commands include `prompt`, `steer`, `follow_up`, `abort`; each accepts an optional `id`. `prompt` response means accepted, not finished. **Busy from accepted `prompt` until `agent_settled`; `agent_end` is not idle.** Close stdin for orderly exit (`docs/rpc.md`; the details of `steer`/`follow_up` were not visible to me).
- Interactive TUI (`docs/usage.md`): Enter sends; while Pi works, **Enter queues a steering message** (delivered after current response and tool calls), **Alt+Enter queues a follow-up**, Escape aborts and returns queued messages to the editor, Alt+Up pulls queued messages back. Footer shows folder, session, model, context usage, cost. `--tui-mode` selects `fullscreen` or `regular`.
- Slash commands: `/model`, `/thinking`, `/login`, `/settings`, `/new`, `/resume`, `/tree`, `/fork`, `/compact`, `/export`, `/reload`, etc.

### 3. Permissions and approvals

Consensus (pi.dev says it; `docs/cli.md` and local `--help` show no approval-mode flag):
- "Pi has no built-in permission popups." Confirmation flows are built with extensions (examples `permission-gate.ts`, `protected-paths.ts`, `sandbox/` in the repo). Container use is the suggested isolation.
- Tool control flags: `--tools` allowlist, `--exclude-tools`, `--no-tools`, `--no-builtin-tools`. Default tools: `read`, `bash`, `edit`, `write`.
- Trust: project-local config under `.pi/` loads only after project trust is granted. Interactive first start "may ask whether you trust it" (usage.md). `--approve/-a` trusts project-local files for the run, `--no-approve/-na` ignores them; `defaultProjectTrust` = `ask|always|never` in agent-dir settings. Context-file (AGENTS.md) discovery does not require trust (configuration.md).
- Extension points: TypeScript extensions (tools, commands, shortcuts, event handlers, status bars), skills, prompt templates, themes, packages (`pi install npm:...`/`git:...`).

Implication: unattended running needs no "yolo" flag. Because there is no sandbox either, the same unsandboxed warning Capstan prints for codex/omp applies.

### 4. Config, context, sessions, MCP, commands

Consensus (`docs/configuration.md`, pi.dev):
- Global dir `~/.pi/agent` (override env `PI_CODING_AGENT_DIR`): `settings.json`, `keybindings.json`, `mcp.json`, `models.json`, `auth.json`, `SYSTEM.md`/`APPEND_SYSTEM.md`, `extensions/ skills/ prompts/ themes/`. Project `.pi/` has the same minus auth/keybindings; project files override agent-dir files of the same name.
- Context files: `AGENTS.override.md`, `AGENTS.md`, `CLAUDE.md` (and upper-case-extension variants) from the agent dir, cwd and parents; `-nc` disables discovery. So Pi reads both AGENTS.md and CLAUDE.md natively.
- System prompt: `--system-prompt <text|path>` replaces; `--append-system-prompt <text|path>` appends (repeatable). The flag name and "file path" semantics match what Capstan already passes to omp.
- Sessions: stored as a tree in one file; `-c` continue, `-r` resume selector, `--session <path|id>`, `--session-id`, `--fork`, `--session-dir` (env `PI_CODING_AGENT_SESSION_DIR`), `--no-session`, `-n` name.
- MCP: built in via the `builtin:mcp` extension; `<agent-dir>/mcp.json` and `.pi/mcp.json`; `--mcp-config <path>` (extension flag in local help); `--no-mcp`. Custom commands: prompt templates (`/name`), skills, extension commands.

### 5. What Capstan needs for a `pi` host (from reading this repo)

How hosts work today: `launcher/src/kernel.rs:1043-1058` (`arguments`) picks args per host kind (`codex_arguments`, `omp_arguments`, otherwise Claude). The adapter writes the role prompt to a private file, starts the command in a Herdr pane, then types messages into the pane; state comes from Herdr's own `status` (`idle`/`done` vs busy, `adapter/input.rs:16-22`) plus a screen read of the input line (`screen.rs::extract_input_line`, kind-specific for `shell`, `codex`, `omp`, `claude`). `HOST_KINDS = ["claude","codex","omp"]` exists twice (`herdr/src/api.rs:453`, `config/src/types.rs:9`) and gates the adapter (`adapter/mod.rs:602`, `panes.rs:532`) and config (`config/resolve.rs:664`). Trust dialogs: `screen.rs::trust_texts/parse_trust_dialog_of` cover claude and codex only; omp has none. Permission-prompt relay: `parse_host_prompt` returns None for every host except claude, so codex and omp already get no relay.

Needed for pi:

| Area | Change | Notes |
|---|---|---|
| Kind registration | add `"pi"` to both `HOST_KINDS` arrays; update the `[hosts.<name>]` doc row | config enum error messages list kinds |
| Launch command | new `pi_arguments(role, prompt_file)` in `hosts.rs` beside `omp_arguments`, plus a `"pi" =>` arm in `kernel.rs` | candidate args: `--model <m>` (reuse `require_no_dash`), `--append-system-prompt <prompt_file>`, `--approve` to pre-trust the worktree. **No approval flag.** Likely also `--offline` (no startup network) and `--tui-mode` pin for a stable screen (see unknowns). Default `command` is presumably `pi` (check how `command` defaults are derived for omp). |
| Prompt delivery | reuse the existing pane typing path | Enter while busy queues a *steering* message in Pi; Capstan only types when Herdr says idle, so that rarely matters. |
| Idle/busy | depends on Herdr's agent detection | Herdr reports `status`; whether Herdr recognises a `pi` process as an agent (it does for omp) is **unverified**. If it does not, a fallback is needed (screen heuristics or RPC). |
| Input-line parsing | new `pi_input_line` in `screen.rs` and arm in `extract_input_line` | omp's parser expects `╰─ text` as the last line; Pi's editor layout is unknown to me. Needs captured `.ansi` fixtures (`pi-<ver>-idle-empty/typed/after-ctrl-u`) as done for omp/codex in `rust/testdata/fixtures`. Also confirm Ctrl-U clears the input as the clear routine assumes. |
| Trust dialog | add Pi entry to `trust_texts`/`parse_trust_dialog_of` or avoid it with `--approve` | `--approve` is the cheaper route; confirm it suppresses the question. |
| Prompt relay | none needed | Pi has no permission prompts; `parse_host_prompt` already returns None for non-claude. The relay config stays Claude-only. |
| Report flow | unchanged | commit-message/branch checks are host-independent (docs/reference/configuration.md). Pi was not checked for adding commit trailers; the report check is the safety net. |
| Config keys | role rules identical to codex/omp: Developer/Verifier only, own `model` required, `permission_mode` acceptEdits/auto, no allow/deny; unsandboxed warning in `cstan config check` must cover pi | find where the warning/role-restriction logic names `codex`/`omp` (not located exhaustively; grep `resolve.rs`). Pi's default provider is `google`, so a role `model` like `provider/id` is required. |
| Docs | `docs/reference/configuration.md` "Running a worker on Codex or OMP" section and host table; `starter.toml` commented `[hosts.pi]` example; CHANGELOG | add "Checked against Pi 0.84.4" |
| Tests | parity cases in `rust/crates/herdr/tests/parity*` (`adapter.json`, `screen.json`), `config` corpus/expected JSON, launcher `spawn.json`, cstan transcripts that print the host list | follow what omp added; fixtures must be real captures |

Reuse of omp: high. omp and pi share the `--append-system-prompt <file>` and `--model` flags (same fork lineage), so `pi_arguments` is a 6-line sibling of `omp_arguments` minus `--approval-mode yolo`. Config, adapter lifecycle, report flow, docs structure and test scaffolding follow the omp pattern. Not reusable: the screen parser (unless Pi's TUI turns out to share omp's `╰─` editor, which I could not confirm) and any assumptions about omp-specific flags.

Alternative (not recommended first): drive Pi through `--mode rpc` for reliable `agent_settled` idle detection and structured `prompt`/`follow_up`. It would need a new non-pane transport, does not fit the Herdr pane model (operators watch panes), and is much larger than a new host kind.

## Conflicts and disagreements

- **Package scope/repo**: search results mention both `@mariozechner/pi-coding-agent` (badlogic/pi-mono) and `@earendil-works/pi-coding-agent` (earendil-works/pi). pi.dev currently points to the latter; the former looks like the earlier name. I did not check redirect history.
- **`--tui-mode` default**: `docs/cli.md` says `fullscreen` is the default; local `pi --help` (0.84.4) says `regular (default)`. Matters for screen parsing; pin it explicitly.
- **pi.dev vs omp**: omp keeps an approval system (`--approval-mode`), Pi has none; do not assume omp flags exist in Pi.
- Third-party sources disagree on package counts (5 vs 7).

## Sources

All accessed 2026-10-18. Publication dates are not shown on these pages unless stated.

1. Pi homepage, pi.dev, Earendil Inc. — https://pi.dev/
2. Pi coding-agent README — https://raw.githubusercontent.com/earendil-works/pi/main/packages/coding-agent/README.md (earendil-works/pi)
3. CLI flags — https://raw.githubusercontent.com/earendil-works/pi/main/packages/coding-agent/docs/cli.md
4. RPC mode — https://raw.githubusercontent.com/earendil-works/pi/main/packages/coding-agent/docs/rpc.md
5. JSON event stream — https://raw.githubusercontent.com/earendil-works/pi/main/packages/coding-agent/docs/json.md
6. Interactive usage — https://raw.githubusercontent.com/earendil-works/pi/main/packages/coding-agent/docs/usage.md
7. Settings — https://raw.githubusercontent.com/earendil-works/pi/main/packages/coding-agent/docs/settings.md
8. Configuration — https://raw.githubusercontent.com/earendil-works/pi/main/packages/coding-agent/docs/configuration.md
9. pi.dev: The Coding Agent That Trusts You To Know What You're Doing, urandom.io blog, 2026-03-07 (URL date) — https://urandom.io/blog/2026-03-07-pi-dev-deep-dive (search snippet only)
10. omp vs. Pi, Terminal Trove — https://terminaltrove.com/compare/ai-coding-agents/oh-my-pi-vs-pi/ (search snippet only)
11. oh my pi, cmux docs — https://cmux.com/docs/agent-integrations/oh-my-pi (search snippet only)
12. oh my pi, wener.me notes — https://wener.me/notes/ai/agent/oh-my-pi (search snippet only)
13. omp, innfactory.ai — https://innfactory.ai/en/ai-harness/omp/ (search snippet only)
14. Local: `pi --version` (0.84.4), `pi --help`, `omp --version` (omp/18.3.1); this repository at commit bd81272.

## Could not verify

- **Pi's TUI screen layout** (empty and typed input line, busy indicator, trust dialog wording) — I did not run the interactive TUI, so no fixture exists. This decides the size of the `screen.rs` work.
- Whether `--approve` fully suppresses the trust question for a fresh worktree, and what the question looks like.
- Whether Herdr detects a `pi` pane as an agent and reports idle/busy/done for it.
- Whether Ctrl-U clears Pi's input line as Capstan's clear routine assumes.
- Details of RPC `steer`/`follow_up` payloads (the summary tool could not see the RPC commands page).
- That Pi adds no commit trailers to commits (not tested).
- The omp relationship rests on third-party pages, not omp's own README; I did not open github.com/can1357/oh-my-pi.
- Exact repo rename history badlogic/pi-mono to earendil-works/pi; the Earendil posts (pi-1-0, pi-durable) were not read.
- WebFetch output is a model summary of each page, so flag lists should be re-checked against `pi --help` (done locally for the main flags only).
- Where in `config/resolve.rs` the codex/omp role restrictions and the unsandboxed warning are coded; I did not read it fully.

## Captured TUI (pi 1.3.0)

The sections above were researched on pi 0.84.4. While the capture was under way the `pi` on this machine became **1.3.0** (installed by another process), so every screen and fact below is from `pi --version` = `1.3.0` (a first 0.84.4 capture was discarded). Fixtures: `rust/testdata/fixtures/pi-130-<state>.ansi`.

### How the screens were captured

Scratch git repos outside the project (in the session scratchpad; one with a committed `.pi/settings.json` containing `{}`), a Herdr pane opened with `herdr pane split <own pane> --direction right`, and the launch command the host would use:

```
clear; cd <scratch repo> && pi --approve --tui-mode regular --model anthropic/claude-haiku-4-5 --append-system-prompt <tmpfile with "Be brief.">
```

After about 7 s the screen was read with the call `HerdrAdapter::screen` makes:

```
herdr pane read <pane> --source visible --lines 200 --ansi > file
```

Typing used `herdr pane send-text <pane> "<text>"`, Enter and Ctrl-U used `herdr pane send-keys <pane> enter` / `ctrl+u` (the adapter sends the key name `ctrl+u`; `C-u` is rejected with `invalid_key`). Only panes opened for the capture were closed. A `clear` ran before pi so the read holds no shell prompt. Redactions applied to the raw read: the scratch path became `/tmp/scratch/repo`, `/home/<user>` became `/home/user`; no keys or session ids appear on screen.

Fixtures: `idle-empty`, `idle-typed` (`hello capstan`), `after-ctrl-u`, `idle-multiline` (a 26-word line that wraps onto two editor lines in a 120-column pane), `busy` (one Haiku call: "Write a 400 word story about a lighthouse. No tools."), `trust-dialog`.

The user's own extensions and skills add noise to the banner (the `[Context]`, `[Skills]`, `[Extensions]`, `[Extension issues]` blocks, extension warnings and a "Package Updates Available" box). They sit above the editor, so a parser must anchor on the bottom block, not on the top of the screen.

### Screen layout

Bottom of every idle screen, in this order (the last five lines of the read):

```
─────────────────────────────── (rule)
<editor line(s)>               (empty, or the typed text; wrapped text takes more lines)
─────────────────────────────── (rule)
<cwd>                          (path, truncated with "..." when long)
<stats> ... (provider) <model> • <thinking>
🔌 MCP: 2 servers enabled      (extension status line; user-specific, may be absent)
```

- The editor has **no prompt glyph** (no `>` or `❯`): the input line is the text between the two horizontal rules, padded with spaces. Empty editor = one all-space line. Wrapped text occupies one line per wrapped row, breaking at a space (`...romeo sierra` / `tango uniform ...`), with no continuation marker.
- Before the first model call the stats line starts `0.0%/200k (auto)`; after a call it starts with token and cost counters (`↑10 ↓604 W23k CH0.0% $0.031 11.6%/200k (auto)`).
- **Busy**: the line above the lower editor rule changes. In 1.3.0 the top rule of the editor becomes `── ⠙ Working ──────…` (spinner glyph changes between reads); the editor stays visible and empty. The user's prompt and the streamed reply are printed above it. (Seen on 1.3.0 only; the discarded 0.84.4 read had a separate ` ⠙ Working...` line above the rules.)
- **Done**: the `Working` rule is a plain rule again; the reply stays above the editor.

### Answers to "Could not verify"

- **TUI layout**: captured, see above. Parser work for `screen.rs`: input line = the lines between the last two rules, minus space padding; busy = a top rule containing `Working`.
- **`--approve` and trust**: without `--approve`, a start in a repo with `.pi/settings.json` shows a blocking dialog (`pi-130-trust-dialog.ansi`): `Trust project folder?`, the path, "This allows pi to load .pi settings and resources, install missing project packages, and execute project extensions.", options `→ Trust`, `Trust parent folder (<parent>)`, `Trust (this session only)`, `Do not trust`, `Do not trust (this session only)`, footer `↑↓ navigate  enter select  escape/ctrl+c cancel`. With `--approve` the same start in the same repo shows **no question** and goes straight to the editor. `--approve` is per run: it did not write to `~/.pi/agent/trust.json`. While the dialog is up, Herdr reports `agent_status` `unknown` (the pi extension that reports state has not started), so a dialog is distinguishable from idle by status as well as by text. Pin `--approve`; the dialog fixture is only for detecting a start without it.
- **Herdr detects pi**: yes. `herdr pane get <pane>` reports `"agent":"pi"` and `agent_status` `idle` (editor shown, before and after typing), `working` (from submit until the reply ends) and `done` (reply finished, editor empty). `pane get` also carries an `agent_session` of kind `path`. Detection here comes from a Herdr state-reporting extension loaded by pi (`herdr-agent-state.ts` in the user's pi setup, `source: herdr:pi`); a machine without that extension was not tested, so the pi-host package should not rely on status alone without a screen fallback for the `unknown` case.
- **Ctrl-U**: clears the whole editor content in one press, also text wrapped over two lines. After `ctrl+u` the screen equals `idle-empty` (checked with a diff of the plain text).
- **`--tui-mode`**: `regular` and `fullscreen` give a **bottom-anchored** editor in the visible read, and the idle, typed, ctrl-u, multiline and busy reads were byte-identical between the two. Pin `--tui-mode regular` explicitly: the 1.3.0 default is `fullscreen`,; `regular` is the 0.84.4 default the research assumed (scrollback behaviour of `--source recent` was not tested). Either is safe for the screen parser.
- **Banner and `--offline`**: a startup banner always appears (context files, skills, extensions); on the first start after an update a changelog list ("Fixed ...") also scrolled in. A "Package Updates Available" box appears when extensions or pi have updates. `--offline` removed that box; the model call afterwards worked (reply `ok`, status `done`). The context/skills/extensions blocks and extension warnings stay with `--offline`. Recommend `--offline` for hosts so the update notice and the update check cannot alter the screen.
- **First-run dialogs other than trust**: none appeared (credentials were already configured). The changelog list is not a dialog.
- **Commit trailers**: not tested.
- **Still open from the research**: RPC `steer`/`follow_up` payloads, omp relationship, repo rename history and the `config/resolve.rs` question; they are outside this capture.

### Flag re-check (pi 1.3.0 `pi --help`)

Each flag was also confirmed by a real start with it (`--approve`, `--tui-mode regular|fullscreen`, `--model anthropic/claude-haiku-4-5`, `--append-system-prompt <file>`, `--offline`).

| Flag | 1.3.0 help line | Change from 0.84.4 |
|---|---|---|
| `--approve`, `-a` | `Trust project-local files for this run` | none; also `--no-approve, -na` |
| `--tui-mode` | `TUI mode: fullscreen (default) or regular` | **default changed** from `regular` to `fullscreen`; names and values unchanged |
| `--model` | `Model pattern or ID (supports "provider/id" and optional ":<thinking>")` | none |
| `--append-system-prompt` | `Append text or file contents to the system prompt (can be used multiple times)` | none; a file path works |
| `--offline` | `Disable startup network operations (same as PI_OFFLINE=1)` | none |
| `--provider` | `Provider to search for --model (requires --model)` | meaning changed: 0.84.4 said `Provider name (default: google)`; no host impact when `--model` is `provider/id` |

0.84.4 claims in this document that no longer hold:

- "`pi --help` ... says `regular (default)`" (Conflicts section): in 1.3.0 `--help` and `docs/cli.md` agree on `fullscreen`.
- The task for the pi host should read "Checked against Pi 1.3.0", not 0.84.4 (line "add Checked against Pi 0.84.4" in the touch-point table).
- Every "Could not verify" bullet answered above is no longer unverified.
