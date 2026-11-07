# Trust check

Back to the [README](../../README.md). Commands: [`cstan trust`](commands.md). Keys: [configuration](configuration.md#trust).

## Why

`capstan.toml` holds commands and permissions: what `[verify]` runs, what `[worktree]` runs in a new checkout, which hosts and MCP servers start, which roles exist. A branch you pull, or a commit a teammate lands, can change those keys, and the next controller you start would run the new values. The trust check closes that gap: a controller loads `capstan.toml` only when its checked values are the ones you accepted on this machine.

## What is checked

Every key of `capstan.toml` is checked except `schema_version` and the keys that only `capstan.local.toml` may set (`[remote]`, the `[ntfy]` connection keys and `[env] pass`). The list of keys and their classes is `rust/crates/config/src/trust_keys.golden`; a new key cannot be added without choosing its class there. A changed value, an added key and a removed key each count as a change. A file with no checked key needs no trust and is never refused.

`capstan.local.toml` (mode 0600, never committed) is trusted by definition: only you write it, and it is the only place for the machine-bound keys above.

## Where the records live

One record per project checkout, per user, outside the repository:

- The store is `~/.local/state/capstan/trust`, or the absolute directory in `CAPSTAN_TRUST_DIR`. The home directory is the one in the password database; `HOME` and the `XDG_*` variables are not read.
- The directory is mode 0700 and each record file is mode 0600. A store that is a symbolic link, is not a directory or has other permissions is refused with a text that says how to fix it.
- A record is written to a temporary file and renamed into place, so a reader never sees a half-written one.
- Nothing is written into the repository, so a pull cannot carry a record along.

## Worktrees

A linked git worktree whose common git directory is named `.git` uses the record of the main checkout, so the worktrees Capstan creates for workers need no trust of their own. A submodule, or a checkout with a separate git directory, has its own record (`record_key` in `rust/crates/config/src/trust.rs`).

## What you see

`cstan trust` prints one line per changed checked key (`+` added, `~` changed, `-` removed):

```
+ verify.commands = ["make test"]
~ worktree.setup: "make deps" -> "make"
- hosts.codex.model (was "x")
```

Values that look like secrets (a word that names a token, password, secret, key or auth, and the word after a flag that names one) are shown as `<hidden>`. Redaction is a heuristic: a hidden part still counts as a change, and a secret that does not look like one is shown. Keep secrets in `capstan.local.toml` or in environment variables, not in `capstan.toml`.

## `cstan trust`

| Form                   | Does                                                                                                   |
| ---------------------- | ------------------------------------------------------------------------------------------------------ |
| `cstan trust`          | Show the changes since the last trust and record the file as it is. It never prompts: the printed list is the review. |
| `cstan trust --check`  | Show the changes and record nothing. Exit 3 with `Not trusted. Run cstan trust to trust it.` when there are changes. |
| `cstan trust --json`   | The same as one object: `state` (`trusted`, `not_needed`, `needed`, `unavailable`), `storeDir`, `changes` (`key`, `change`, `old`, `new`) and `recorded`. Combine with `--check`. |

An up-to-date file prints `capstan.toml is trusted on this machine.` and exits 0. If the file changes between showing and recording, nothing is recorded: `capstan.toml changed while it was shown; run cstan trust again.` An unusable store exits 3 with the reason and `Set CAPSTAN_TRUST_DIR to an absolute directory.`

**Agents and apps cannot trust.** `cstan trust` refuses when `CAPSTAN_TOKEN` or `CAPSTAN_SOCKET` is in its environment (an agent's pane) with `cstan trust is for the user: run it in your own terminal, not from an agent.` This is a guard rail, not a security boundary: a process running as you can write the store files itself. Nothing in the Operator or the apps trusts a file for you.

## Where the gate applies

- `cstan start` (and the controller it spawns), `cstan restart`, `cstan config check` and the other commands that load the configuration refuse an untrusted `capstan.toml` with exit 3. The text names the keys (up to 20) and ends `Review the change and trust it with: cstan trust`, or, when there is no usable store, `Set CAPSTAN_TRUST_DIR to an absolute directory, then run: cstan trust`.
- The start command checks before it stops or spawns anything. If `capstan.toml` changes between that check and the controller's own load, the spawn fails and the error shows the daemon log, which holds the same refusal.
- `cstan init` trusts the starter it just wrote and says `Trusted capstan.toml on this machine; later changes need cstan trust.` If the store is unusable it says `Wrote capstan.toml but did not trust it: <reason>. Set CAPSTAN_TRUST_DIR to an absolute directory, then run cstan trust.` An existing file is not trusted by `init`: `capstan.toml exists: review it with cstan trust.`
- `cstan status` adds `trust: needed (<n> keys) - run cstan trust` (or `trust: unavailable - <reason>`) and a `trust` member in `--json`; a trusted project shows nothing.

## Upgrading

The first `cstan start` after you upgrade to a version with the check refuses a `capstan.toml` that has checked keys (exit 3, text naming `cstan trust`) and starts nothing. Run `cstan trust`, read the list, then start again.

A controller that is already running keeps the configuration it loaded and keeps answering. While the file is untrusted:

- `cstan status` shows the trust line.
- `cstan restart` and the Operator's restart refuse (`untrusted_config` in the Operator reference) until you have run `cstan trust`.
- Remote decisions (approve and deny from a phone or app) read the file again: they fall back to the defaults, and an integration merge is refused because `[remote] merge` is then off.

CI jobs and containers often have no home directory. Set `CAPSTAN_TRUST_DIR` to an absolute directory, then run `cstan trust` to accept the file, or `cstan trust --check` to fail a job whose file has changes nobody reviewed.

Tracking `capstan.toml` in git stays opt-in and is unchanged: see [configuration](configuration.md).
