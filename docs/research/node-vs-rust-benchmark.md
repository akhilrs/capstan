# Node v0.3.0 against Rust v0.4.0: what the migration changed for users

Release to release: the published `v0.3.0` Node standalone and the published `v0.4.0` Rust binary, same machine, same scratch
projects, same commands, interleaved. No product code was changed. Raw samples are in
[node-vs-rust-benchmark/results.tsv](node-vs-rust-benchmark/results.tsv), per-metric statistics in
[results-summary.tsv](node-vs-rust-benchmark/results-summary.tsv).

## Summary

"Node" is `cstan-0.3.0-linux-x64` (the Node 24.6.0 standalone: CLI and daemon in one file). "Rust" is `cstan-0.4.0-linux-x64`.
Medians of 3 interleaved rounds (20 calls per command per round; 3 x 5 daemon start/stop cycles; 3 x 30 latency samples).

| Area | Node 0.3.0 | Rust 0.4.0 | Rust vs Node |
| --- | --- | --- | --- |
| Download size | 131.7 MB | 10.0 MB | 13 times smaller (92% less) |
| Runtime dependency | none (Node is embedded) | none (static binary) | same |
| One CLI call (`status`, `ping`, `send`, `inbox`), wall | 182 to 185 ms | 6 to 7 ms | about 27 to 30 times faster |
| `--version`, wall | 174 ms | 4 ms | 43 times faster |
| One CLI call, max RSS | 68 to 70 MB | 1.3 to 2.5 MB | 28 to 36 times less (about 97% less) |
| Daemon cold start (`cstan start` until it answers) | 404 ms | 114 ms | 3.5 times faster |
| Daemon stop | 282 ms | 105 ms | 2.7 times faster |
| Idle daemon RSS (small ledger) | 73.2 MB | 7.5 MB | 90% less |
| Daemon RSS after 200-message burst | 99.8 MB | 24.7 MB | 75% less |
| 10 agents blocked in `cstan wait` (sum of their RSS) | 658 MB | 14.4 MB | 98% less (46 times) |
| Burst of 200 sends from 8 agents at once | 33 msg/s | 430 msg/s (130 to 467) | about 13 times faster |
| One agent doing send + inbox + ack in a loop | 1.58 per s | 9.54 per s | 6 times faster |
| Send return until the waiting recipient's `wait` returns | 218 ms | 1.7 ms | about 130 times faster |
| Large ledger (12 MB) `status --json`, wall | 190 ms | 12 ms | 16 times faster |
| Large ledger daemon RSS after calls | 93.6 MB | 28.0 MB | 70% less |

Where there is no difference or Node is ahead: idle daemon CPU is the same at this resolution (0.12% against 0.15% of one core;
one scheduler tick per 60 s is 0.017 points). The dashboard binary of 0.3.0 is already the Rust `cstan-dash`, so 0.3.0 to 0.4.0
changed it little (the 0.4.0 dashboard used 1.1 MB more RSS; CPU was not higher). A 12 MB ledger takes the Rust daemon 414 ms to
open against 533 ms for Node, but the ranges overlap (Node 408 to 618, Rust 414 to 614), so call that a tie. The daemon's CPU time
per message is only about twice as good (see below); most of the user-visible speed-up is the client and the process start, not the
daemon's message handling.

## Setup

| | |
| --- | --- |
| Date, host | 2026-10-10, Linux 6.1.0-48-amd64 x86_64, QEMU virtual CPU (8 vCPUs), 16 GB RAM, ext4. No cpufreq governor is exposed (a VM). |
| Machine load | shared and busy (other agents building and testing). 1-minute load average sampled before and after each side's small-ledger run: 0.9 to 8.5 before, 1.2 to 13.4 after (all values in `results.tsv`, metric `machine.loadavg_*`). Mitigation: A/B/A/B interleaving and 3 rounds, not a quiet machine. |
| Node side | `cstan-0.3.0-linux-x64` from the v0.3.0 release: an ELF, dynamically linked against glibc, with Node v24.6.0 embedded (single executable application). `cstan start` runs the Node daemon as `cstan daemon` from the same file. SHA256 verified against the release's `SHA256SUMS`. |
| Node, npm form (extra) | `capstan-controller-0.3.0.tgz` (verified) unpacked, `npm ci --omit=dev --ignore-scripts`, run as `node dist/src/cli.js` with the system `node` v24.6.0. Called `npm` in the data. It is what an npm install would run. |
| Rust side | `cstan-0.4.0-linux-x64` from the v0.4.0 release: a static executable (`ldd`: not dynamic). `cstan start` runs `cstan daemon` from the same file. SHA256 verified. |
| Dashboards | `cstan-dash-0.3.0-linux-x64` and `cstan-dash-0.4.0-linux-x64` (both verified). Also the Node (Ink) dashboard inside the 0.3.0 standalone, `CSTAN_DASH=node cstan dash`. |
| Environment | `env -i HOME=<scratch> PATH=/usr/bin:/bin CAPSTAN_LAUNCH=off`, plus `CAPSTAN_TOKEN`, `CAPSTAN_SOCKET`, `CAPSTAN_AGENT_ID` where a command needs an identity. No Herdr. |
| Projects | a fresh `git init` + `cstan init` per side and round, the same `capstan.toml`, a PM and 10 developers registered with the same Node 0.3.0 core (`seed.mjs`), so both start from an identical ledger. A Rust daemon migrates the seeded schema (v36 to v37) on its first start; the first start is reported separately (`daemon.first_start_ms`) and is not in the cold start statistic. |
| Large ledger | `scripts/shadow-daemon.mjs --generate` run against the 0.3.0 core: 50 agents, 5000 messages, 20 plans, 12.0 MB `controller.sqlite`. Generated afresh for each side and round, because a project's state directory is path-bound. |

## Method

* Order: round r runs `node`, `rust`, `npm` one after the other, each with its own scratch project; 3 rounds (A/B/C/A/B/C...). Per-round medians are in `results-summary.tsv` (last column) to show the spread between rounds.
* Per call: 20 runs per command and round (n = 60 per side). Wall time from `date +%s%N` around `/usr/bin/time -f %M` (so it includes `time`'s own exec, about 1 ms), max RSS from `%M`.
* Daemon idle: 5 s settle, then 60 s of `/proc/<pid>/stat` utime + stime (CPU as percent of one core; the tick is 10 ms) and `VmRSS`.
* Cold start: wall time of `cstan start`, which returns when the daemon answers; stop is the wall time of `cstan stop`. 5 stop/start cycles per side and round.
* `cstan wait` with nothing queued blocks up to 90 s, so it has no wall time; its RSS is read from `/proc` 3 s after it blocked (`wait.blocked_rss_kb`). Ten of them (developer-1 to 10) are blocked at once for `waiters10.*`.
* Latency: the PM is blocked in `cstan wait`; a developer runs `cstan send`; two intervals are recorded, send start until the wait returned (what a user sees, includes the sending CLI's start-up) and send return until the wait returned (the controller's push path). 10 samples per side and round.
* Throughput: (a) sequential: 60 x (developer `send`, PM `inbox`, PM `ack`), one agent at a time, so it is bound by CLI start-up; (b) burst: 8 developers send 25 messages each at the same time (200 sends), daemon CPU (ticks) and peak RSS (`clear_refs` reset before, `VmHWM` after) recorded, then the PM reads the inbox once and acks the 200 messages with 8 parallel `ack` processes.
* Large ledger: 20 runs each of `status --json`, `inbox` and `plan show` as the PM, then the daemon's RSS and 30 s of idle CPU.
* Dashboards: 30 s in a 160x45 pty against the live scratch daemon (`dash-sample.py`), CPU of the dashboard process only, RSS at the end, refresh interval 1 s.
* Safety: only scratch repositories under `/tmp/capstan-cutover-bench`; no live `.capstan` ledger was opened. Every daemon was started by the script from a scratch project and its `/proc/<pid>/cwd` was checked against the project before anything else touched it; the run never signalled a process it did not start. Disk use stayed under 200 MB; downloads and scratch directories were deleted afterwards.

## Results

### 1. Footprint

| | Node 0.3.0 standalone | Rust 0.4.0 | Node 0.3.0 npm form |
| --- | --- | --- | --- |
| Download | 131,665,088 B (131.7 MB) | 10,006,320 B (10.0 MB) | 391,487 B (`.tgz`) |
| Installed on disk | 131.7 MB, 1 file | 10.0 MB, 1 file | 12.1 MB, 5,159 files (`dist` 1.6 MB in 162 files, `node_modules` 10.4 MB in 4,993 files after `npm ci --omit=dev`) |
| Runtime dependency | none (embeds Node v24.6.0; glibc) | none (static, no libc dependency) | `node` >= 24.6 < 25 on the machine (the `node` binary is 129 MB, the Node tree 811 MB), plus the npm install |
| Dashboard binary | `cstan-dash-0.3.0` 1,311,568 B | `cstan-dash-0.4.0` 1,235,256 B | not available (uses the Node dashboard) |

The 0.3.0 release also shipped `cstan-front-0.3.0` (598,576 B), the Rust client that needs a Node implementation beside it; it was
not measured, because the standalone is the single artifact that works alone. A user's 0.3.0 download of standalone + dash was 133 MB;
0.4.0 standalone + dash is 11.2 MB.

### 2. One CLI call (small ledger, daemon running; n = 60 per cell)

| Command | Node wall median (p95) | Rust wall median (p95) | Wall ratio | Node max RSS | Rust max RSS |
| --- | --- | --- | --- | --- | --- |
| `--version` (no daemon) | 174 ms (227) | 4 ms (5) | 43 | 67.9 MB | 1.26 MB |
| `ping` | 182 ms (234) | 6 ms (7) | 30 | 68.7 MB | 2.47 MB |
| `status --json` | 183 ms (219) | 6 ms (8) | 30 | 67.9 MB | 1.91 MB |
| `inbox` (empty) | 182 ms (230) | 5 ms (6) | 36 | 69.2 MB | 1.64 MB |
| `send` | 185 ms (300) | 7 ms (13) | 26 | 69.5 MB | 1.69 MB |
| `plan show` | 185 ms (233) | 5 ms (6) | 37 | 67.9 MB | 1.69 MB |
| `config check` | 180 ms (209) | 5 ms (5) | 36 | 68.0 MB | 2.16 MB |
| `wait`, nothing queued (RSS 3 s in) | blocks | blocks | n/a | 63.6 MB | 1.44 MB |

The Node npm form is slower than the standalone on every row (218 to 228 ms wall, 72 to 73 MB): the reason was not investigated.
Rust's rare slow outliers (maximum 36 to 40 ms) were not traced; the machine was shared and busy.

### 3. Daemon (small ledger)

| Metric | Node | Rust | Ratio / note |
| --- | --- | --- | --- |
| Cold start, `cstan start` until it answers | 404 ms (p95 432, n = 15) | 114 ms (p95 150) | 3.5 times faster |
| First start on the seeded ledger | 405 ms | 113 ms | Rust includes the schema migration |
| Stop | 282 ms (p95 344) | 105 ms (p95 107) | 2.7 times faster |
| Idle CPU, 60 s (one core) | 0.12% | 0.15% | no difference at this resolution (rounds: Node 0.13/0.12/0.12, Rust 0.15/0.13/0.15) |
| Idle RSS | 73.2 MB | 7.5 MB | 90% less |
| Threads | 7 | 6 | |
| Burst, 200 sends from 8 agents: wall | 5,986 ms | 465 ms | median; Rust rounds 1,541 / 465 / 428 ms |
| Burst send throughput | 33.4 msg/s | 430 msg/s | 13 times; Rust round 1 was 130 msg/s on a loaded machine (467 and 430 in rounds 3 and 2) |
| Burst: daemon CPU time | 490 ms (2.45 ms per message) | 240 ms (1.2 ms per message) | 2 times less CPU |
| Burst: daemon peak RSS | 93.2 MB | 20.4 MB | 78% less |
| Burst ack: 200 acks, 8 at a time | 35 msg/s | 218 msg/s | 6.2 times; daemon CPU 940 against 720 ms |
| Daemon RSS after the whole small-ledger suite (2.2 MB ledger) | 99.8 MB | 24.7 MB | 75% less |
| Sequential send + inbox + ack | 1.58 per s | 9.54 per s | 6 times (bound by CLI start-up in both) |
| Latency, send start until `wait` returns | 402 ms (p95 509) | 11.5 ms (p95 34) | 35 times |
| Latency, send return until `wait` returns | 218 ms (p95 321) | 1.65 ms (p95 3.3) | 130 times |
| 10 blocked `wait` processes, sum of RSS | 658 MB | 14.4 MB | 46 times less |
| ... daemon RSS with the 10 waiters | 81.7 MB | 12.1 MB | |

The waiting `cstan wait` is already running when the message arrives, so the send-return-to-wait-return interval contains no CLI
start-up; the cause of Node's 218 ms there (against 1.7 ms) was not investigated. It is the time a user sees, because the
interval ends when the waiting process exits. The minimum values (Node 3.8 ms, Rust -0.1 ms) are samples where the waiter returned
at about the moment `send` exited. Some Node samples are also much higher than the minimum (p95 321 ms).

The burst and ack throughput are mostly client-bound: 8 parallel Node CLIs at about 180 ms each and a shared 8-core machine. The daemon
CPU rows are the honest daemon-side comparison: about 2 times less CPU per message for Rust, not 13 times.

### 4. Large ledger (12 MB generated: 50 agents, 5000 messages, 20 plans)

| Metric | Node | Rust | Ratio |
| --- | --- | --- | --- |
| `status --json`, wall median (p95) | 190 ms (223) | 12 ms (17) | 16 |
| `inbox` (PM), wall median (p95) | 190 ms (244) | 22 ms (29) | 8.6 |
| `plan show`, wall median (p95) | 182 ms (277) | 8 ms (9) | 23 |
| `status --json`, client max RSS | 67.9 MB | 2.3 MB | 29 times less |
| `inbox`, client max RSS | 69.5 MB | 2.6 MB | 26 times less |
| Cold start with this ledger | 533 ms (408 to 618) | 414 ms (414 to 614) | not distinguishable (ranges overlap) |
| Daemon RSS after start | 74.6 MB | 18.3 MB | 76% less |
| Daemon RSS after the 60 calls (peak the same) | 93.6 MB | 28.0 MB | 70% less |
| Idle CPU, 30 s | 0.13% | 0.13% | no difference |
| Stop | 280 ms | 106 ms | 2.6 times faster |

On the large ledger the Node call time barely moves (190 against 183 ms on the small one): it is dominated by the client start-up.
Rust grows from 6 to 12 ms, so the daemon's query work is visible for Rust and hidden for Node.

### 5. Dashboard (30 s, 160x45 pty, 1 s refresh, small ledger, daemon idle)

| | CPU (one core) | RSS |
| --- | --- | --- |
| `cstan-dash-0.3.0` (Rust) | 2.96% (2.96, 2.93, 2.96) | 2.35 MB |
| `cstan-dash-0.4.0` (Rust) | 2.23% (2.23, 2.30, 2.13) | 3.45 MB |
| Node (Ink) dashboard in the 0.3.0 standalone (`CSTAN_DASH=node cstan dash`) | 9.54% | 267.5 MB |

There is no Node dashboard in 0.4.0, and the 0.3.0 `cstan-dash` is already the Rust one: the user-visible dashboard change from the
Node standalone is 9.5% to about 2.5% CPU and 268 MB to about 3 MB, and from 0.3.0 `cstan-dash` to 0.4.0 `cstan-dash` there is no real
difference (CPU lower, RSS 1.1 MB higher; both within what a busy machine and a single 30 s window can tell apart). The 1 s interval is
shorter than the default, so these CPU figures are high against the earlier `rust-dash-performance.md` figures for the default interval and a fake daemon.

### 6. Other: 10 idle worker CLIs

This is the "team of ten" case from `rust-cutover-baseline.md`: ten agents each holding a `cstan wait` costs 658 MB with Node and 14.4 MB
with Rust, on top of the daemon (81.7 MB against 12.1 MB). For ten blocked agents plus the daemon the resident set goes from about 740 MB to about 27 MB.

## Chart-friendly table

Medians; lower is better except throughput. Units in the column header.

| Metric | Unit | Node 0.3.0 | Rust 0.4.0 |
| --- | --- | --- | --- |
| Download size | MB | 131.7 | 10.0 |
| CLI `--version` wall | ms | 174 | 4 |
| CLI `ping` wall | ms | 182 | 6 |
| CLI `status --json` wall | ms | 183 | 6 |
| CLI `send` wall | ms | 185 | 7 |
| CLI `status --json` wall, 12 MB ledger | ms | 190 | 12 |
| CLI `inbox` wall, 12 MB ledger | ms | 190 | 22 |
| CLI `plan show` wall, 12 MB ledger | ms | 182 | 8 |
| CLI max RSS (`status`) | MB | 67.9 | 1.9 |
| Blocked `wait` RSS | MB | 63.6 | 1.4 |
| 10 blocked `wait`, sum | MB | 658 | 14.4 |
| Daemon cold start | ms | 404 | 114 |
| Daemon stop | ms | 282 | 105 |
| Daemon idle RSS | MB | 73.2 | 7.5 |
| Daemon RSS after burst | MB | 99.8 | 24.7 |
| Daemon RSS, 12 MB ledger, after calls | MB | 93.6 | 28.0 |
| Daemon idle CPU | % of one core | 0.12 | 0.15 |
| Daemon CPU per sent message (burst) | ms | 2.45 | 1.2 |
| Burst send throughput | msg/s | 33.4 | 430 |
| Burst ack throughput | msg/s | 35 | 218 |
| Sequential send+inbox+ack | per s | 1.58 | 9.54 |
| Latency send return to wait return | ms | 218 | 1.65 |
| Latency send start to wait return | ms | 402 | 11.5 |
| Dashboard CPU | % of one core | 2.96 (Rust dash 0.3.0); 9.54 (Node dash) | 2.23 |
| Dashboard RSS | MB | 2.35 (Rust dash 0.3.0); 267.5 (Node dash) | 3.45 |

## Variance and caveats

* A shared, busy VM: the load average during runs was 0.9 to 13. Per-round medians are in `results-summary.tsv`; the spread between rounds is small for per-call times and sizes (a few percent), larger for throughput. The one clearly disturbed cell is Rust round 1 of the burst (130 msg/s and 1,541 ms, against 430 to 467 msg/s in rounds 2 and 3, during a load spike); the table keeps the median of three and states the range.
* p95 is taken over pooled samples of 60 (calls), 15 (cold start and stop), 30 (latency) and 3 (per-round figures such as idle CPU and RSS, where the p95 equals roughly the maximum). Idle CPU has a 10 ms tick, so 0.12 against 0.15% is a difference of one or two ticks over 60 s and is not a result.
* Throughput and latency figures include the CLI process cost on both sides; that is what an agent experiences, and it is most of the Node number. The daemon-side comparison is the CPU time per message (2.45 against 1.2 ms), a smaller gap.
* The 0.3.0 standalone has Node embedded; the same code started with the system `node` (the `npm` rows) is about 22% slower per call and about 7% larger in RSS. No claim is made for a Node other than v24.6.0.
* Both sides' ledgers were created by the Node 0.3.0 core. The Rust daemon migrated to a newer schema (v37), so Rust 0.4.0 is measured on its own schema and Node 0.3.0 on v36; the data is the same.
* The Rust figures are for the musl release binary as published, not a local build.

## Not measured

* A daemon with Herdr launching real agent panes, real model traffic, or a live ledger (66.7 MB as in `rust-port-estimate.md`); only generated ledgers and the 12 MB size were used.
* Server-side handler time alone (separate from the client); only the daemon's CPU time over a burst.
* macOS, arm64 (the arm64 assets exist and were not downloaded), a cold file cache, a long soak (RSS growth over hours), and install time (`npm ci` took 6 s with a warm cache; the Rust and standalone installs are a file copy).
* The dashboard with working agents or at the default refresh interval (see `rust-dash-performance.md`); `cstan-front-0.3.0`.
* Correctness or features: the two releases are not the same program in every respect (0.4.0 is Rust-only and removed the Node implementation).

## How to rerun

All scripts are in [node-vs-rust-benchmark/](node-vs-rust-benchmark/).

```sh
W=/tmp/capstan-cutover-bench; mkdir -p $W/dl3 $W/dl4 $W/npm3
( cd $W/dl3 && gh release download v0.3.0 -p 'cstan-0.3.0-linux-x64' -p 'cstan-dash-0.3.0-linux-x64' -p 'capstan-controller-0.3.0.tgz' -p SHA256SUMS && sha256sum -c --ignore-missing SHA256SUMS )
( cd $W/dl4 && gh release download v0.4.0 -p 'cstan-0.4.0-linux-x64' -p 'cstan-dash-0.4.0-linux-x64' -p SHA256SUMS && sha256sum -c --ignore-missing SHA256SUMS )
chmod +x $W/dl3/cstan-* $W/dl4/cstan-*
( cd $W/npm3 && tar xzf ../dl3/capstan-controller-0.3.0.tgz && cd package && npm ci --omit=dev --ignore-scripts )
mkdir -p $W/genroot/scripts && cp scripts/shadow-daemon.mjs scripts/copy-ledger.mjs $W/genroot/scripts/ && ln -s $W/npm3/package/dist $W/genroot/dist
# needs node 24.6, python3, git, /usr/bin/time; about 15 minutes per round for the three sides
bash docs/research/node-vs-rust-benchmark/bench.sh $W 3 node rust npm     # RUNS=20 IDLE_SECONDS=60 DASH_SECONDS=30 are the defaults
node docs/research/node-vs-rust-benchmark/aggregate.mjs $W/results.tsv > results-summary.tsv
```

`bench.sh` refuses a work directory outside `/tmp/capstan-cutover-`, builds every project fresh and removes the per-side projects when
a round ends. Delete `$W` when done (downloads about 143 MB, the npm install 28 MB).

Files: `bench.sh` (the whole suite), `seed.mjs` (registers a PM and 10 developers through the 0.3.0 core), `dash-sample.py`
(dashboard in a pty with CPU and RSS from `/proc`), `aggregate.mjs` (median, p95, min, max and per-round medians),
`results.tsv` and `results-summary.tsv` (the data behind this report). The `cutover-baseline/` scripts are the earlier harness this one extends.
