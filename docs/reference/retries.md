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
