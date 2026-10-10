# The soak gate

Rust became the only implementation at cutover (plan-30), but the frozen Node tree (`src/`, `test/`, `package.json`,
`.github/workflows/ci.yml`'s Node job) is still in the repository. It is deleted by **plan D**, and plan D does not open until
a release has run on a real project for the agreed time without a problem. This page defines that gate. The decision to open
plan D is the user's; nothing here lets an agent take it.

## What happens

1. **The user cuts and pushes a release.** The user, not an agent, runs `scripts/release.sh` (see the
   [Install reference](reference/install.md#maintainer-release-steps)), pushes the commit and the tag, and lets the Release
   workflow build and publish the binaries. Agents never push or tag.
2. **The user installs it on a real project** with `install.sh` and uses it for real work: `cstan start`, a PM, workers,
   reviews, integrations, an Operator restart (`cstan op propose --restart`) at least once, and `cstan dash`.
3. **The soak runs for a period the user approves.** Proposed: **7 days** of calendar time with the controller running on
   working days. The user may shorten or lengthen it; the period in force is the one the PM records.
4. **Samples are taken** (below) and the exit criteria are checked.
5. **Plan D opens only after the PM records the user's approval** (the project manager writes the user's words and the date in
   the plan record). Passing the criteria is necessary, not sufficient: without that recorded approval plan D stays closed.

## What to sample

Everything here is read-only. The live ledger is never opened by a sampler or by an agent.

| What | How | When |
| --- | --- | --- |
| Daemon errors | `grep -ci 'error\|failed\|panicked' .capstan/daemon.log`, then a read of every distinct matching line (the log is `.capstan/daemon.log` of the project, one JSON object per line with a `ts`; `config_warning` lines are the retired-`[daemon]` warning and are expected until the table is deleted) | at least daily |
| Restarts of the daemon | the pid in `.capstan/state/daemon.pid` and `cstan ping --json`; a changed pid that no `cstan stop`, `cstan start` or Operator restart explains is an unplanned restart | at least daily |
| RSS | `grep VmRSS /proc/<daemon pid>/status` (read-only `/proc`; nothing is sent to the daemon) | every hour while working, or a 60 s loop in a scratch `sh` |
| CPU | utime + stime (fields 14 and 15 of `/proc/<pid>/stat`) before and after a 60 s window, as a percent of one core (`CLK_TCK` is 100) | the same samples as RSS |
| Ledger integrity | copy the ledger **while the daemon is stopped** or with the SQLite backup API (`sqlite3 controller.sqlite ".backup /tmp/capstan-soak/copy.sqlite"`), then `sqlite3 /tmp/capstan-soak/copy.sqlite 'PRAGMA integrity_check; PRAGMA foreign_key_check;'` on the **copy** | start, middle and end of the soak, and after every Operator restart |
| Client commands | any `cstan` command that exits with 5 (the controller did not answer) or hangs | as it happens |
| Operator restart | the `result.json` of every restart (`.capstan/state/restart/<id>/result.json`) | after each restart |

Compare RSS and CPU with the idle numbers of `docs/research/rust-cutover-baseline.md`: the daemon is expected to stay within
the same order of magnitude on a ledger of the same size.

## Exit criteria

All of these hold at the end of the period:

1. **No crash and no unplanned restart** of the daemon, and no `panicked` line in `daemon.log`.
2. **No error line in `daemon.log` that is not explained** (an explained line is one with a known cause that the user accepts,
   written next to the sample).
3. **No growth without cause**: RSS at the end is within 25% of RSS after the first hour at a comparable ledger size, and
   idle CPU stays under 1% of a core.
4. **`PRAGMA integrity_check` returns `ok` and `PRAGMA foreign_key_check` returns no row** on a copy of the ledger at each of
   the sampling points.
5. **At least one Operator restart** ended `ok`, or ended `rolled_back` for a reason the user accepts; none ended `down`.
6. **No loss of work**: every report, review and integration the user made is in the ledger, and `cstan status` agrees with
   what the user did.
7. **No data-affecting defect found** in the period that is still open. A defect that is found and fixed restarts the period
   from the fix unless the user says otherwise.

## If a criterion fails

The soak stops, the PM writes what failed with the evidence (log lines, samples, the copy of the ledger), and the fix is made
on the Rust side. Plan D stays closed. The Node tree is untouched, so a project that needs it can install the release before
0.4.0 (see [Upgrading from 0.3](reference/install.md#upgrading-from-03)).
