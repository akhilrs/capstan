# rust/testdata

Test data the Rust tests and `scripts/smoke-binary.sh` read. The files that came from the Node tree are **frozen copies**:
they were copied (not moved) from `test/`, `src/` and `migrations/`-adjacent data when the Node tree was about to go, and
nothing regenerates them. Consumers find them relative to `CARGO_MANIFEST_DIR` (crates) or the repository root (the script).

| Path | Copied from | Read by |
|---|---|---|
| `fixtures/prompts-off/` | `test/fixtures/prompts-off/` | `config/tests/prompts_parity.rs` |
| `fixtures/plan-bodies/`, `fixtures/kernel-text/` | same names under `test/fixtures/` | `kernel/tests` (`fixture()`) |
| `fixtures/*.ansi`, `*.txt`, `fixtures/prompts/`, `ps-macos.txt` | `test/fixtures/` (only the files `herdr/tests/parity/screen.json` names) | `herdr/tests/parity.rs` |
| `fixtures/fake-claude.py`, `fixtures/claude-trust-dialog.txt` | `test/fixtures/` | the live suites (`herdr/tests/common/live_env.rs`) |
| `golden/legacy-status-inspect.json` | `test/golden/` | `kernel/src/areas/status.rs` tests |
| `node/daemon.ts` | `src/daemon.ts` | `daemon/src/handlers/mod.rs` (`ROUTES` test) |
| `daemon-api-coverage.txt` | the `api-coverage` block of `test/daemon-scenarios/README.md` | `daemon/src/deps.rs` test |
| `ledger/ledger-better-sqlite3.sqlite` | `test/fixtures/ledger-better-sqlite3.sqlite` (schema v31, made by Node) | `ledger/tests/compat.rs`, `smoke-binary.sh` |

The reference for behaviour that used to be compared with a live Node daemon is the frozen corpus: the parity transcripts
under the crates' `tests/`, `cstan/tests/support/node-usage.txt` and the `divergences/` overlays.

## `ledger/ledger-v0.4.1.sqlite`

A ledger written by the **released** `cstan` v0.4.1 (not hand-written SQL, not a build of this tree). It holds a project,
a PM, an architect and a developer (three agents), a plan with one package (`fixture-package`), messages and one report.
`ledger/tests/compat.rs` opens a copy with the current build, runs the migrations and compares a sorted row dump of every
table (over the columns it had) before and after; `smoke-binary.sh` serves a copy with a daemon and runs `cstan status`.

**Binary used:** the published release asset `cstan-0.4.1-linux-x64` of tag v0.4.1
(`gh release download v0.4.1 -p cstan-0.4.1-linux-x64 -p SHA256SUMS`), sha256
`d537326092d8161cd88b7430b7d4a1bb290b1e3b0b219f3eee8eb460f7a0f130` (equal to its line in the release's `SHA256SUMS`).

**How it was made.** Everything ran in a scratch project under `/tmp/cg` with a scrubbed environment (`env -i`), its own HOME
and its own isolated Herdr session (`herdr --session capstan-gen server`); no agent token was read by hand. The agents were
launched by the v0.4.1 daemon (`cstan start`, `cstan spawn`), with a copy of `fixtures/fake-claude.py` as `claude` on PATH. The
copy was patched so each agent, while running, executes a script file named after its working directory
(`/tmp/cg/scripts/<worktree dir>.sh`) with `bash`, in its own environment, which already holds its `CAPSTAN_TOKEN`:

```python
# added to the copy of fake-claude.py, before main(), and started as a daemon thread at the top of main()
def run_scripts():
    mine = os.path.join("/tmp/cg/scripts", os.path.basename(os.getcwd()) + ".sh")
    while True:
        if os.path.exists(mine):
            os.rename(mine, mine + ".running")
            with open(mine + ".out", "w") as handle:
                subprocess.run(["bash", mine + ".running"], stdout=handle, stderr=subprocess.STDOUT)
            os.rename(mine + ".running", mine + ".done")
        time.sleep(0.5)
```

Steps (`cstan` is the v0.4.1 binary; the operator commands run in `/tmp/cg/repo`, the project, with `CAPSTAN_LAUNCH` unset):

1. `git init -b main` plus an empty commit; `cstan init`; replace `capstan.toml` by one with `herdr_session = "capstan-gen"`,
   `[nexora] track = "never"`, `[architect] enabled = true role = "architect"`, hosts `claude` and roles `pm`, `architect`
   and `developer` (the latter two of kind Developer); `cstan daemon &`; `cstan start`.
2. Each agent pane shows the trust dialog; it was answered with `herdr --session capstan-gen pane send-keys <pane> Down` and `Enter`.
3. `cstan plan open normal "Fixture plan"`; `cstan spawn architect --title "Fixture architect"`;
   `cstan spawn developer --title "Fixture developer"`.
4. The architect's script (run by the fake claude): `cstan plan submit plan-1 '{"summary":"Fixture plan","packages":[{"id":"fixture-package","title":"Fixture package","owns":["fixture/"],"acceptance":["the fixture exists"],"estimate_hours":1,"type":"chore"}]}'`. The normal plan is approved at once.
5. `cstan plan assign plan-1 fixture-package developer-1` (the controller sends the package text to the developer).
6. The developer's scripts: commit `fixture/file.txt` in its worktree (`chore: add the fixture file`), `cstan send @pm "fixture message from the developer"`;
   then `cstan ack <the assignment message id>` and `cstan report "$(git rev-parse HEAD)" "fixture report"` (the first report was
   refused with `unread_messages` until the assignment message was acknowledged, as the rules say).
7. `cstan stop`, `herdr --session capstan-gen server stop`, then the ledger was copied into one file with the SQLite backup API
   (`sqlite3.connect(controller.sqlite).backup(...)`, WAL mode kept).

Credentials in the file are SHA-256 hashes; the paths in it point into `/tmp/cg`.
