# `cstan` command reference

Back to the [README](../../README.md).

Checked against the `usage` string in `src/cli.ts` (run `cstan` with no arguments to print it), the `ROUTES` table in `src/daemon.ts` and the handlers in `src/commands.ts`. "Who" shows which credential the daemon accepts: operator (the key in `.capstan/`), agent (an agent's environment token), or either. Every routed command accepts `--json`.

### Project and daemon (operator)

| Command                                                     | Purpose                                                                       |
| ----------------------------------------------------------- | ----------------------------------------------------------------------------- |
| `cstan --version`, `-V`, `version`                          | Print `cstan <version>` and exit 0. Needs no daemon, `.capstan/` or git repo. |
| `cstan --help`, `-h`, `help`                                | Print the usage text to stdout and exit 0.                                    |
| `cstan init`                                                | Create `.capstan/` and a starter `capstan.toml`.                              |
| `cstan start`                                               | Start the daemon and launch the PM.                                           |
| `cstan stop`                                                | Stop the daemon.                                                              |
| `cstan ping`                                                | Check that the daemon answers.                                                |
| `cstan config check`                                        | Validate `capstan.toml` and print the resolved config (warnings on stderr).   |
| `cstan config sync`                                         | Write role definitions into the ledger (daemon stopped).                      |
| `cstan herdr-config`                                        | Print an optional Herdr `config.toml` snippet.                                |
| `cstan pm restart`                                          | Replace the PM session, seeded from the ledger.                               |
| `cstan resolve <message-id> retry\|skip\|cancel ["<note>"]` | Settle a blocked message.                                                     |
| `cstan cancel <message-id>`                                 | Cancel a message.                                                             |

`cstan daemon` is the internal command that `cstan start` runs in the background; you do not need to run it. `cstan assign` and `cstan ask` are listed in the usage string but answer `not_implemented` in this version.

### Look and read

| Command                                                     | Who            | Purpose                                                                  |
| ----------------------------------------------------------- | -------------- | ------------------------------------------------------------------------ |
| `cstan status [--json]` / `--watch [--interval N]`          | either         | Project state.                                                           |
| `cstan inspect <id> [--json]`                               | operator       | One ledger record.                                                       |
| `cstan dash [--interval N] [--no-color] [--reduced-motion]` | operator       | Dashboard.                                                               |
| `cstan inbox [<agent-id>]`                                  | either         | Agent: own pending messages. Operator: an agent's mailbox (id required). |
| `cstan observe <agent-id> [lines]`                          | PM, Supervisor | Another agent's recent screen.                                           |

### Messaging and workers

| Command                               | Who    | Purpose                                   |
| ------------------------------------- | ------ | ----------------------------------------- |
| `cstan send <agent-id\|@pm> "<text>"` | either | Queue a message.                          |
| `cstan wait`                          | PM     | Block for new messages.                   |
| `cstan ack <message-id>`              | agent  | Acknowledge a message.                    |
| `cstan spawn <role>`                  | either | Start a worker.                           |
| `cstan release <agent-id>`            | either | End a worker, free its pane and worktree. |
| `cstan replace <agent-id>`            | either | Replace a lost or stuck worker.           |

### Delivery flow

| Command                                                   | Who                 | Purpose                                            |
| --------------------------------------------------------- | ------------------- | -------------------------------------------------- |
| `cstan report <commit> "<summary>"`                       | Developer, Verifier | Report a finished commit (full 40-character id).   |
| `cstan request-review <report-id\|integration-id> [role]` | PM                  | Start an independent review.                       |
| `cstan review pass\|findings "<text>"`                    | Verifier            | Answer a review request once.                      |
| `cstan integrate <report-id>...`                          | PM or operator      | Merge reviewed reports onto an integration branch. |
| `cstan integrate confirm\|discard <integration-id>`       | PM or operator      | Settle a merged integration.                       |

### Supervision

| Command                                                                         | Who        | Purpose          |
| ------------------------------------------------------------------------------- | ---------- | ---------------- |
| `cstan finding <agent-id> <severity> "<evidence>" "<correction>" "<done-when>"` | Supervisor | Raise a finding. |
| `cstan finding check <finding-id> resolved\|unresolved "<evidence>"`            | Supervisor | Check a finding. |

### Planned work (with `[architect] enabled = true`)

| Command                                                         | Who            | Purpose                                                                        |
| --------------------------------------------------------------- | -------------- | ------------------------------------------------------------------------------ |
| `cstan plan open normal\|high-risk "<title>" [<superseded-id>]` | PM or operator | Open a draft plan; a trailing plan id supersedes an approved plan.             |
| `cstan plan submit <plan-id> "<json>"`                          | Architect      | Submit the plan body (packages, owned areas, dependencies, acceptance, risks). |
| `cstan plan show [<plan-id>]`                                   | any            | List plans, or one plan with its packages, assignees and progress.             |
| `cstan plan assign <plan-id> <package-id> <agent-id>`           | PM or operator | Assign a package; the controller sends the package text to the developer.      |
| `cstan plan signoff <plan-id> <integration-id> "<summary>"`     | Architect      | Sign off a reviewed integration; the PM is told the branch to hand to you.     |
| `cstan plan cancel <plan-id> [<package-id>]`                    | operator       | Cancel a plan or one package.                                                  |

### Nexora links

| Command                                                                | Who            | Purpose                                                       |
| ---------------------------------------------------------------------- | -------------- | ------------------------------------------------------------- |
| `cstan link requirement\|plan\|package <ref-id> <nexora-id> [<state>]` | PM or operator | Record a Nexora item id and the state last written to Nexora. |
| `cstan link bind <requirement-ref-id> <agent-id>`                      | PM or operator | Tie a small-tier requirement to the developer working on it.  |

### Prompt relay (with `[prompt_relay] enabled = true`)

| Command                                                                       | Who       | Purpose                                                                                                             |
| ----------------------------------------------------------------------------- | --------- | ------------------------------------------------------------------------------------------------------------------- |
| `cstan prompt show <agent-id>`                                                | active PM | Capture a blocked worker's permission prompt, or an unrecognised blocking dialog (Esc only), with options and hash. |
| `cstan prompt answer <relay-id> --hash <hash12> option <n>\|esc\|text <text>` | active PM | Type the answer the user chose; a dialog relay takes `esc` only and reports `inputReadable`.                        |

### Operator (with `[operator] enabled = true`)

| Command                                                                          | Who                        | Purpose                                              |
| -------------------------------------------------------------------------------- | -------------------------- | ---------------------------------------------------- |
| `cstan op propose "<command>" "<reason>"`                                        | Operator                   | Propose one shell command.                           |
| `cstan op propose --restart [--force] "<reason>"`                                | Operator                   | Propose a controller restart.                        |
| `cstan op decide <id> approve --hash <hash12> [--session exact\|prefix=<words>]` | PM                         | Approve a proposal, optionally with a session grant. |
| `cstan op decide <id> deny ["<note>"]`                                           | PM or operator             | Deny a proposal.                                     |
| `cstan op show [<id>]` / `cstan op cancel <id>`                                  | see the Operator reference | Show proposals and runs / withdraw a proposal.       |
| `cstan op grants` / `cstan op revoke <grant-id>`                                 | PM or operator             | List / end session grants.                           |
| `cstan op full-auto on [<minutes>] --asked-user "<text>"`                        | PM                         | Switch every guard off for a limited time.           |
| `cstan op full-auto off` / `status`                                              | PM or operator             | Switch full auto off / show the time left.           |

See the [Operator reference](operator.md) for the rules behind each command.

Arguments may not be empty and may not contain invalid UTF-8. Put long text in a quoted heredoc so the shell does not expand it:

```sh
cstan send developer-1 "$(cat <<'EOF'
Task text here.
EOF
)"
```

## Controller messages

Messages whose sender is `controller` start with one of these prefixes (`src/controller/core.ts`, `src/prompts.ts`):

| Prefix                            | Meaning                                                                                      |
| --------------------------------- | -------------------------------------------------------------------------------------------- |
| `Verified report`                 | A report was accepted: the commit exists on that worker's branch. Not a review.              |
| `Review request`                  | To a reviewer: the commit (or integration) to review.                                        |
| `Review`                          | To the PM: the reviewer's verdict as recorded. The text inside is the reviewer's opinion.    |
| `Finding`                         | A supervisor raised, resolved, escalated or cancelled a finding.                             |
| `Delivery problem`                | A message to a worker is `unacked`, `expired` or `failed`; run the `cstan resolve` it names. |
| `Agent stalled` / `Agent blocked` | A worker made no progress / waits at a prompt.                                               |
| `Agent ... is lost`               | Herdr no longer finds the agent's pane or process.                                           |
| `Plan <plan-id> approved`         | To the PM: the plan's packages, dependencies and order.                                      |
| `Plan <plan-id> needs attention`  | To the PM: the plan did not pass review, or the Architect was lost.                          |
| `Plan <plan-id> signed off`       | To the PM: the integration branch and commit the Architect signed off.                       |
| `Operator run <id> finished`      | A proposed command ran; the output tail is untrusted data.                                   |
| `Routine check`                   | To the Supervisor: do one watch pass.                                                        |

## Exit codes

From `EXIT` in `src/cli.ts`: `0` ok, `2` usage error, `3` invalid input or configuration, `4` blocked (the controller refused the request, for example `worker_limit`, or `cstan start` failed to launch), `5` runtime error.
