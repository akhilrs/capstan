# Stage 2 gate on real Claude Code sessions (PM-26)

Run on 2026-10-01 with Claude Code 2.1.286, Herdr 0.9.1 and the Capstan build at commit `1c73990`. Real PM and worker sessions ran in an isolated Herdr session (`capstan-gate`) in a throwaway repository under `/tmp/capstan-gate/repo`. The agents were real Claude sessions with no canned replies. The operator's own Herdr session was only read, before and after.

The evidence is in `docs/stage2-gate-evidence.txt`: the message transition table (every state change, from the ledger's event table), the wait and state-history rows, the input-clear record, the notification record, token counts and the exact operator input. Times are UTC. The daemon was restarted once at 08:26:39Z (to raise the host wait timeout for case f5). That restart ended the dead worker and cancelled its open messages, so the final row of a message can differ from what the run showed live; the transition table is the record.

A case is marked PASS only when the stated oracle was met. Where the oracle was weak or was replaced, the result says so.

## Result per case

**(a) 10 consecutive PM to developer rounds: PASS.** The real PM spawned `developer-2`, sent ten tasks one at a time and acked each reply. The ledger holds messages 3 to 22: ten to `developer-2`, ten to `pm-1` (`ECHO-1` to `ECHO-10`, in order), all 20 `acked`, one reply per message id. A first smoke round (messages 1 and 2, `PONG-1`) also ended `acked`.

**(b) text already in the worker's input line: PASS, both variants.**
- Variant 1 (message 23, frame id `2966c849…`): with `draft text in progress` typed in the input line the message went `deferred` (`input_not_empty`). After the operator cleared the line it was sent and `acked`. The pane read shows the frame header and then exactly `Probe B1: acknowledge this message and take no other action.`, with no part of the draft.
- Variant 2 (message 24, frame id `6543b560…`): the draft stayed past the 120 s maximum deferral. The message was sent at 08:12:41.030Z and acked. The ledger table `message_input_clears` holds the cleared text and its hash, and `notifications.jsonl` holds an `input_cleared` entry from the fallback channel (the Herdr channel failed, see the notification item below). The received text equals the sent text.
- By design the controller wipes the operator's draft in variant 2. The text is kept in the ledger, so nothing is lost, but the operator loses a draft they were still typing.

**(c) message queued while the agent is working: PASS.** The `developer-3` state history shows `working` at 08:13:17.960Z and `done` at 08:13:24.019Z. Message 26 was queued at 08:13:21.232Z (during `working`), deferred at 08:13:22.008Z as `agent_busy`, and sent at 08:13:24.043Z, 24 ms after `done`.

**(d) killing the worker's pane mid-round: PASS for the oracle, and it exposes a failure the oracle does not cover.** The pane was closed 0.7 s after message 27 turned `sent` (08:14:19.250Z). The message was never resent (`send_attempts` 1). The timer moved it to `unacked` at 08:24:19.912Z (600.7 s after the send), and the operator's `resolve cancel` ended it at 08:25:13.330Z. **Failure: the controller does not notice a dead worker pane while it runs.** The agent stayed `active` with its pane gone for about 12 minutes, until the daemon restart at 08:26:39Z, when adoption ended it. This needs a liveness check (Stage 6).

**(e) `pm restart` summary: PASS, with a weak oracle.** The recorded summary lists `openWork: []` and one unacknowledged message (`568133c6…`). A direct ledger query also finds zero open work items, and the restarted PM answered "none" plus the same message id, and said it treated the body as information. **Equal-and-empty proves little:** no work item can be created yet (`assign` is a Stage 3 stub), so the list-the-open-work part is untested with real content. The old generation's message was cancelled (`generation_replaced`).

**(f) PM pull path: PASS for five sub-cases; one oracle replaced.**
- f1: operator text typed during a `wait`. The message arrived through the wait (message 29), stayed `sent`, and was acked only when the PM was asked to ack it (08:16:00.605Z). The typed draft stayed in the input line untouched.
- f2: Esc interrupted a wait (the wait row ended 8.2 s after it started). Message 30, sent afterwards, stayed `queued` and became `sent` only on the next `inbox` (08:16:45.302Z).
- f3: three queued worker reports (messages 32 to 34, `RPT-1` to `RPT-3`) were delivered one per ack, in order, none dropped (sent and acked between 08:18:12Z and 08:18:19Z).
- f4: a `wait` run in the background delivered message 35 as `sent`, not acked (the restart later cancelled it).
- f5: a foreground `wait` that outlasted two minutes was moved to the background by Claude Code (details below). Message 36 reached it, stayed `sent`, and the timer moved it to `unacked` at 08:39:37.825Z (609.7 s after the send: the 600 s PM-ack timeout plus one timer tick).
- **Replaced oracle:** "pane input history shows only the operator's keys" cannot be checked, because Herdr keeps no input history. In its place: none of the PM transcripts holds a `[capstan message` frame (0 matches in both), and all 18 PM-bound messages were delivered by a `message.pull` event (18 messages, 18 pull events). This shows nothing was typed as a frame, not that no other text was typed; the operator's own input is listed in the evidence file.

**(g) an unacked message blocks the next one: PASS.** Message 28 (`G1`) was queued at 08:14:50.940Z behind message 27, which was `sent`, then `unacked` from 08:24:19.912Z. G1 stayed `queued` with no send attempt until the operator's `resolve cancel` at 08:25:13.330Z. It was then tried at 08:25:18.545Z and ended `failed` (`Herdr failed: agent_not_found`), visible rather than lost. The daemon restart at 08:26:39.586Z then cancelled it (`agent_ended`), which is why its final row reads `cancelled`.

**(h) a new worktree path and the trust dialog: PASS, by a route the plan did not expect.** Every `spawn` answered `started` (not `blocked`) and no worker pane showed the dialog; the daemon log holds no `trust_dialog_key` entry, so the logged exception was never used. Over the run `~/.claude.json` gained one project entry, for the repository itself (trust accepted by the operator's keys in the PM pane), and none for any worktree path. Claude Code treated the git worktrees of a trusted repository as trusted. This holds once the repository is trusted, which needs the operator's answer in the PM pane first.

## Items from section 12 of the plan

- **`cstan wait` under Claude Code's shell behavior.** In 2.1.286 a foreground `cstan wait` that outlasts two minutes is moved to the background by Claude Code, not killed: the daemon wait (host `wait_timeout_seconds = 300`) stayed open, the PM was told it was "running in the background as task …", and a message sent later reached the wait and was printed. Esc ends a wait. Typing during a wait changes neither the wait nor the message.
- **`herdr notification`.** `herdr notification show` returned `shown: false` with `no_foreground_client` in this headless session. The adapter treats that as a failure, so every notification went to the fallback channel and was recorded in `notifications.jsonl`. The fallback works. Whether the Herdr channel shows anything with an attached UI client was not tested.
- **Selective PM hooks through `--setting-sources`.** Not run (optional). Hooks stay off through the role setting.
- **`cstan` allow-listing and the role prompt.** Both agent kinds ran `cstan` through `Bash(cstan *)` with no permission prompt in any case. The PM followed the delegation prompt: the smoke round shows spawn, send, wait, ack, report and release. Asked to edit `README.md` itself, the PM refused and offered to spawn a worker. **That refusal came from the prompt. The tool deny list was not exercised**, so its effect is untested.
- **Pre-trusting worktrees.** See case (h).
- **Per-loop cost.** Every Claude session starts with about 72 000 cache-creation tokens (the operator's user-level `CLAUDE.md` and rules load into the agents). The PM sessions together used 52 turns, 285 921 cache-creation, 4 624 518 cache-read and 8 483 output tokens. Across the 11 PM-driven rounds (case a plus the smoke round) the first PM session used about 23 turns, roughly 2 turns, 180 000 cache-read tokens and 400 output tokens per round.

## Findings that change the plan

1. The controller does not detect a dead worker pane while it runs (case d). It learns at the next daemon start. A liveness check belongs in Stage 6.
2. Agents load the operator's user-level `CLAUDE.md` and rules (`hooks` off does not stop that). That adds about 72 000 tokens to every session and can change agent behavior. A real isolation option for the host (a separate Claude configuration directory with its own login) is open work.
3. Real Claude trusts worktrees of a trusted repository, so the trust-dialog exception stays unused in the normal case.
4. Claude Code shows grey suggestion text in the input line (for example `Now ack message …`). It looks like typed input in a plain pane read. No case here put suggestion text in a worker's input line, so this run does not show whether the delivery driver mistakes it for typed text; that needs its own test.
5. A foreground `wait` is auto-backgrounded after two minutes, so `shell_command_timeout_seconds` and the host wait timeout in `capstan.toml` do not stop a long wait the way the plan assumed.
6. Message 28 shows that a dead recipient only becomes visible when a message to it fails or the daemon restarts.

## Guard and what changed on the operator's side

| Guard item                               | Limit | Used                                                                   |
| ---------------------------------------- | ----- | ---------------------------------------------------------------------- |
| PM sessions                              | 2     | 2 (generation 1 and the restart)                                       |
| Worker sessions active at once           | 2     | 2 at most (`developer-3`, dead, and `developer-4`)                     |
| PM-driven rounds                         | 12    | 11 (the smoke round and case a)                                        |
| Operator probe messages                  | 12    | 11 (B1, B2, C1, C2, D1, G1, R, F1, F2, F4, F5)                         |
| Extra usage, retries                     | off   | off, none                                                              |

Token totals from the transcripts, all six sessions: 178 input, 12 000 output, 577 563 cache-creation and 7 693 067 cache-read tokens (per session in the evidence file). The operator's own `/usage` reading is not in this file; the operator reads it.

State after the run: the default Herdr session shows nothing added and nothing removed compared with its snapshot before the run; the checksum of `~/.claude/settings.json` is unchanged; `~/.claude.json` changed (one new project entry, `/tmp/capstan-gate/repo`, plus Claude's own counters), as the CLARIFY entry said it would. The gate daemon and the `capstan-gate` Herdr server are stopped, the worktrees and branches are removed, and one PM Claude process that survived the server stop was ended by its pid. The throwaway repository and its ledger remain under `/tmp/capstan-gate`.
