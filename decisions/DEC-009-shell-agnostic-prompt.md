# DEC-009: Recognise any operator shell's prompt before typing into a fresh pane

**Tracker:** PM-538
**Status:** proposed 2026-11-17.
**Decision:** Before capstan types into a pane it just created, it accepts either today's exact prompt symbol or a last line that ends in a prompt mark while Herdr reports the pane's own shell as the only foreground process. The line it types is `exec /bin/sh <private file>`, which every common shell runs the same way. The POSIX `exec env -i … bash` command moves into that file.

## Context

A fresh pane runs the operator's own interactive shell. `wait_for_fresh_prompt` accepted it only when the last screen line was exactly `❯`, `$`, `#` or `%`, the proof that nothing was typed. Many common prompts fail that test: nushell (`<path>> `, or `❯ :` under starship with vi mode), a default bash (`user@host:~$ `), and a two-symbol starship line. For those operators every `cstan start` failed with `PromptUnrecognized`.

The typed line also assumed POSIX quoting. nushell's `exec` builtin keeps single quotes in its arguments, so the clean bash would get `HOME='/h'` and an unusable `PATH`.

## Options

1. **Let the operator configure the prompt line.** This is exact, but every operator with a custom prompt must find and set it, and the default experience stays broken.
2. **Recognise nushell's indicators only.** This is small, but it fixes one shell and leaves the rest.
3. **Screen rule plus Herdr process info (chosen).** `herdr pane process-info` shows whether a program other than the shell holds the terminal. A last line ending in a prompt mark, with nothing after it, shows that nothing is typed. Together they work for any shell without configuration.

## Consequences

- Process info cannot see a question the shell asks with a builtin (`read -p`, nushell `input`). The screen rule therefore accepts a lone `:` only directly after a prompt-symbol token (`❯ :`). It refuses a `:` at the end of a word (`Password:`) and a lone `:` with no symbol before it (`Enter value :`). A builtin question ending in another prompt mark (`>`, `$`, `#`, `%`, `»`, `〉`, `❯`, `❮`) would still be taken for a prompt. The typed line would then become that question's answer and a temporary file path would be revealed. Nothing runs.
- The exact-symbol rule still decides first. A pane that passed before makes the same Herdr calls as before, so the frozen parity corpus changes only where the typed line itself changed.
- The typed line now names a file instead of carrying the environment. The file is private (0600, in a 0700 directory) and is deleted with the rc file as before.
