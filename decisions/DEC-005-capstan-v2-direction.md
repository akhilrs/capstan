# DEC-005: Capstan v2 direction: no containers, interactive agents in Herdr, controller-mediated messaging

**Task:** PM-20  
**Status:** proposed 2026-09-30; becomes accepted when the PR that adds it is merged by the operator.  
**Decision:** Drop Docker, per-role networking, the egress firewall and the OMP-extension bridge from the Capstan MVP. Run interactive, persistent agents in Herdr panes and git worktrees on one trusted single-user VM. The controller is a separate daemon that owns all agent-to-agent messaging and records every workflow fact from `cstan` receipts.

## Context

`MVP_PLAN.md` (M0 to M3) built a fixed four-seat workflow: one OMP session per role in a Docker container with a per-role network and an iptables egress allowlist, driven from a brief file through an OMP-extension bridge with an fsynced receipt journal. The operator chose a different shape on 2026-09-30:

- The operator runs `cstan`, which opens the configured agent host in Herdr. The first agent is the PM. The operator talks to it directly.
- The PM asks Capstan to spawn other agents (developer, reviewer, later designer, tester and a Supervisor). Agents are interactive and persist until the task ends.
- Isolation is git worktrees, not containers. This runs only on the operator's own VM.
- The host (Claude Code first, Codex and OMP later) and its permissions are set per role in `capstan.toml`.

Evidence for this decision: the PM-19 spike (`docs/spike-herdr-agents.md`, six experiments with Claude Code in an isolated Herdr session, one run each) and an external plan review of the redesign. The full design is `MVP_PLAN_V2.md`.

## Decision

1. Remove Docker, per-role networks, the egress allowlist, the TLS/SNI proxy, the C kill helper, cgroup-empty proofs, the container image pin and the M1 qualification scripts from the MVP path. Containment is process-group only.
2. The controller is a long-running daemon separate from the PM, with a single-instance lock. Its SQLite ledger stays the authority for workflow state.
3. All agent-to-agent text goes through the controller (`cstan send`), one writer and one strict FIFO per recipient. Only the controller sends input to, starts, stops or closes agents in Herdr; no agent, including the PM, does so by policy. Read-only inspection (`herdr agent list`, `read`, `wait`) is allowed for the Supervisor and for diagnosis. Nothing enforces any of this (see the trust model).
4. Workflow facts (reports, acks, review requests, findings) are recorded when an agent runs a `cstan` command. Completion is never inferred from terminal text or Herdr's state. The controller verifies what it can, but an environment credential only labels a reporter (spike E5).
5. Roles come from configuration instead of the fixed list in `src/controller/types.ts`; for the first slice the configured names are aliases onto the existing capability roles (`reviewer` is `Verifier`), and the database migration is additive (`MVP_PLAN_V2.md` section 9).

## What this supersedes

| Earlier decision | What changes | What stays |
| --- | --- | --- |
| DEC-001 | The consequence that M1 must prove the OMP/Herdr bridge and Docker containment. | The decision to build a small independent TypeScript controller, and the OpenRig licence position. |
| DEC-002 | The selected-path qualification (Docker, egress policy, OMP bridge, receipt journal) becomes historical evidence, not a requirement. Its statements that the prohibitions on terminal scraping remain and that no terminal-scraping fallback follows are narrowed by the rule below. | The Decision paragraph: terminal transcript scraping and Herdr prompt delivery are not Capstan's command authority. v2 uses prompts only to deliver controller-mediated messages; authority stays with receipts. |
| DEC-003 | Two of its three revisit triggers are met by design and accepted here with the residual risks below: "independent Herdr activity is observed during a Capstan assignment" (agents and the PM can run `herdr` themselves) and "the operating model adds concurrent Herdr clients" (agents and the controller all use Herdr). The third, "a future acceptance requirement again demands technically enforced sole authority", is not met and v2 does not require enforcement. Its statement that no terminal transcript scraping is authorized is narrowed by the rule below. | The statement that Herdr does not enforce exclusive command authority, and that this is an operational assumption, not a guarantee. |
| DEC-004 | The M1 bridge adapter, the Docker-specific runtime recovery, the fixed role list, the four-seat workflow and its statement that terminal scraping is not authorized (narrowed by the rule below). | The SQLite/WAL controller core, transition and capability tables, idempotent versioned mutations, generations, receipts as a concept, candidate binding, findings, restart reconciliation, and the project lock. |

## Rule on reading panes (narrows the scraping statements in DEC-002, DEC-003 and DEC-004; the prohibition on scraping or prompt delivery as command authority stands)

Reading a pane to diagnose or to show evidence is allowed. Reading a pane never causes a state change or an automatic resend. Exactly two cases let a screen read gate an action, and each is logged:

1. Before a controller message to a worker pane: a read shows the input line is empty. If it is not, the message is deferred. After the maximum deferral the controller may clear the line with `ctrl+u`, but only after logging the text it read and notifying the operator.
2. The startup trust dialog for a worktree path the controller has just created: the controller may answer it only after reading that the dialog names exactly that path, choosing the option by its text. Pre-trusting through the host's configuration is preferred if it works. Every keypress is logged.

The controller never types into the PM pane: the operator may be typing there, and text already in an input line merges with a later prompt (spike E4).

## Trust model

The VM is a single-user machine that the operator trusts. Agents run as the operator with full network access. A worktree isolates changes; it is not a security boundary.

Residual risks accepted by this decision:

- Any process of the same user can read an agent's environment token and forge acks and reports (E5). The controller verification narrows this but does not close it: a forger with the stolen token and a valid commit on that agent's branch is accepted. Any same-user process can also commit into any worktree.
- The same holds for the operator's own authority: the control socket is reachable by every same-user process, and an agent that reads the operator credential file or ignores its host deny rules can forge operator commands.
- An agent can write the SQLite ledger directly and can run `herdr` itself. Detection by comparing send counts with Herdr's server log is partial and is not part of the first slice.
- Other worktrees and the main checkout are writable by any agent.
- Prompt injection through repository, web or tool content, including worker-to-PM message text, which is untrusted content in the PM's context.
- Exfiltration over the open network, and the operator's secrets in the environment inherited from the Herdr server and in `~/.ssh`.
- Herdr's agent state came from screen-detection rules with a remotely updated manifest in the installed configuration (E3), so it is a hint, not authority.
- Worker agents run with hooks disabled through `--settings`, which also drops any guard the operator wants; the rest of `~/.claude` (CLAUDE.md, MCP servers, permission rules) was not examined.
- Supervisor independence is by convention only: it shares the account and the VM.

## Revisit triggers

Stop and revisit this decision if: a second user or an untrusted party can reach the VM; agents work on untrusted repositories or prompts; production credentials come within an agent's reach; Capstan runs unattended; or a Claude Code, Herdr or detection-manifest update changes state detection, hooks or dialogs.

## Consequences

- The PM-5 to PM-9 controller core is reused; the Docker runtime manager (about 1,250 lines), the OMP bridge, the egress and receipt scripts and the fixed scheduler are replaced and later removed. Nothing is removed until the replacement passes its stage gate.
- PM-12 (Herdr re-pin and Docker requalification) is moot except for choosing a Herdr version. PM-13 (failure-injection harness) is rescoped to pane and process failures.
- The build is a thin vertical slice on Claude Code first (PM, one developer, one reviewer), with the Supervisor, recovery, the harness and Codex and OMP adapters after it (`MVP_PLAN_V2.md`).
- Real-host tests spend the operator's Claude account; the operator sets a cap before any long run.

## Not verified

Codex and OMP behavior; an updated Herdr Claude hook; long tasks and context growth; agent death and replacement; several agents interfering; pre-trusting worktrees; scrubbing the Herdr server's environment; pinning the detection manifest; `herdr notification`; `cstan wait` under Claude Code's shell-command behavior; selective PM hooks through `--setting-sources`. `MVP_PLAN_V2.md` section 12 lists each item with the stage that tests it and a fallback where one exists; some items have none yet.
