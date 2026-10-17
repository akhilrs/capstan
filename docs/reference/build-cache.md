# Build cache

How agents share compiler and package-manager caches, what each choice costs, and how to measure it for your own project. The keys are listed in [Configuration](configuration.md#cache), the commands in [Commands](commands.md#tool-caches), and the measurements for this repository are in [the build-cache measurement](../investigations/build-cache-measurement.md).

## What is shared without any configuration

Every agent runs with the same `HOME` as the controller, so the tools' default caches are already shared by all worktrees of all projects:

| Tool | Shared by default | Not shared |
| --- | --- | --- |
| cargo | `~/.cargo/registry` and `~/.cargo/git` (downloaded crates; cargo locks the package cache, so concurrent fetches wait for each other) | `target/` of each worktree: every worktree compiles everything itself |
| npm | `~/.npm` (downloaded tarballs) | `node_modules/` of each worktree |
| pnpm | the content-addressed store, `~/.local/share/pnpm/store` | `node_modules/` (links into the store) |
| uv, pip | `~/.cache/uv`, `~/.cache/pip` | the virtual environment |
| go | `~/.cache/go-build`, `~/go/pkg/mod` | nothing: go's build cache is already content-addressed |

What a new worktree lacks is the *build output*, which is large (this repository's `rust/target` is 5.6 GiB after `cargo build --workspace --all-targets`) and costs a full compile. `[cache]` exists for that, and for keeping the shared downloads inside a directory Capstan can size, prune and clear.

Without a `[cache]` section nothing changes for the project.

## What `[cache]` does

For each enabled entry the controller creates a directory under the cache root and exports environment variables that point the tool at it. Every agent and every worktree hook (setup, teardown) gets the same variables, so a build in one worktree can reuse the work of another. `CAPSTAN_CACHE_DIR` holds the root. The tool decides what is safe to share: Capstan never reads or rewrites a cache's contents.

The root is `root` (an absolute path or `~/...`), else `$XDG_CACHE_HOME/capstan`, else `~/.cache/capstan`, then `projects/<project id>` for `scope = "project"` or `shared` for `scope = "user"`. Directories are created with mode 0700.

### Scope

- `scope = "project"` (default): the caches belong to this project. `cstan cache clear` works on them.
- `scope = "user"`: every project that uses the same `root` shares the caches. This saves the most disk and gives the most reuse, but one project's `max_size_gb` and `max_age_days` prune data the others use, and `cstan cache clear` refuses without `--force` because other projects depend on the store. Use it on a machine where you trust all projects to use one policy.

## Presets and modes

| Preset | Variables (each points into the entry's directory) | Needs on PATH |
| --- | --- | --- |
| `rust`, mode `sccache` (default mode) | `SCCACHE_DIR`, `RUSTC_WRAPPER=sccache`, `SCCACHE_CACHE_SIZE`, `CARGO_INCREMENTAL=0` | `sccache` |
| `rust`, mode `shared-build-dir` | `CARGO_BUILD_BUILD_DIR` | `cargo` |
| `rust`, mode `off` | none | |
| `python` | `PIP_CACHE_DIR`, `UV_CACHE_DIR`, `POETRY_CACHE_DIR` | |
| `node` | `NPM_CONFIG_CACHE`, `NPM_CONFIG_STORE_DIR` (pnpm), `YARN_CACHE_FOLDER` | |
| `go` | `GOCACHE`, `GOMODCACHE` | |

An entry whose required tool is missing is switched off with one warning (`cache_tool_missing`), and `cstan cache status` shows the reason. The variable names are checked against the installed tools in [the measurement document](../investigations/build-cache-measurement.md#variable-names-checked-against-the-tools).

### Why there is no shared `CARGO_TARGET_DIR`

Pointing every worktree at one `CARGO_TARGET_DIR` looks like the simplest cache and is the one to avoid:

- Cargo takes an exclusive lock on the target directory while it builds. Two agents building at once wait for each other (`Blocking waiting for file lock on build directory`), so the parallelism that worktrees exist to give is lost.
- The final artifacts (binaries, test executables, `libfoo.rlib`) of different branches share names. A build in one worktree overwrites the binary another worktree's test run is about to execute.

### `rust` mode `shared-build-dir`

Sets `CARGO_BUILD_BUILD_DIR` (cargo's `build.build-dir`). Only cargo's *intermediate* build output (compiled dependencies, fingerprints) goes to the shared directory; each worktree keeps its own `target/` for the final artifacts, so the clobbering problem above does not occur. On this repository a warm build takes 4-6 s instead of 55-70 s, and a worktree's own `target/` shrinks from 5.6 GiB to 0.46 GiB.

Trade-off: cargo still locks the build directory while it builds. Measured with two worktrees running `cargo check --workspace --all-targets` at the same time on one shared build directory: the second printed `Blocking waiting for file lock on build directory` until the first was done. So concurrent builds are serialised on this lock; for a project where several agents compile all the time and each build is long, `sccache` avoids the wait. Needs a cargo that supports `build.build-dir` (1.91 or newer).

### `rust` mode `sccache` (default)

Sets `RUSTC_WRAPPER=sccache`; every `rustc` call goes through sccache, whose cache is keyed by the compiler command and inputs, so concurrent builds do not lock each other and different worktrees share hits. Limits: sccache cannot cache a crate cargo builds incrementally (the default for workspace members in the dev profile: 190 of 242 non-cacheable calls in the measurement), nor crate types it does not support (38 calls, binaries and proc-macros among them). The workspace crates and link steps still run each time, so on this repository the warm build was 49-50 s against 57-60 s cold. The preset therefore sets `CARGO_INCREMENTAL=0` in the agent's environment, which lets sccache cache the workspace crates too (hit rate 78%, warm 39 s against 52 s cold) and keeps the incremental cache (about 3 GB per worktree) from filling the disk. A project that wants incremental builds back sets `[cache.rust] incremental = true`; `scripts/check.sh` uses `CARGO_INCREMENTAL=0` for its cargo stages unless the variable is already set. It needs `sccache` installed (`cargo install sccache --locked`); Capstan does not install it. The server is stopped (`sccache --stop-server`) before the cache is cleared.

### `rust` mode `off`

No variables. Use it to keep `[cache.rust]` in the file while turning the preset off.

## Size and age bounds

| Key | Default | Meaning |
| --- | --- | --- |
| `max_size_gb` | 20 | Total size of all entries. A `[cache.<name>] max_size_gb` caps one entry; an entry's cap may not exceed the total and the caps may not add up to more than it. For `sccache` the effective cap is also given to the tool as `SCCACHE_CACHE_SIZE`. |
| `max_age_days` | 30 | An entry not used for longer is removed. |

The daemon measures and prunes every 6 hours (off the controller's main thread). A prune first runs the tools' own commands (`uv cache prune`, `npm cache verify`, `pnpm store prune`), then removes entries older than `max_age_days`, then the least recently used until the total is under the cap. A prune removes whole entries only while no agent worktree exists; while agents work it runs the tool commands and reports what it would remove. Sizes come from walks that are bounded in time; a partial figure is marked `partial`.

## `cstan cache`

```sh
cstan cache status [--json]            # anyone: one row per entry, size, last use, why an entry is off
cstan cache prune [<entry>...] [--dry-run]   # PM or operator
cstan cache clear <entry>...|--all [--force] # PM or operator
```

`status` reads `capstan.toml` and `.capstan/project.json` and works without the daemon. `prune` and `clear` run in the daemon for an agent and locally otherwise; `clear` is refused while an agent worktree exists, and for `scope = "user"`, unless `--force`. An entry's `stop` commands run before it is removed. `cstan status` shows one `cache:` line read from the manifest. Details: [Commands](commands.md#tool-caches).

## Kill-safety

A cache can be rebuilt, so the goal is that a killed process never leaves a state that breaks the next build:

- `prune` and `clear` never delete in place. They rename an entry into `<root>/.trash` (an atomic rename on one filesystem) and delete it there. A kill between the two leaves a complete `.trash` directory, not a half-deleted entry the tool might read.
- The manifest and setup snapshots are written to a temporary name and renamed into place.
- When the daemon starts it empties `.trash` and removes temporary files whose writer is gone. A store lock keeps two prunes or clears from running together; the second gets "busy".
- The tools' own caches (cargo, sccache, npm) handle concurrent readers and a killed writer themselves; that is what they were built for.

## Secrets policy

Caches hold no credentials and `[cache]` cannot carry one. Variable names are refused when they contain `TOKEN`, `SECRET`, `KEY`, `PASSWORD`, `PASSWD` or `CREDENTIAL`, start with `CAPSTAN_`, or are a name Capstan reserves (`PATH`, `HOME`, `LD_*`, `GIT_*` ...), or are already passed by `[env] pass`. Values in `set` must be one line without credential-shaped text. A real secret an agent needs (a registry token, a license key) goes through `[env] pass`. Prune and stop commands run with only `PATH`, `HOME`, `USER`, `LOGNAME`, `LANG`, `LC_ALL`, `TERM`, `TMPDIR` and the entry's own variables. Do not point a cache directory at a place that also holds credentials (for example `~/.npmrc`'s directory); the presets use their own subdirectories.

## Setup reuse

Installing dependencies is the other large cost of a new worktree (`npm ci`, `uv sync`). With `[worktree] setup` configured, `[cache.setup_reuse]` stores the result of the setup and gives it to the next worktree whose inputs are identical:

```toml
[worktree]
setup = "npm ci"

[cache.setup_reuse]
key_files = ["package-lock.json"]   # the files that identify the result
paths = ["node_modules"]            # the directories to store and restore
```

When a worktree is set up, the snapshot key is computed from the setup command, the platform, the content of every key file, the paths, and the variables of the enabled cache entries. If a snapshot with that key exists, its paths are copied into the worktree and **the setup command does not run**. Otherwise the setup runs, and when it succeeds the paths are stored as a snapshot for the next worktree. A snapshot lives in the `setup` entry (`setup/<key>`: visible in `cstan cache status`, removable with `cstan cache clear setup/<key>`, counted against the size and age bounds). Writing is atomic (copy to a temporary name, then rename) and the first snapshot for a key wins. A failed restore removes the partial copy and runs the setup normally; a failed store is logged and never fails the spawn.

Limits:

- It is a copy, not a link: restoring costs the time of the copy and the disk of a second copy per worktree. In the measurement a `cp -a` of `node_modules` (144 MiB) took 1.0 s against 3.2 s for a warm `npm ci`; the gain grows with the number of dependencies.
- It applies only when every key file exists in the new worktree and has the same content. A changed lockfile or setup command is a new key and a normal setup (which then fills the snapshot). Nothing is reused when a key file is missing.
- Only content that survives a copy to another path can be reused. Reuse is refused, with a `setup_reuse_refused` log entry, for a path that is tracked by git, a path that is not relative to the worktree, a directory holding `pyvenv.cfg` (a virtual environment records its absolute path and is not relocatable, so do not list `.venv`), and a symlink that leaves the worktree. pnpm's `node_modules` links into its store and copies poorly. Build outputs that embed absolute paths (`target/`) are better served by the `rust` presets.
- Only the listed paths are stored. Side effects of the setup elsewhere (global installs, generated files outside `paths`) are not reproduced when the setup is skipped.
- Snapshots are removed by `max_age_days` and the size cap like any entry; the next setup rebuilds them.

## Adding an entry for another tool

A tool without a preset gets `[cache.entries.<name>]`: `dirs` (variable = subdirectory of the entry's directory), `set` (fixed values), `requires`, `prune`, `stop` and `max_size_gb`. The rules are in [Configuration](configuration.md#adding-your-own-entry).

### Worked example: Gradle

Gradle keeps downloaded dependencies and its build cache in `GRADLE_USER_HOME`. Point it at an entry, and stop the daemon before the entry is cleared because a running daemon keeps files open:

```toml
[cache]
presets = ["rust"]
max_size_gb = 30

[cache.entries.gradle]
dirs = { GRADLE_USER_HOME = "gradle-home" }
set = { GRADLE_OPTS = "-Dorg.gradle.caching=true" }
requires = "gradle"
stop = ["gradle --stop"]
max_size_gb = 10
```

`requires = "gradle"` switches the entry off on a machine without Gradle on PATH (a project that only uses `./gradlew` drops `requires` and `stop`, and stops the daemon with `./gradlew --stop`). `GRADLE_USER_HOME` holds the wrapper distributions, the dependency cache and the build cache together; one entry covers all three. `gradle --stop` runs with the entry's variables, so it stops the daemon that uses this home.

### Worked example: Bazel

Bazel has a disk cache and a repository cache, both safe to share, and a server process per output base:

```toml
[cache.entries.bazel]
dirs = { BAZEL_DISK_CACHE = "disk", BAZEL_REPO_CACHE = "repo" }
requires = "bazel"
stop = ["bazel shutdown"]
max_size_gb = 15
```

Bazel does not read those variables by itself: the tool has to be told. Pass them from a wrapper the project commits (for example `tools/bazel`, which agents call instead of `bazel`):

```sh
#!/bin/sh
exec bazel "$@" ${BAZEL_DISK_CACHE:+--disk_cache="$BAZEL_DISK_CACHE"} ${BAZEL_REPO_CACHE:+--repository_cache="$BAZEL_REPO_CACHE"}
```

Capstan only sets the environment; this holds for any tool whose cache location is a flag, not a variable. Outside Capstan the variables are unset and the wrapper adds nothing.

## Measuring a project

`scripts/measure-build-cache.sh <repo> <setup-cmd> <build-cmd> [--cache-env file] [--reuse path]...` builds the same commit twice in fresh git worktrees, first with an empty cache root (cold), then with the filled one (warm), and prints wall times, sizes and, for sccache, hit statistics. Both worktrees and the temp root are removed afterwards, also after a failure. The `--cache-env` file holds `NAME=value` lines; `$CACHE_ROOT` stands for the run's cache root. The commands to use for Python and Node projects, and the results for this repository, are in [the measurement document](../investigations/build-cache-measurement.md#measuring-your-own-project).

## The `capstan.toml` for this repository

`capstan.toml` is not tracked, so apply this yourself. On the measurements, `shared-build-dir` is the best fit (warm build 4-6 s against 55-70 s cold, no sccache needed):

```toml
[cache]
presets = ["rust"]

[cache.rust]
mode = "shared-build-dir"
```

Dropping the `npm ci` setup step is a separate decision (plan-31); do not add `node` to `presets` for it.
