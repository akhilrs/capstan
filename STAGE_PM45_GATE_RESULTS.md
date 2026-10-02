# Automatic oversight gate (PM-45)

Run on 2026-10-02 in an isolated Herdr session (`capstan-s13`) with the fake Claude stand-in for every agent (PM, two developers and the Supervisor), so no model usage was spent. The timers were shortened (`max_deferral_seconds = 20`, `worker_ack_timeout_seconds = 25`, `pm_wake_after_seconds = 10`, `pm_wake_interval_seconds = 30`, `supervision.check_seconds = 60`). The default Herdr session has the same workspaces before and after. Evidence is in `docs/stage-pm45-gate-evidence.txt`.

## The real failure this change answers

In the operator's `nexora` ledger a message to `developer-7` was enqueued at 05:05:50.953, deferred 0.6 s later (the worker was busy), and went `expired` at 05:07:51.860, exactly `max_deferral_seconds` (120 s) later, never typed. An `expired` message can only be cancelled and the recipient's queue stops behind it, so the next message stayed `queued` for hours; nothing told the PM, which concluded that messages are lost; replies to an idle PM wait for a pull; no Supervisor ran.

## What ran

1. **Automatic Supervisor.** `cstan spawn developer` started the first worker; within one tick (15 s) the daemon started `supervisor-1` from its role (`supervisor_started`) and queued a routine check for it (`supervision_check_queued`); the fake Supervisor received the text. The Supervisor did not count against `max_workers = 3` (two developers and a Supervisor were running). A check nobody acknowledged was cancelled (`superseded_check`) and replaced each 60 s: three cancelled checks and a fresh one in the ledger, and no `Delivery problem` message for any of them.
2. **A busy worker keeps its messages.** `developer-2` was reported `working` to Herdr and sent a message. It stayed `deferred` for more than 40 s, twice the 20 s deferral limit that expired the real message, and was typed once (`sent`, `deferral_count` 1) when the worker became idle.
3. **The PM is told.** Two messages that the fake workers never acknowledged became `unacked` after 25 s, and the PM received one `Delivery problem` message for each (the message id, the worker, the state, and the `cstan resolve` command).
4. **The idle PM is woken.** With those messages unread, the controller typed `Run cstan inbox: a teammate has written to you.` into the idle PM's pane 10 s after the first notice and again 30 s later (two rows in `pm_wakes`, two lines in the fake PM's log, `pm_woken` twice in the daemon log), and not in between.

## What the unit tests cover that this run did not

The wake never types while the PM works, is blocked or unobserved, or has text on its input line (adapter and state-machine tests); at most 5 wakes per message; the `Agent stalled` and `Agent blocked` notices once per episode and never for the Supervisor; a failed message's notice atomic with the failure; the Supervisor released after 10 minutes without a worker; a failed spawn retried only after a minute; `supervision.enabled = false`, `pm_wake_after_seconds = 0` and an unconfigured Supervisor role turning each part off; and the deferral clock restarting when the reason changes (a message that waited for a busy worker is not cleared at once when text appears on the input line).

## Changes to know about

- **A new limit for busy messages.** `max_busy_deferral_seconds` (default 3600, 60 to 86400): a message that waits for a busy or blocked worker expires only after it, and the PM is told. Without it a permanently stuck worker would hold a queue forever without notice. `max_deferral_seconds` (120) now only governs clearing a line with text on it.
- **Supervision is on by default** when the configuration has a `Supervisor` role (the starter file has one). It runs a Claude session, so it uses usage: `[supervision] enabled = false` removes it, `check_seconds` (default 300) sets the interval.
- Migration 0021 adds `pm_notices`, `pm_wakes` and `supervision_checks`; restart the daemon after merging.

## Limits

- Fake agents only: whether a real PM acts on the wake line, and whether a real Supervisor finds anything on a routine check, are not shown here (the earlier real runs showed a Supervisor that declines instructed waiting, and the finding mechanism end to end when seeded).
- A wake is a prompt in the PM's pane, so it starts a PM turn; the check narrows but does not close the race with an operator who starts typing between the second state check and the keys.
- The `Delivery problem` text starts with the first 80 characters of the message, which is the sender's text, not verified.
