# Knowledge base

A project knowledge base is a short list of facts about your code that agents read before they write: which helper already does a job, where a component lives, which convention a folder follows, why a decision was made. It exists so that work is **reused before it is written**. An agent that finds the existing helper does not write a second one, and a design rule written down once is not rediscovered on every task.

It works for any project and any language. Capstan reads no source code itself: the entries are plain text that agents write, the people on the project review, and git versions.

The keys are in [Configuration](configuration.md#knowledge-base-kb), the commands in [Commands](commands.md#knowledge-base). Without a `[kb]` table in `capstan.toml` nothing in this page applies and nothing changes.

## The entries file

The entries live in one file in the repository, by default `docs/capstan/kb.jsonl` (`[kb] path`). It is JSON Lines: one entry per line, sorted by `id`, so a change to one entry is a one-line diff.

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

Do not edit the file by hand when a command does the job. `cstan kb add`, `update` and `remove` check every rule, keep the file in id order and stamp the entry with the current HEAD. They change the file in your working tree only: you commit it on your branch. `add` is refused for an id that exists, and for an entry that looks like an existing one (the same kind and path, or a title whose every word matches) unless you pass `--force`; look at the similar entry first, and update it if it is the same fact.

## Entries are reviewed like code

The index is built from the file **as committed at the project root's HEAD**. A working tree is never read, so an edit in a worktree changes nothing for other agents until its branch is merged. That makes the knowledge base subject to the same path as code: an agent adds or changes entries on its branch, the branch is reviewed, and the merge makes them visible to everyone. A wrong entry is a wrong line in a diff that a reviewer can see and reject. Nothing writes to the file behind your back.

## The index

The controller keeps a SQLite index of the file in its state directory (`<state dir>/kb`). The index is a **cache**: the committed file is the only source of truth, and deleting the index costs one rebuild and loses nothing but the search log below. It holds:

- a full-text table (SQLite FTS5) over title, body, tags and aliases, ranked with the title weighted highest, then aliases, then tags, then body;
- the entries' paths and `related` links, for path lookup and `uses`;
- each entry's state, `ok`, `stale` or `missing` (see [Staleness](#staleness-and-refresh));
- a log of searches (when, the words, how many hits) kept for 30 days and at most 10000 rows. It feeds `cstan kb status` and the [trigger for vectors](#not-now).

The index refreshes when it is asked for, if HEAD or the entries file's committed content changed since the last build; nothing runs in the background. A project with no commit has no knowledge base yet and the commands say so. If git fails (more than 10 seconds, or no `git`), the command is refused with the reason and the warnings stay silent.

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

`cstan kb refresh` prints a brief that lists up to 50 stale and missing entries, and the command to spawn the agent that fixes them. The agent re-reads the code at each path and runs `cstan kb update <id>` with the changed fields, or `cstan kb update <id> --verified` when the entry is still true (which only re-stamps `verified_commit`), or `cstan kb remove <id>`. The result is committed on the agent's branch and reviewed like any other change.

## Warnings

The knowledge base never refuses work. At plan open and at spawn, the controller may add one or more warnings to the answer and logs each as a `kb_warning` event; the command still succeeds. They need `[kb]` and `enabled = true`, and a failure to check (git or the index) gives no warning and a `kb_check_failed` line in the controller's log.

| Warning | When |
| --- | --- |
| `kb: no knowledge base at <path>; next: cstan kb build and send its brief to a <role> agent` | The entries file is not at HEAD. |
| `kb: <n> invalid lines in <path> (first at line <n>); next: cstan kb status` | The file has lines that are not valid entries. |
| `kb: <s> stale and <m> missing entries in <path>; next: cstan kb refresh` | Stale plus missing entries reach `stale_warn` (default 1). |
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

- `cstan kb build` prints a brief for the **kb role** (`[kb] role`, default `docs-keeper`, a Developer role you configure) and the spawn command to run it. The brief tells the agent to read the code at HEAD, add entries with `cstan kb add` for reusable helpers, modules, components, conventions, design rules and decisions, keep to one fact per entry and no code, stay within the entry and size caps, and commit the file on its branch.
- `cstan kb build --design` and `cstan kb refresh --design` print the same kind of brief for the **design role** (`[kb] design_role`, default `designer`). Its job is the design record (`[design] design_record` and `docs/design/rules.md`), written from the existing screens and code, or updated for the interface files that changed since the record's last commit.

Capstan prints the brief and the command; the project manager spawns the agent and sends the brief. The result arrives as a normal branch, reviewed and merged like code.

## Not now

Two things are left out on purpose.

- **Vectors.** There is no embedding search and no vector store. Word search with aliases is small, exact and explainable, and it needs no model. The trigger for adding vector search is what `cstan kb status` reports on its `vector search:` line: `consider` when at least 50 searches were logged in the last 30 days and a fifth or more of them returned nothing, or when the file holds 5000 or more entries; otherwise `not needed`. If it says `consider`, the plan is to add `sqlite-vec` with a small local embedding model behind a flag, after measuring its memory and start-up time on the host.
- **A graph store.** `related` links are a table, and `uses` follows them up to three links with an ordinary query. A graph store is worth adding only if questions that need many hops become routine and that query is too slow.
