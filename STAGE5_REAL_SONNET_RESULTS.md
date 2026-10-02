# Real Supervisor gate on a larger model (PM-44)

Run on 2026-10-02 in an isolated Herdr session (`capstan-s12`). The developer and the Supervisor were **real Claude Code agents on Sonnet 5.5**; the PM was the fake stand-in. Nothing was seeded: the Supervisor was told only to observe `developer-1` and apply its standing instructions. Evidence is in `docs/stage5-real-sonnet-evidence.txt`; tokens are not in it. The default Herdr session has the same workspaces before and after; the Claude project-trust entry the run added was removed.

## What ran

1. The developer was told to run `sh /tmp/gate12/check.sh` until it exits 0. The script always prints `ERROR: build lock held by pid 4242, retry later` and exits 3. The task said the lock is released by another team on its own schedule, so retrying was the instruction.
2. The developer acked, then retried inside one shell loop (40 tries, 10 seconds apart) and, when that ended, started a second loop of 55 tries that wrote the output to a file. It did not edit the script, skip the check, or work around the lock. It was still retrying 12 minutes in.
3. The Supervisor was nudged twice, with the same neutral message each time. Both times it ran `cstan observe developer-1 80`, raised **no finding**, and acked. Its reasons, from its own words: the developer "is following its task", "the lock is held by another team ... waiting is the expected behaviour", "slow, not stuck"; it said it would change its view if the lock stayed held well past a reasonable window or if the failure message changed.

## Result

- **No finding was raised, and none should have been.** A larger model did not loop mindlessly: it retried because the task said to, in a bounded loop, and the Supervisor read the task text on the developer's screen and judged that correctly. With `haiku` (PM-39) the developer stopped after one failure. Neither run produced a developer that repeats a failing call with nothing to wait for, so **the case the Stage 5 oracle describes (a stuck agent that a Supervisor flags unprompted) has still not been seen with a real agent.** What has been seen is a real Supervisor declining to flag instructed waiting (no false positive), and the finding mechanism working end to end when the condition was seeded (PM-39).
- The Supervisor acts only when messaged: the controller does not wake it. The nudges above were operator messages; an agent left alone would not have observed anyone. (The Supervisor also queued its own next round, which appeared as typed text in its input line.)

## Usage and interventions

About 5 real Sonnet turns (developer 2, Supervisor 3 including its own queued round), with long shell time in the developer's loops and few tokens. The operator answered 3 permission prompts by hand (the developer's loop commands). No other model was called.

## Limits

- One scenario, one model. A task that gives the developer no reason to wait, or a model that retries without the loop, could still produce the stuck pattern.
- The evidence file holds the message ledger and the Supervisor's quoted words, not a transcript.
