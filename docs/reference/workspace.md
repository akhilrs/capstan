# Workspace mode

A project whose code lives in several git repositories, changed together by one team. Back to the [README](../../README.md). Keys are in the [Configuration reference](configuration.md); commands are in the [Command reference](commands.md).

## When to use it

Use workspace mode when one change spans more than one repository (an API and its web client, say) and you want one plan, one branch name and one review for it. A project with a single repository does not need it and is unchanged: leave `[repos]` out.

In workspace mode the folder that holds `capstan.toml` and `.capstan/` is only a **config folder**. It holds the Capstan state; the code is in the repositories listed in `[repos]`. The config folder need not be a git repository, and when it is one it is not treated as one of the repos (a repo whose `path` is the config folder is refused: `the workspace root cannot be a listed repo`).

## Set it up

In the folder that will hold the state, either a folder that is not a repository or the root of a repository:

```sh
cstan init --workspace     # .capstan/ and a starter capstan.toml with a commented [repos] and [workspace] example
# edit capstan.toml: uncomment and fill in [repos.<name>] and [workspace]
cstan config check
cstan trust                # the [repos] and [workspace] keys are checked keys
cstan start
```

`cstan init --workspace` runs no git. It writes the capstan block of `.gitignore` and the local exclude only when the folder is a repository, and it cannot be combined with `--git` or `--gitignore`.

### `[repos.<name>]`

One table per repository. A repo needs at least `path` and `main_branch`.

| Key | Default | Meaning |
| --- | --- | --- |
| `path` | required | The repository's top-level folder: relative to the config folder, outside it with `..`, or absolute. It needs at least one commit. |
| `main_branch` | required | A plain branch name that exists in the repo. Worktrees start at its tip and integrations merge into it. |
| `verify` | the top-level `[verify]` | `{ commands = [...], env = {...}, timeout_seconds = 60 to 14400 }`: this repo's gate. |
| `cache` | all `[cache]` entries | `[cache]` entry names this repo's worktrees may share. |
| `setup` | the `[worktree]` `setup` | A shell command run in this repo's worktree. |

Lessons from real setups:

- **Repo names** must match `^[a-z0-9][a-z0-9_-]{0,31}$`: lowercase letters, digits, `-` and `_`, starting with a letter or digit, at most 32 characters. `Api` or a name with a dot is refused (`repo name Api must match ^[a-z0-9][a-z0-9_-]{0,31}$`). At most 32 repos may be listed.
- **A per-repo `setup` replaces the root `[worktree]` `setup`** for that repo; it does not run in addition to it. A repo without `setup` gets the root one. `setup_timeout_seconds` and `teardown` stay under `[worktree]`. The command runs in the repo's worktree with `CAPSTAN_REPO=<name>` set.
- **`[env] pass` belongs in `capstan.local.toml`.** Variables a setup or a test needs (a registry token, an API key) are named there, never in a tracked `capstan.toml`; they reach every agent pane and every `setup` command of every repo.

### `[workspace]`

Optional, and only with at least one repo.

| Key | Default | Meaning |
| --- | --- | --- |
| `merge_order` | all repos by name | Repos to integrate in this order. Repos left out follow, by name. |
| `worktree_dir` | `.capstan/worktrees` in the config folder | Where change folders go. It must lie inside no listed repo and inside no other Capstan project. |
| `default_repo` | the first repo by name | The repo used when none is named. |

### Refusals when the config is read

`cstan config check`, `cstan start`, `cstan init --workspace` and the daemon at boot refuse, with these texts:

- `[workspace] needs at least one [repos.<name>]`
- `at most 32 repos may be listed`
- `repo name <name> must match ^[a-z0-9][a-z0-9_-]{0,31}$`
- `[repos.<name>] needs path` and `[repos.<name>] needs main_branch`
- `repos.<name>.main_branch must be a plain branch name`
- `the workspace root cannot be a listed repo`
- `repo <name> names unknown cache entry <entry>`
- `merge_order names unknown repo <name>` and `merge_order names <name> twice`
- `default_repo names unknown repo <name>`
- `repo <name>: <path> is not the top level of a git work tree with a commit`
- `repo <name> has no branch <main_branch>`
- `repos <a> and <b> are the same repository`
- `repo <a> lies inside repo <b>`
- `worktree_dir lies inside repo <name>` and `worktree_dir lies inside another project <path>`

## The change folder

A worker of a workspace project gets one **change folder**, `<worktree_dir>/<agent-id>`, with one git worktree per repo of its task, all on **one branch** named after the task. Each worktree is cut from the tip of that repo's main branch and runs that repo's `setup`. The agent opens in the change folder, which is not a git work tree: it runs git inside each repo (`git -C <repo> ...`). Its prompt lists the repo paths.

A worker may need another repo later: `cstan repo add <repo>` adds a worktree for it on the same branch. It is refused with `not_workspace`, `unknown_repo`, `not_in_package`, `already_added`, `branch_taken`, `repo_unusable`, `setup_failed` or `not_a_worker`; see the [Command reference](commands.md).

## Reports

A report names the commit of each repo the worker changed:

```sh
cstan report api=<40-char-commit> web=<40-char-commit> "<summary>"
cstan report --heads "<summary>"     # the current HEAD of every repo that has new commits
```

A repo left out keeps the commit of the worker's last report. `--heads` leaves out a repo with no new commits and is refused with `no repo has new commits` when none has. Refusals: `repo <r> is not a repo of this project`, `repo <r> is not in your change set`, and `repo <r> has commits after <report>; name it` when a repo moved on since the last report and is not named.

## Plan packages

In a workspace project every package lists the `repos` it changes and prefixes each `owns` entry with `<repo>:`, for example `"repos": ["api"], "owns": ["api:src/server.rs"]`. A submit is refused with `package <id> needs repos in a workspace project` and `package <id> names unknown repo <r>`; a project with no `[repos]` refuses `repos` (`repos are only for workspace projects`). `repos` cannot be amended: `cstan plan amend` never names them (`repos cannot be amended`), and a change of repos is a new submit.

## Verify and review

The gate (the `[verify]` commands) runs per repo: a report gets one run per repo it names a commit for, with that repo's `verify` or else the top-level one. `cstan verify <id> --repo <repo>` queues one repo's run again; `--repo` outside a workspace is refused. The gate passes only when every repo's latest run passed, and the report hears one message once all runs are final.

A review covers an integration **set** (below) as a whole. `cstan request-review` of a set is refused while the set is not merged (`set <id> is <state>; only a merged set can be reviewed`).

## Integration sets

`cstan integrate <report-id>...` in a workspace project makes a **set**: one integration per repo the reports touch, merged one at a time in merge order (`merge_order`, then by name). A set is `running`, then `merged` (or stopped at a conflict or failure), and its members are settled one repo at a time.

- A stopped set names the repo and the next step: `resolve with a new report for repo <r>, then cstan integrate redo <set> --repo <r>`, or `cstan integrate redo <set> --repo <r>, or cstan integrate discard <set> --repo <r>`.
- `cstan integrate redo <set-id> --repo <repo>` integrates one repo again. It is refused for `set <id> is running; wait for it to settle`, `set <id> is discarded`, `set <id> has no member for repo <r>`, `repo <r> of set <id> is confirmed; its code is on main`, `repo <r> of set <id> is still merging`, `no report for repo <r> since the conflict; report the fix first`, `repo <r> has nothing to integrate` and `branch <b> already exists in repo <r>`; when the repo's main already holds the change it says to confirm instead.
- `cstan integrate discard <set-id>` throws the whole set away. `cstan integrate discard <set-id> --repo <repo>` drops one unconfirmed member: the set continues without it (`repo <r> dropped from set <id>; the set continues without it; integrate redo <id> --repo <r> brings it back`). A dropped member does not block the others.
- `cstan integrate confirm <set-id> --repo <repo>` keeps one repo once its main holds the change. Every live member must be confirmed before the set is.
- `decide-merge` merges a member into its own repo's main branch (`--into` does not apply to a member). Members merge in order: a member whose earlier repo is not settled is refused with `merge <repo> first`.

## Cleanup

`cstan cleanup` makes one offer per repo that has candidates, in merge order, and a repo that left the config offers its worktrees only. `cstan cleanup apply <cleanup-id>` and `decline` work per offer, as in a single-repo project.

## Limits

- The agent's working directory is the change folder, which is not a git work tree: git commands run inside a repo, not in the folder.
- Each repo's own `.claude` settings files are not loaded for an agent that works from the change folder, so a repo's project-level permissions and hooks do not apply; put what the team needs in `capstan.toml` roles. A repo's `CLAUDE.md` is still read when the agent opens it, and the agent's prompt says to.
- The config folder is only a config folder: the code, the branches and the merges are in the listed repos.
- Git 2.38 or newer is needed to integrate.
