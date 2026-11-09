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
| any `rust` mode except `off`, with `target_dir = true` | also `CARGO_TARGET_DIR` (subdirectory `cargo-target`) | `cargo` |
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

The daemon measures and prunes every 6 hours (off the controller's main thread; sooner when the last measurement was over the cap). A prune first runs the tools' own commands (`uv cache prune`, `npm cache verify`, `pnpm store prune`), then removes entries older than `max_age_days`, then trims until the total is under the cap. While no agent worktree exists it may remove whole entries; while agents work it only evicts what the rules below allow and reports the rest. Sizes come from walks that are bounded in time; a partial figure is marked `partial`.

| Key | Default | Meaning |
| --- | --- | --- |
| `evict_while_active` | `true` | While agents work, a prune may still evict unleased gate slots and old, unused units. `false` reports only. |
| `active_evict_min_age_minutes` | 120 | While agents work, a unit must be untouched this long before it is evicted; 0 to 10080, 0 means no age rule. |

`sccache` is given `SCCACHE_CACHE_SIZE` as the entry's own `max_size_gb`; with none, 40% of the total (at least 1 GB), so 5 GB gives 2G and 10 GB gives 4G.

### Eviction units

An entry over the cap is trimmed inside, one unit at a time, instead of being removed whole. A unit is a directory that holds the entry's `unit_lock` file (rust: `.cargo-lock`, which cargo locks while it builds there); the rust preset sets it with `target_dir = true`. The order is: unleased gate slots, incremental sub-units (`incremental/` inside a unit), whole units, setup keys, entries. A prune takes the unit lock without blocking, empties the unit while holding it, and keeps the lock file and directory, so a build waiting on the lock simply proceeds cold. After taking a lock the prune checks that the file it holds is still the file at that path (device and inode); a replaced lock file means a build is recreating the unit, which counts as `building`. A unit whose lock is held stays (`building`).

While agents run, a unit is also kept when anything inside it changed within `active_evict_min_age_minutes` (`recently used`) or when a process runs a binary from it or has it as its working directory (`a process runs from it`, read from `/proc`). If an own-user process has its exe, cwd and command line all unreadable, the prune cannot tell what it uses and every unit counts as in use (fail safe). When idle the age and process rules do not apply, only the lock. An entry over the cap that has no units, or only a tool that trims itself (sccache), is listed with a reason (`no evictable units`, `trimmed by sccache`) and not enforced.

Occupants are scratch directories the daemon knows about, such as the verify scratch worktree: they count against the cap, are listed (`held by verify`) and never removed by a prune. Occupants arrive with a later package; until then the list is empty and the periodic daemon prune records only what it skipped (`lastSkipped`).

`cstan cache status --json` adds, next to the sizes: `overCapBytes` (how far over the cap, also per entry), `notEnforced` (`[{label, reason}]`), `lastSkipped` (what the last daemon prune left and why, `reasons` as above) and `occupants` (`[{label, bytes, at}]`).

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

### A persistent target directory (`target_dir`)

`[cache.rust] target_dir = true` (default `false`) adds `CARGO_TARGET_DIR` pointing into the cache, so a build keeps its compiled output between runs. Agents share that one directory: cargo holds a file lock on a target directory, so two builds at once wait for each other (`Blocking waiting for file lock`) instead of corrupting it. A verify gate does not use it; it builds in its own slot (see below). The directory is part of the entry, so `max_size_gb` and `max_age_days` bound it like the rest of the cache. Never run `cargo clean` (or delete the directory) while `CARGO_TARGET_DIR` is set: it removes what every other agent is building in. Scripts that build several trees into one `CARGO_TARGET_DIR` should give each its own subdirectory, as `scripts/check.sh` does. Other presets need no such option: their cache directories (`GOCACHE`, `PIP_CACHE_DIR`, `NPM_CONFIG_CACHE`, ...) are safe to share between concurrent runs by the tools' own design.

### Gate build directories

With `target_dir = true` a `cstan verify` run gets `CARGO_TARGET_DIR` (and, in mode `shared-build-dir`, `CARGO_BUILD_BUILD_DIR`) pointing at a slot of its own: `<entry>/gate-target/<project>/slot-<n>` and `<entry>/gate-build/<project>/slot-<n>`. The entry names those variables in its `gate_dirs`; a project with no `gate_dirs` entry (other languages, rust with `target_dir` off) keeps the same cache variables as an agent. Every other cache variable (`SCCACHE_DIR`, ...) is unchanged, and `[verify].env` still wins on a clash.

- Slots per project: `n` is the index of the verify worker, so there are `max_parallel` slots and two workers never share one. `max_parallel` is read when the daemon starts; a lower value at the next start removes this project's surplus slots that nobody holds (`verify_slot_removed`, or `verify_slot_skipped` when one is held).
- Cold first run: the first gate in a slot builds from nothing (sccache helps when it is on), so a project with a short `[verify] timeout_seconds` may see its first gate per slot time out. Raise the timeout or warm the slot once.
- Disk per slot: one full build tree, about 3 GB on this repository, times `max_parallel`. The slots count against the entry's `max_size_gb`.

### Gate builds without debug info

A gate builds without debug info by default: in a verify gate and in the Shared fallback, the rust preset adds `CARGO_PROFILE_DEV_DEBUG=0` and `CARGO_PROFILE_TEST_DEBUG=0` to the environment, after the entry's other variables (the entry's `gate_set`; an agent and a gate in Bare scope never get them, and python, node and go set none). It makes gate builds faster and smaller. `[verify].env` still wins on a clash. Tests that need debug info, such as backtrace line numbers, may differ in a gate: set `[cache.rust] gate_debug = true` (default `false`) to leave the project's own setting in force.

Limits: only the global `dev` and `test` profiles are changed, which `cargo build` and `cargo test` use; `release`, `bench` and custom profiles keep the project's settings. The variables override a `debug` setting in the project's `.cargo/config.toml` for gates only, but a per-package override (`[profile.dev.package.<name>]`, `[profile.test.package.*]`) still wins. Changing the setting changes cargo's fingerprints, so the first gate per slot after an upgrade is a full rebuild (about 1.5 times the usual gate time) and old and new artifacts coexist in the slot, about 2.5 GB more, until the old ones are evicted. A gate never reuses an artifact an agent built, since agents keep debug info.

### Leases

A gate holds its slot for the whole run, from before the scratch worktree is added until after its teardown hook has run, through an `flock` on `<entry>/.slot-<project>-<n>.lease`. The lease dies with the process; the descriptor is close-on-exec, so a build that outlives its gate does not keep the slot. Waiting for a lease counts against `[verify] timeout_seconds`; a slot still held after the wait ends the run as `error: infrastructure: verify slot <n> is held by another process` (a hung old daemon of the project, or a second host on a shared root).

- Prune and clear skip a leased slot (`in use`); the lease files sit beside the slots, outside any directory a build can remove.
- Leases need a local disk. At start-up the daemon checks that a second open file description cannot take a lock the first one holds; on NFS or a synced folder it logs `cache_lease_unreliable`, removes no slot, and runs every gate in the Shared fallback.
- Infrastructure rerun: if a step fails and a slot directory is gone, replaced, or lost one of its top-level subdirectories, the lease is renewed and the whole step list is run again from step 1 (a later step may need an earlier step's output in the wiped directory), once per run and inside the original deadline. A rerun that fails with the slot intact is an ordinary failure whose text starts `after an infrastructure rerun from step 1:`; a second removal is `error: infrastructure: the gate build directory was removed during the run`. A `cargo clean` or `rm` that removes only files under `target/debug` is not detected and reports as an ordinary failure.
- The Shared fallback (the self test failed, or the lease could not be taken: `verify_lease_unavailable`): the cache variables are kept except the ones `gate_dirs` names, so cargo builds in the scratch worktree's own `target`. Every gate then builds cold, so each may approach `timeout_seconds`. A recovered orphan run's teardown also runs Shared, because its slot is unknown.
- With the cache off a gate runs with no cache variable at all (Bare) and takes no lease.

## The `capstan.toml` for this repository

`capstan.toml` is not tracked, so apply this yourself. On the measurements, `shared-build-dir` is the best fit (warm build 4-6 s against 55-70 s cold, no sccache needed):

```toml
[cache]
presets = ["rust"]

[cache.rust]
mode = "shared-build-dir"
```

Dropping the `npm ci` setup step is a separate decision (plan-31); do not add `node` to `presets` for it.
