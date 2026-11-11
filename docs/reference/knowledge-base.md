# Knowledge base

A project knowledge base is a short list of facts about your code that agents read before they write: which helper already does a job, where a component lives, which convention a folder follows, why a decision was made. It exists so that work is **reused before it is written**. An agent that finds the existing helper does not write a second one, and a design rule written down once is not rediscovered on every task.

It works for any project and any language. Capstan reads no source code itself: the entries are plain text that agents write and the people on the project can review. They are kept either in the repository, where git versions them, or in a local file that no branch carries (see [Storage](#storage)).

The keys are in [Configuration](configuration.md#knowledge-base-kb), the commands in [Commands](commands.md#knowledge-base). Without a `[kb]` table in `capstan.toml` nothing in this page applies and nothing changes.

## Storage

`[kb] storage` chooses where the entries live. The entry rules, the index, staleness, warnings and the Reuse section work the same for both.

| | `repo` (default) | `local` |
| --- | --- | --- |
| File | `[kb] path`, by default `docs/capstan/kb.jsonl`, in the repository | `<state dir>/kb/entries.jsonl` (shown as `.capstan/state/kb/entries.jsonl` when under the project root, else absolute) |
| Source of truth | The file as committed at the project root's HEAD | The local file |
| Who changes it | Any agent, on its branch; reviewed as code | Any agent, through the controller; nothing in the repository changes |
| Visible to others | When the branch is merged | At once |
| Backup | git | `cstan kb export > file` |

In `local` storage nothing is committed: an agent that adds an entry has no file to commit, and the knowledge base never appears in a diff. Writes go through the controller one at a time. Each write replaces the file atomically (write a new file, then rename it) and keeps the version before it as `entries.jsonl.prev`. The file is created with mode 0600. A change log, `changes.jsonl`, sits next to it (see [Changes in local storage](#changes-in-local-storage)).

The state directory also holds `index.sqlite`, the search index. **Deleting `index.sqlite` is safe**: it is rebuilt from the entries on the next lookup. **Deleting `entries.jsonl` loses the entries.** Take a copy with `cstan kb export` now and then, and keep it where you keep other backups.

`[kb] path` is used only with `storage = "repo"`, and to find a leftover file after a switch (see the warning below).

## The entries file

Both storages use the same format: JSON Lines, one entry per line, sorted by `id`. In repo storage a change to one entry is a one-line diff.

```json
{"id":"retry-backoff","kind":"helper","title":"Retry with backoff","body":"Retries a fallible call with exponential backoff and jitter; use it for every network call instead of a loop.","path":"src/util/retry.ts","area":"net","tags":["network","resilience"],"aliases":["backoff","retry policy"],"related":["http-client","path:src/net/"],"verified_commit":"3f2c9a7d41e86b05c1d2a9e4f7b8036a5d1c4e92"}
```

| Key | Rule |
| --- | --- |
| `id` | Required. Lowercase letters, digits, `.`, `_` and `-`; starts with a letter or digit; at most 64 bytes. |
| `kind` | Required. One of `helper`, `component`, `convention`, `design_rule`, `decision`. |
| `title` | Required. 1 to 120 bytes, one line. |
| `body` | Required. One line, not empty. One fact, no code: say what the thing does and when to use it. |
| `path` | Optional. A repo-relative file or directory (no leading `/`, no `..`, at most 200 bytes). It is what staleness is checked against. |
| `area` | Optional. A short group name: lowercase letters, digits and `-`, at most 40 bytes. |
| `tags` | Optional. Up to 8 entries of lowercase letters, digits and `-`, at most 32 bytes each. |
| `aliases` | Optional. Up to 8 other names for the thing, 1 to 64 bytes each, one line. |
| `related` | Optional. Up to 8 links, each an entry `id` or `path:<repo path>`. |
| `verified_commit` | Required. The 40-character commit at which someone last checked the entry against the code. `cstan kb add` and `update` write it. |

The keys must come in the order above, with no other key. One line may hold at most `max_entry_bytes` bytes (default 1200) and the file at most `max_entries` entries (default 2000). A line that breaks a rule is not an entry: it is listed as invalid by `cstan kb status` with its line number and reason, and the rest of the file still works. A repeated `id` keeps the first line and marks the later one invalid.

Do not edit the file by hand when a command does the job. `cstan kb add`, `update` and `remove` check every rule, keep the file in id order and stamp the entry with the current HEAD. In repo storage they change the file in your working tree only: you commit it on your branch. In local storage they change the local file through the controller and there is nothing to commit. `add` is refused for an id that exists, and for an entry that looks like an existing one (the same kind and path, or a title whose every word matches) unless you pass `--force`; look at the similar entry first, and update it if it is the same fact.

## Entries are reviewed like code in repo storage

This part is for `storage = "repo"`.

The index is built from the file **as committed at the project root's HEAD**. A working tree is never read, so an edit in a worktree changes nothing for other agents until its branch is merged. That makes the knowledge base subject to the same path as code: an agent adds or changes entries on its branch, the branch is reviewed, and the merge makes them visible to everyone. A wrong entry is a wrong line in a diff that a reviewer can see and reject. Nothing writes to the file behind your back.

## Changes in local storage

Local entries are not reviewed on a branch, so the controller keeps a log of who changed what.

- `changes.jsonl`, next to the entries file, gets one line per change (an import writes one line, not one per entry): when, the agent, the verb (`add`, `update`, `remove` or `import`), the entry id and its title. The newest `log_max_entries` lines are kept (`[kb] log_max_entries`, default 1000).
- `cstan kb log [--limit <n>] [--json]` lists them, newest first, 20 by default. In repo storage it is refused (`kb: [kb] storage is repo; ...`): there the branch history is the log.
- When an agent other than the project manager or the operator adds, updates or removes an entry (only the PM and the operator may import, and an import sends no note), the PM gets an informational note, `kb: <agent> <added|updated|removed> <id> (<title>); later changes within <m> minutes are not announced; cstan kb log lists every change`. At most one note is sent per `[kb] pm_note_minutes` window (default 30; `0` sends none), so a burst of writes is one note. The log has every change either way.
- The log and the note never block a write. If either cannot be written, the change still happens.

`cstan kb update` and `remove` read the entry with a content hash and send it back with the write; if another write changed the entry in between, the write is refused under the lock with `kb: <id> changed since it was read; run the command again.` Run the command again.

A wrong or unwanted local entry is removed with `cstan kb remove <id>`, or put back from `entries.jsonl.prev` or an export.

## Switching a project to local storage

1. Set `[kb] storage = "local"` in `capstan.toml`, and run `cstan trust` (every `[kb]` key is a checked key). Restart the controller if it is running.
2. Import the file: `cstan kb import <path>`, where `<path>` is the old entries file. If the file is already gone from your working tree, write it out first: `git show <commit>:<path> > file`, then `cstan kb import file`. Existing ids are skipped; `--replace` overwrites them. The command sends the file in parts (`import-part`) that the controller stages under a random token, then asks for the import with that token (`import <token>`), so the import is all or nothing: either every valid entry is written, with one change line, or nothing is. An empty file is not an error: it prints `kb: imported 0 entries ...` and exits 0. Staged parts older than an hour are swept. Refusals, all with nothing imported: `kb: the import is over the limit of <N> entries of <M> bytes; nothing was imported` (the parts together are larger than `max_entries` times `max_entry_bytes`, or the result would pass `max_entries`) and `kb: no staged import for that token; run cstan kb import again` (the staged parts are gone, for example after the sweep).
3. Check the result with `cstan kb status`. In local storage its first line reads `kb: ok` when the entries file exists and `kb: missing` before the first entry is written; the `head` shown is the project's HEAD, which only the stale check of `path` entries uses.
4. Remove `<path>` from the repository in one commit.

Until step 4, the controller warns at plan open and at spawn: `kb: <path> is in the repository but [kb] storage is local; next: cstan kb import <path>, then remove the file from the repository`. Only the project manager and the operator may import.

Briefs for the kb role and refresh briefs, and the sentence given to a developer, change with the storage. In local storage they say the entries are saved in the controller's local knowledge base at its location, and end with: `Commit no knowledge base file; report when done.`

## The index

The controller keeps a SQLite index of the entries in its state directory (`<state dir>/kb`). The index is a **cache**: the entries file (committed in repo storage, the local file in local storage) is the only source of truth, and deleting the index costs one rebuild and loses nothing but the search log below. It holds:

- a full-text table (SQLite FTS5) over title, body, tags and aliases, ranked with the title weighted highest, then aliases, then tags, then body;
- the entries' paths and `related` links, for path lookup and `uses`;
- each entry's state, `ok`, `stale` or `missing` (see [Staleness](#staleness-and-refresh));
- a log of searches (when, the words, how many hits) kept for 30 days and at most 10000 rows. It feeds `cstan kb status` and the [trigger for vectors](#not-now).

The index refreshes when it is asked for, if HEAD or the entries (the committed file, or the local file) changed since the last build; nothing runs in the background. A project with no commit has no knowledge base yet and the commands say so. If git fails (more than 10 seconds, or no `git`), the command is refused with the reason and the warnings stay silent.

### Aliases

A search matches words, and code names are not words. When an entry is indexed, its `id`, the words of its title, the file name of its `path` without the extension and its `aliases` are split into words: `snake_case` and `kebab-case` at the separators, `CamelCase` and `camelCase` at the capital letters (`RetryBackoff` gives `retry` and `backoff`). The words you type are split the same way, and each matches as a prefix. So `retry_backoff`, `RetryBackoff` and `backoff` all find the same entry, whichever form the entry or the search used. Add an `aliases` item for a name that does not appear anywhere else, such as an older name or a term your team uses.

### Path lookup and links

`cstan kb for <path>` lists the entries whose `path` is that path, a directory above it or a file under it, then the entries that name the path in `related` (`path:<repo path>`), marked as uses. `cstan kb uses <id-or-path>` follows `related` links backwards: the entries that point at an entry or a path, then the ones that point at those, up to three links deep, nearest first.

## Staleness and refresh

An entry with a `path` is checked against git when the index is built:

| State | Meaning | Reason shown |
| --- | --- | --- |
| `ok` | The path exists at HEAD and nothing under it changed since `verified_commit`. | none |
| `stale` | Something under the path changed since `verified_commit`. | `changed since <12-character commit>`; `verified commit unknown` when the commit is not in the repository; `not checked` when git failed or more than 300 different commits had to be compared |
| `missing` | The path is not at HEAD. | `not at HEAD` |

An entry without a `path` is always `ok`. Stale and missing entries are still returned by every lookup, marked as such: an old fact is better than none, and the mark tells the reader to check.

`cstan kb refresh` prints a brief that lists up to 50 stale and missing entries, and the command to spawn the agent that fixes them. The agent re-reads the code at each path and runs `cstan kb update <id>` with the changed fields, or `cstan kb update <id> --verified` when the entry is still true (which only re-stamps `verified_commit`), or `cstan kb remove <id>`. In repo storage the result is committed on the agent's branch and reviewed like any other change; in local storage there is nothing to commit.

## Warnings

The knowledge base never refuses work. At plan open and at spawn, the controller may add one or more warnings to the answer and logs each as a `kb_warning` event; the command still succeeds. They need `[kb]` and `enabled = true`, and a failure to check (git or the index) gives no warning and a `kb_check_failed` line in the controller's log.

| Warning | When |
| --- | --- |
| `kb: no knowledge base at <path>; next: cstan kb build and send its brief to a <role> agent` | The entries file is not at HEAD (repo storage), or the local file does not exist or is empty (local storage). |
| `kb: <n> invalid lines in <path> (first at line <n>); next: cstan kb status` | The file has lines that are not valid entries. |
| `kb: <s> stale and <m> missing entries in <path>; next: cstan kb refresh` | Stale plus missing entries reach `stale_warn` (default 1). |
| `kb: <path> is in the repository but [kb] storage is local; next: cstan kb import <path>, then remove the file from the repository` | Local storage, and the file at `[kb] path` is still at HEAD. |
| `design record: <files> missing; next: cstan kb build --design and send its brief to a <design role> agent` | The project has interface files but a design record file does not exist. |
| `design record: <file> is <n> UI commits behind; next: cstan kb refresh --design` | The design record is at least `design_stale_commits` (default 20) commits older than the interface files. |

### Detecting the interface

Capstan cannot tell which files are your interface in a language-neutral way, so you say so: `[kb] ui_paths` is a list of up to 32 path globs, written like the globs of `[verify]` (`**` crosses `/`; `*` and `?` do not), for example `["src/ui/**", "app/**/*.tsx"]`. With the list empty there are no design warnings. With it set, and at least one file at HEAD matching it:

- the design record files are the `[design] design_record` files and `docs/design/rules.md`; if any of them is missing from HEAD, the first design warning above is shown;
- otherwise the controller counts the commits that touched a matching file since the record last changed, and shows the second design warning when the count reaches `design_stale_commits`;
- at the spawn of a task that names a package, the check applies only when the package owns a path that is, or contains, an interface file (a design package always counts). At plan open, and at a spawn without a task, it applies to the whole project.

## The Reuse section

When a package is assigned, the controller looks up the entries that cover the paths the package owns (the first word of each `owns` item, cut before its first glob character) and appends a section to the assignment:

```
Reuse (knowledge base at 3f2c9a7d41e8; check these, cstan kb search and a symbol search before writing a new helper or component; report Reused and New):
- retry-backoff (helper) Retry with backoff: src/util/retry.ts
- http-client (component) HTTP client: src/net/ [stale]
- (more: cstan kb for src/net)
```

Each line is `<id> (<kind>) <title>: <path, else area>` and carries `[stale]` or `[missing]` where it applies. The section is capped at `reuse_max_bytes` (default 1500): lines beyond the cap are replaced by one `(more: cstan kb for <path>)` line. `reuse_max_bytes = 0` attaches nothing. The section is dropped, and never the assignment, when it would push the message over the message limit, when the package owns no path with entries, or when the knowledge base cannot be read. The developer is asked to look at the listed entries, search before writing a new helper or component, and say in the report which things they **Reused** and which are **New**, so a reviewer can see whether a duplicate was written.

## Building it

Nobody is expected to write the first entries by hand.

- `cstan kb build` prints a brief for the **kb role** (`[kb] role`, default `docs-keeper`, a Developer role you configure) and the spawn command to run it. The brief tells the agent to read the code at HEAD, add entries with `cstan kb add` for reusable helpers, modules, components, conventions, design rules and decisions, keep to one fact per entry and no code, and stay within the entry and size caps. In repo storage it commits the file on its branch; in local storage it commits nothing.
- `cstan kb build --design` and `cstan kb refresh --design` print the same kind of brief for the **design role** (`[kb] design_role`, default `designer`). Its job is the design record (`[design] design_record` and `docs/design/rules.md`), written from the existing screens and code, or updated for the interface files that changed since the record's last commit.

Capstan prints the brief and the command; the project manager spawns the agent and sends the brief. In repo storage the result arrives as a normal branch, reviewed and merged like code; in local storage the entries are already saved when the agent reports.

## Not now

Two things are left out on purpose.

- **Vectors.** There is no embedding search and no vector store. Word search with aliases is small, exact and explainable, and it needs no model. The trigger for adding vector search is what `cstan kb status` reports on its `vector search:` line: `consider` when at least 50 searches were logged in the last 30 days and a fifth or more of them returned nothing, or when the file holds 5000 or more entries; otherwise `not needed`. If it says `consider`, the plan is to add `sqlite-vec` with a small local embedding model behind a flag, after measuring its memory and start-up time on the host.
- **A graph store.** `related` links are a table, and `uses` follows them up to three links with an ordinary query. A graph store is worth adding only if questions that need many hops become routine and that query is too slow.
