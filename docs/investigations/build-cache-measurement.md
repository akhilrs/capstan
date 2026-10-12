# Build cache measurement

Cold and warm builds of this repository's Rust workspace and of `npm ci`, with the cache presets of [Build cache](../reference/build-cache.md). Measured 2026-10-12 with `scripts/measure-build-cache.sh` on commit `4f9abfa`.

## Setup

- Machine: Linux 6.1, 8 cores, 15 GB RAM, other agents running on the same machine (see "Noise").
- Tools: cargo 1.99.0, rustc 1.99.0, sccache 0.18.0 (already installed; nothing was installed for this work), npm 11.5.1, pnpm 10.14.0, uv 0.8.11, pip 25.2, go 1.25.0.
- Each measurement is one `scripts/measure-build-cache.sh` call: two fresh git worktrees of `HEAD`; the first run (cold) starts with an empty cache root, the second (warm) uses the root the first filled. The shared downloads in `HOME` (`~/.cargo/registry`) were already present for every row, so the table shows build output reuse, not download time.
- Rust: setup `cd rust && cargo fetch --locked`, build `cd rust && cargo build --workspace --all-targets && cargo test --workspace --no-run`.
- Cache env files: `no cache` none; `shared-build-dir` `CARGO_BUILD_BUILD_DIR=$CACHE_ROOT/cargo-build`; `sccache` `SCCACHE_DIR=$CACHE_ROOT/sccache`, `RUSTC_WRAPPER=sccache`, `SCCACHE_CACHE_SIZE=20G` (the preset's values).

## Results: Rust workspace

Wall time of the build step in seconds, two runs each (run 1 / run 2). Disk is the size of one worktree after the build plus the cache root.

| Row | Cold | Warm | Disk after warm | Hit rate |
| --- | --- | --- | --- | --- |
| no cache | 77.7 / 66.7 s | 58.9 / 85.2 s | worktree 5.57 GiB; cache 0 | n/a |
| shared-build-dir | 55.5 / 69.6 s | 5.7 / 4.0 s | worktree 0.45 GiB; cache 5.58 GiB | n/a |
| sccache | 56.8 / 59.7 s | 50.0 / 49.0 s | worktree 5.57 GiB; cache 0.05 GiB | 92.3% (60 of 65 requests; run 2) |

sccache with `CARGO_INCREMENTAL=0` added (one run, not a preset): cold 52.3 s, warm 39.0 s, worktree 4.27 GiB, cache 0.11 GiB, hit rate 77.9%.

Reading:

- `shared-build-dir` is the only row with a large gain: a warm build is 10-15 times faster and every extra worktree costs 0.45 GiB instead of 5.6 GiB. The first worktree pays the full 5.6 GiB once into the shared directory.
- `sccache` hit rate is high but the time gain is small, because the cargo dev profile compiles the workspace crates incrementally and sccache cannot cache those (190 of 242 uncacheable calls in an earlier run), and linking is not cached. It stores very little (52 MiB).
- `no cache` has no systematic difference between cold and warm: the spread between runs (59-85 s) is larger than the effect.

Noise: the machine ran other agents during the measurement, so single runs vary by about 20 s (the `no cache` warm times are 58.9 s and 85.2 s for the same work). Read the table as orders of magnitude, not as exact percentages. One concurrency check was made separately: two worktrees running `cargo check --workspace --all-targets` at once on one `CARGO_BUILD_BUILD_DIR` printed `Blocking waiting for file lock on build directory` once, so builds on a shared build directory wait for each other.

### The sccache first run

The first sccache run reported all counters as 0 because the script read the statistics from a different sccache server than the build used. The script now starts the build and the statistics on one private server port and stops it afterwards. The times of that run (56.8 s cold, 50.0 s warm) are kept in the table because the cache itself worked.

## Results: `npm ci`

Setup `npm ci --ignore-scripts --no-audit --no-fund`, build `true`; wall time of the setup step. `NPM_CONFIG_CACHE=$CACHE_ROOT/npm` for the cache rows.

| Row | Cold | Warm | Disk |
| --- | --- | --- | --- |
| no cache variable (uses `~/.npm`) | 3.3 s | 3.2 s | worktree 144 MiB |
| npm cache in the cache root, no setup reuse | 5.0 s | 3.2 s | worktree 144 MiB; cache 21 MiB |
| npm cache + setup reuse (`--reuse node_modules`) | 5.1 s | 1.0 s | worktree 144 MiB; cache 21 MiB (the snapshot adds another 144 MiB) |

Setup reuse in this measurement is the script's `--reuse`: a `cp -a` of `node_modules` from the cold worktree, which stands in for restoring a stored snapshot (the real restore also copies; the time of the key hash and the store is not included). The gain is small here (2.2 s) because the dependency tree is small and `~/.npm` was already warm.

## Variable names checked against the tools

Each check sets the preset (upper-case names, as the preset exports them)'s variable to a path in a scratch directory and asks the tool where its cache is.

| Variable | Check | Tool | Result |
| --- | --- | --- | --- |
| `NPM_CONFIG_CACHE` | `npm config get cache` | npm 11.5.1 | returns the path set |
| `NPM_CONFIG_STORE_DIR` | `pnpm store path` | pnpm 10.14.0 | returns `<path>/v10` (pnpm adds its store version directory); without the variable `~/.local/share/pnpm/store/v10` |
| `UV_CACHE_DIR` | `uv cache dir` | uv 0.8.11 | returns the path set |
| `PIP_CACHE_DIR` | `pip cache dir` | pip 25.2 | returns the path set |
| `GOCACHE`, `GOMODCACHE` | `go env GOCACHE GOMODCACHE` | go 1.25.0 | returns both paths set |
| `YARN_CACHE_FOLDER` | | yarn | tool not installed |
| `POETRY_CACHE_DIR` | | poetry | tool not installed |
| `SCCACHE_DIR`, `RUSTC_WRAPPER` | `sccache --show-stats` shows the cache location | sccache 0.18.0 | location was the directory set |
| `CARGO_BUILD_BUILD_DIR` | the `cargo-build` directory was 5.5 GiB after a build, the worktree's `target` 0.45 GiB | cargo 1.99.0 | honored |

## Measuring your own project

Run from a clone of this repository against the project to measure (`<repo>` is the project's checkout; it must have the commit committed, since the script builds `HEAD`). One command per row; run them one at a time, each needs the disk of its worktrees. Use `--cache-env` files like these:

```sh
printf 'PIP_CACHE_DIR=$CACHE_ROOT/pip\nUV_CACHE_DIR=$CACHE_ROOT/uv\n' > /tmp/python.env
printf 'NPM_CONFIG_CACHE=$CACHE_ROOT/npm\nNPM_CONFIG_STORE_DIR=$CACHE_ROOT/pnpm-store\n' > /tmp/node.env
```

Python, uv:

```sh
scripts/measure-build-cache.sh <repo> 'uv sync --frozen' 'uv run python -c "import sys"' --cache-env /tmp/python.env
```

Python, pip:

```sh
scripts/measure-build-cache.sh <repo> 'python -m venv .venv && .venv/bin/pip install -r requirements.txt' '.venv/bin/python -c "import sys"' --cache-env /tmp/python.env
```

Node, npm:

```sh
scripts/measure-build-cache.sh <repo> 'npm ci' 'npm run build' --cache-env /tmp/node.env
```

Node, pnpm:

```sh
scripts/measure-build-cache.sh <repo> 'pnpm install --frozen-lockfile' 'pnpm run build' --cache-env /tmp/node.env
```

For a "no cache" row leave out `--cache-env`; for a setup reuse row add `--reuse node_modules` (npm). Do not use `--reuse .venv` (a virtual environment is not relocatable) or `--reuse node_modules` with pnpm without its store (the links point into the store). Replace the build command by `true` to measure installation only.

## For this repository

The recommended `capstan.toml` (untracked, to be applied by the user) is in [Build cache](../reference/build-cache.md#the-capstantoml-for-this-repository):

```toml
[cache]
presets = ["rust"]

[cache.rust]
mode = "shared-build-dir"
```

`npm ci` is not part of this change: whether to drop the step stays with plan-31.

## Not measured

- Python (`uv sync`, `pip install`) and Node with pnpm were not timed; their variable names were checked and their commands are above.
- The real restore path through Capstan (rather than the script's `cp -a`) was not measured.
- Several agents building at the same time with each preset (only the two-worktree lock check above was run).
