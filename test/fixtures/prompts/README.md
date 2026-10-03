# Permission prompt fixtures

Real captures (`herdr pane read --source visible --lines 40 --ansi`) of Claude Code
**2.1.288** worker permission prompts, taken in an isolated Herdr 0.9.1 session
(`--session`, throwaway repo, `--permission-mode default`, no allow rule). The
operator's hook-error lines show a path rewritten from the home directory to
`/home/user`; nothing else is edited. Files named `synthetic-*.ansi` are derived
from these captures and exist only to prove rejection.

| File                                         | What it shows                                                                                                                                                                                            |
| -------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `claude-bash-permission.ansi`                | Bash prompt for `touch spike-a.txt`: 1 Yes, 2 Yes and always allow access to `<dir>` from this project, 3 Yes and switch to auto mode, 4 No; footer `Esc to cancel · Tab to amend`                       |
| `claude-bash-permission-no-textfield.ansi`   | Same prompt after `down` x3 and `tab`: option 4 now reads `No, and tell Claude what to do differently` (the open text field); footer `Esc to cancel`                                                     |
| `claude-write-permission.ansi`               | Write prompt for `spike-b.txt`: 1 Yes, 2 Yes and switch to accept edits (...) for this session (shift+tab), 3 No                                                                                         |
| `claude-write-permission-yes-textfield.ansi` | Write prompt after `tab` on option 1: reads `Yes, and tell Claude what to do next`                                                                                                                       |
| `claude-bash-permission-no-typed.ansi`       | After typing `do not touch files` in the open field: option 4 reads `No, do not touch files`                                                                                                             |
| `claude-write-permission-yes-typed.ansi`     | After typing `go ahead`: option 1 reads `Yes, go ahead`                                                                                                                                                  |
| `synthetic-dialog-teach-auto-mode.ansi`      | The `Teach auto mode about your environment?` dialog (toggle rows, a `Continue` row, footer `←/→ to change · Enter to continue · Esc to cancel`, no input box); rebuilt from the incident, not a capture |

Field wording is `<option>, <typed text>`; the adapter presses Enter only when
the target option reads exactly that and nothing else changed.

## AC-H0 findings

(a) Version and captures: above.

(b) Text entry. Tab on the selected option opens an inline text field in that
option. Typing text then Enter submits it with the choice ("No, <text>" tells
Claude what to do instead). Verified: `down` x3, `tab`, literal text
`do not touch files`, `enter` -> the worker left blocked, the file was not created.
`esc` after the field opened cancelled the whole prompt (worker became done).
Fixture-proven text paths: the Bash prompt's `No` and the Write prompt's `Yes`.
Every other option has `acceptsText: false`.

(c) Herdr 0.9.1 `herdr pane send-text <pane> <text>` sends literal text with no
Enter (the text sat in the input line until `send-keys enter`); `pane send-keys`
takes `up`, `down`, `tab`, `enter`, `esc`.

Live test: Capstan starts claude with `--permission-mode default` and no allow
rule, so a plain `touch <file>` request blocks the worker on the Bash prompt.
The live run uses the operator's real HOME login in an isolated session.

The live test (`test/prompt-relay-live.test.ts`) starts the Herdr server with
`SHELL=/bin/bash` so the scratch HOME's bash prompt is the one the launcher
waits for; `test/launcher-live.test.ts` needs the same when the login shell is
not bash.
