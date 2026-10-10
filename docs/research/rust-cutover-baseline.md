# Cutover baseline: Node and Rust, side by side

Re-measures section 2 of [rust-port-estimate.md](rust-port-estimate.md) now that Rust code exists. Same method as there, same
commands for both sides, on scratch projects only: the live `.capstan` ledger and other projects were never opened. By hand
with `docs/research/cutover-baseline/measure.sh` (and `seed.mjs` beside it); it needs the frozen Node build, so it is outside
`scripts/check.sh` (the no-node rule applies to the gate, not to this).

## Setup

| | |
| --- | --- |
| Date, host | 2026-10-10, Linux 6.1.0-48-amd64 x86_64, 8 CPUs, 16 GB, a busy shared machine (other agents building) |
| Node side | `node dist/src/cli.js` (`npm run build`, the frozen tree at this commit), node v24.6.0; its daemon is the Node daemon that `cstan start` selects by default |
| Rust side | `cargo build --release --locked -p cstan-front`, `cstan 0.3.0` (the workspace version); `cstan start` runs its own `cstan daemon` |
| Environment | `env -i HOME=<scratch> PATH=/usr/bin:/bin CAPSTAN_LAUNCH=off` (no Herdr, no agent identity unless a row says so) |
| Projects | a fresh `git init` + `cstan init` per side, a PM and a developer registered in the ledger with the Node build (`seed.mjs`, the registration of the parity harness); no daemon owned the ledger while it was seeded |
| Large ledger | `node scripts/shadow-daemon.mjs --generate` (50 agents, 5000 messages, 20 plans with reports and integrations; 12.0 MB `controller.sqlite`), generated once per side |
| CPU | percent of **one core** from `/proc/<pid>/stat` (utime + stime, `CLK_TCK` 100) over 60 s after a 5 s settle |
| RSS | `VmRSS` of `/proc/<pid>/status` |
| Per call | 20 runs each; wall time in ms from `date +%s%N` around `/usr/bin/time -f %M` (so it includes `/usr/bin/time`'s own exec, about 1 ms), max RSS from `%M` |

## Results

### Scratch daemon, idle

| | Node daemon | Rust daemon |
| --- | --- | --- |
| CPU, 60 s | 0.07% | 0.17% |
| RSS | 75.8 MB (`VmHWM` the same) | 9.3 MB |
| Threads | 7 | 6 |

The Rust daemon uses a little more CPU idle (0.17% against 0.07%: one 60 s reading each, so no more than "both are negligible") and one eighth of the memory. The estimate's 83.2 MB / 0.08% for Node reproduces.

### One `cstan` call, scratch project with the daemon running

| Command | Node wall median (min to max) | Node max RSS | Rust wall median (min to max) | Rust max RSS |
| --- | --- | --- | --- | --- |
| `ping` | 227 ms (218 to 256) | 74.5 MB | 4 ms (4 to 5) | 4.4 MB |
| `status --json` | 216 ms (210 to 230) | 74.3 MB | 4 ms (4 to 5) | 4.4 MB |
| `inbox` (as the PM) | 222 ms (216 to 234) | 72.6 MB | 4 ms (4 to 6) | 3.7 MB |

Roughly 50 times faster and 17 to 20 times smaller. The estimate predicted 5 to 15 ms and 2 to 4 MB for Rust; it is 4 ms
and 3.7 to 4.4 MB.

### A blocked `cstan wait` (a developer with nothing queued, sampled 3 s in)

| | Node | Rust |
| --- | --- | --- |
| RSS | 72.2 MB | 1.8 MB |

Every agent holds one of these for up to 90 s, so a team of ten saves about 700 MB.

### `status --json` on the large generated ledger

| | Node | Rust |
| --- | --- | --- |
| Wall median (min to max) | 230 ms (222 to 243) | 8 ms (7 to 12) |
| Client max RSS | 74.2 MB | 4.8 MB |
| Daemon RSS afterwards | 82.1 MB | 11.3 MB |

On the estimate's live ledger (66.7 MB) the Node daemon took 135 ms per `status`; this generated ledger is 12.0 MB, so the
call here is dominated by the client's start-up (about 215 ms of the 230 ms on the Node side). The Rust side is 8 ms in total.
The comparison that the estimate could not make, the daemon's own handler time on a large live ledger, is not repeated here:
the live ledger was not copied or opened.

## Notes and limits

- One 60 s idle window per side and 20 calls per command; a shared, busy machine. Treat the idle CPU figures as order of
  magnitude, the RSS and the call times as stable (the spread is in the table).
- The Rust numbers are for the build of this commit, release profile, not the musl release binaries.
- Not measured, as in the estimate: a daemon with a launching Herdr and real agents, and the daemon's CPU under the live
  ledger's `status` load (0.9 requests per second against a 66.7 MB ledger).
