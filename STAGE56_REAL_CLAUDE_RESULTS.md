# Real-Claude Supervisor and replacement gate (PM-39)

Run on 2026-10-02 in a throwaway repository with an isolated Herdr session (`capstan-s8`). The developer and the Supervisor were **real Claude Code agents on the `haiku` model**; the PM was the fake stand-in (chosen by working directory through a wrapper, so no PM turns were spent). Evidence is in `docs/stage56-real-claude-evidence.txt`; tokens are not in it. The default Herdr session has the same workspaces before and after (only focus and status flags differ, from the operator's own activity). The Claude project-trust entry the run added to `~/.claude.json` was removed.

## Usage

About 14 real Claude turns in total: developer-1 about 5 (three instructions, one finding, one extra), supervisor-1 about 5 (four instructions), developer-2 1. The operator answered 6 permission prompts by hand (the developer's `ls` outside its worktree and its `git apply` outside it, which the role's allow list does not cover). No other model was called.

## Stage 5 (Supervisor and findings): result

- **A real developer did not loop on its own.** Told to run a failing command up to five times, it ran all five at once and reported. Given a task that could not succeed, it tried once, saw the cause and reported to the PM. Real Claude does not repeat the same failing call the way the plan's trigger assumes, so the gate's trigger could not be produced naturally.
- **A real Supervisor judged the instructed loop correctly:** shown five failures it had been told to expect, it raised no finding ("not a stuck loop").
- **The finding path ran end to end with an operator-seeded condition.** The operator told the Supervisor the developer was blocked on a missing patch. The Supervisor ran `cstan observe developer-1`, then raised one finding (severity `low`, although `warning` was asked for; evidence text that quotes the missing file but says "after 10 attempts", which overstates what the developer ran). The controller delivered it to developer-1 and the PM, developer-1 acknowledged it and did the correction (wrote and committed `hello.txt`), and the Supervisor recorded the resolution check `resolved` with the commit id as evidence.

## Stage 6 (replacement): result

- The developer's pane was closed. The driver recorded `agent.lost` (reason `pane_gone`) and one controller message reached the PM.
- `cstan replace developer-1` started `developer-2` of the same role on `capstan/developer-2-g1`. The base was the repository head, not the predecessor's commit, because the predecessor had made no accepted report (the design: a replacement starts from the last accepted report).
- The replacement answered a question about its predecessor by quoting the predecessor's recorded instruction text, so the seed block reached a real Claude and was used.
- **Nothing was typed twice:** every message was recorded `sent` exactly once, including the instruction to the replacement. Predecessor messages were not resent.
- The replacement printed its `cstan ack` and `cstan send` commands as text instead of running them, so the instruction stayed `sent` and no PM reply arrived. This is the model's behaviour (haiku), not a controller fault; with the 600 s timer the message would end `unacked`, visible to the operator.

## Limits

- Real runs on `haiku`; behaviour on a larger model may differ (it may also loop, or write evidence more carefully).
- The finding was seeded by the operator, so what is shown is the finding, delivery, acknowledgement and check mechanism with real agents, not that a real Supervisor detects a real stuck agent unprompted. That detection has still not been seen.
- The predecessor's token was not retried after the replacement (covered by the fake-agent gate in `STAGE6_GATE_RESULTS.md`).
- The Supervisor's evidence text is model-written and was not checked against the observed screen by the controller.
