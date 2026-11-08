# Retries and repeated sends

A `cstan` command can lose its reply: the connection drops after the controller did the work but before the answer arrived. The controller keeps a small reply cache so that asking again does not do the work twice. This page says what it keeps, for how long, and what it cannot recover.

## Request ids

A request may carry a request id (`rid`): 1 to 64 characters of `A-Z`, `a-z`, `0-9` and `-`. Anything else is ignored, as if the request had none. The controller checks the credential first, then the reply cache, then runs the command.

Asked again with the same `rid` (by the same caller, with the same command and arguments), the controller answers:

| First attempt | Answer to the retry |
| --- | --- |
| finished, answer kept | the same answer, word for word |
| still running | waits for it, then gives the same answer; after at most 60 s (30 s for a command with no time limit) it answers `attempt_running`: "the first attempt is still running; check before running it again" |
| the controller failed inside the command | `attempt_failed`: "the first attempt failed inside the daemon; check before running it again". The command is never run a second time |
| finished, answer not kept | `answer_not_kept`: "the first attempt completed; its answer is not kept: check with cstan status" |
| used for a different command or arguments | `invalid_request`: "request id reused for a different request"; nothing runs |

`attempt_running`, `attempt_failed` and `answer_not_kept` make `cstan` exit 1 and are never retried by the client.

Entries are kept for 600 seconds. A running attempt is never evicted to make room: when every entry that could go is still running, the new request runs without an entry (it is logged as `reply_cache_uncached`), and its retry runs again too. Each caller (a credential; the PM's processes share one) keeps at most 256 entries, so one caller never evicts another's. Overall there are at most 4096 entries and 16 MB of answer text; the oldest go first.

## What is not stored

- **Busy and shutting-down answers** (`controller_busy`, `busy`, `shutting_down`) and any answer where the controller's own kernel failed: the retry runs the command. A command whose kernel job failed rolled its changes back, so nothing was committed.
- **`wait`** is never stored or joined; the client never retries it.
- **Answers over 64 KB** are not stored: the retry gets `answer_not_kept`.
- **Commands whose answer may carry a credential or a nonce** (`spawn`, `launch`, `replace`, `pm-restart`, `op`, `decide`, `decide-merge`) are never stored as text. If the reply to one of these is lost, it cannot be recovered; the retry gets `answer_not_kept`, and you check the result with `cstan status`.
- `inbox` is stored as text, so the private message text it returns stays in the controller's memory for 600 seconds, in the process that already holds the ledger.

The cache is in memory only. A restart forgets it.

## Repeated sends

A `send` that is identical to an earlier one from the same caller (same recipient, text and flags, such as `--action` or `--info`) within `repeat_window_seconds` (`[daemon]`, default 60) is not sent again. The answer is the first one, with these extra members:

- `repeat: true`
- `repeatAgeMs`: how long ago the first went out
- `note`: "not sent again: the same message went to `<recipient>` `<n>` s ago (message `<id>`)"

`cstan` prints the note and exits 0, so a script does not retry it. Only a first send that ended ok is folded. Two identical sends that run at the same time both go out. No other command is folded.

This also folds an identical resend you meant. To send the same text again inside the window, change the text, or set `repeat_window_seconds = 0` to turn folding off. A controller run without a `capstan.toml` (a transcript replay) has the window off.

The log records `reply_cache_hit` and `send_repeat_folded` entries with the command and the age.

## What `cstan` retries

Only the commands `cstan` sends to the controller retry; every other client of the socket (the dashboard feed, `cstan recent`, the shutdown call, and so on) makes one attempt as before. `cstan` makes one request id per command and uses it for every attempt of that command. A request too large to carry the id is sent without it, and a lost reply to it is not asked again.

**Controller busy.** On a `controller_busy` answer `cstan` waits the `retryAfterMs` the controller gave (500 ms if it gave none) plus up to 20% more, then asks again. All the waits of one command together take at most 30 s, and never run past the command's own time limit plus 30 s. `CAPSTAN_BUSY_BUDGET_MS` sets the 30 s. Any command waits this way, because a busy answer means nothing ran. When the budget is spent, the busy answer is shown as it always was. The older `busy` code (`notify`, `decide-merge`) is never retried. `cstan inbox --hook` waits at most 2 s and never asks again after a lost reply.

**Lost reply.** When the connection closes with no answer byte, or the call times out, `cstan` asks again with the same request id, at most twice (after 500 ms and after 1 s). It does so only when both hold:

- the controller's boot file (`<socket>.boot`) was read before the first connect and holds the same id after the failure, so the same controller is still running; an older controller has no boot file and is never asked again;
- the command is not one of `NO_LOST_REPLY_RETRY`: `spawn`, `launch`, `replace`, `pm-restart`, `op`, `decide`, `decide-merge`, `wait` and `notify`.

A reset after part of an answer was read is a read error and is not asked again. The controller answers a repeated request id from its reply cache, so a command that finished is not run twice. A command that fails inside the controller after part of its work was committed is not stored, so a repeat may run it again; check the result before running such a command by hand.

**Final errors.** With no retry made the errors are as before: `connection closed`, `timed out`. After retries:

`connection closed; retried 2 times, the request may have been applied: check with cstan inbox or cstan status before running it again`

When the boot id changed during the request, `; the controller restarted during the request` is added (also with no retry). Check with `cstan inbox` or `cstan status` before running the command again.

**Messages on stderr.** Each retry writes one line, `controller busy, retrying in <s> s (attempt <n>)` or `controller did not answer, retrying (attempt <n>)`, only when stderr is a terminal or `CAPSTAN_RETRY_LOG=1`. Hooks and captured agent output stay quiet.
