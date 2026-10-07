import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { test } from "node:test";
import { pathToFileURL } from "node:url";
import { openSqlite } from "../src/controller/sqlite.js";

// MERGE GATE for plan-23/daemon-profile. The generated large ledger (test/fixtures/daemon-cost) has the shape of a
// long-running live one: 500 agents, a PM with 1046 messages, 23 plans, 200 reports, 287 reviews, 20000 events.
// It checks that status, inbox, inspect and the background loops return what the code before the fixes returned
// (golden.json, captured with git 6881413) and that status and each loop stay within a statement and row budget.
// CAPSTAN_DAEMON_COST_DIST names another build to run the same checks against; the budgets fail on the old one.
const root = path.resolve(import.meta.dirname, "..", "..");
const fixtureDir = path.join(root, "test", "fixtures", "daemon-cost");
const distDir =
  process.env.CAPSTAN_DAEMON_COST_DIST ??
  path.resolve(import.meta.dirname, "..");

interface Counters {
  statements: number;
  rows: number;
}
interface Built {
  stateDirectory: string;
  workspaceRoot: string;
  credential: string;
  timers: Record<string, number>;
}
type Core = { close(): void } & Record<string, unknown>;
interface Served {
  call(command: string, args?: string[]): Promise<{ ok: boolean }>;
  close(): Promise<void>;
}
interface Capture {
  FIXED_NOW: number;
  withCounters<T>(
    dist: string,
    fn: () => T | Promise<T>,
  ): Promise<{ value: T; counters: Counters }>;
  openFor(dist: string, built: Built): Promise<Core>;
  serve(dist: string, core: Core, built: Built): Promise<Served>;
  capture(
    dist: string,
    core: Core,
    served: Served,
    built: Built,
  ): Promise<unknown>;
  normalise(value: unknown, replacements: Record<string, string>): unknown;
  relayPass(core: Core, credential: string): number;
  driverPass(
    dist: string,
    core: Core,
    credential: string,
    timers: Record<string, number>,
    pm: string,
  ): Promise<void>;
}

async function load<T>(file: string): Promise<T> {
  return (await import(pathToFileURL(path.join(fixtureDir, file)).href)) as T;
}

test("status, inbox, inspect and the background loops return what the code before the fixes returned, within the cost budget", async () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "capstan-daemon-cost-"));
  const { buildLedger } = await load<{
    buildLedger(dist: string, dir: string): Promise<Built>;
  }>("ledger.mjs");
  const lib = await load<Capture>("capture.mjs");
  const built = await buildLedger(distDir, dir);
  const core = await lib.openFor(distDir, built);
  const served = await lib.serve(distDir, core, built);
  try {
    const golden = JSON.parse(
      fs.readFileSync(path.join(fixtureDir, "golden.json"), "utf8"),
    ) as unknown;
    // The unmeasured pass first: it also warms the statement cache the way a running daemon has it.
    const result = lib.normalise(
      await lib.capture(distDir, core, served, built),
      { [dir]: "<dir>" },
    );
    assert.deepEqual(result, golden, "results match the pre-fix goldens");

    const measure = async <T>(
      label: string,
      fn: () => T | Promise<T>,
    ): Promise<{ counters: Counters; ms: number }> => {
      const started = performance.now();
      const { counters } = await lib.withCounters(distDir, fn);
      const ms = performance.now() - started;
      process.stderr.write(
        `daemon-cost ${label}: ${counters.statements} statements, ${counters.rows} rows, ${ms.toFixed(1)} ms\n`,
      );
      return { counters, ms };
    };

    // status: before the fixes 203 statements and 2695 rows, ~150 ms on a quiet machine.
    const status = await measure("status", () => served.call("status"));
    assert.ok(status.counters.statements <= 260, "status statement budget");
    assert.ok(status.counters.rows <= 1600, "status row budget");
    assert.ok(status.ms < 1500, "status time bound (generous)");

    // report relay, steady state: before 56 statements and 662 rows. Two of the plan notices are missing in this
    // ledger on purpose (it is what the goldens announce), so two body lookups remain.
    lib.relayPass(core, built.credential);
    const relay = await measure("report relay tick", () =>
      lib.relayPass(core, built.credential),
    );
    assert.ok(relay.counters.statements <= 16, "relay statement budget");
    assert.ok(relay.counters.rows <= 300, "relay row budget");
    assert.ok(relay.ms < 1500, "relay time bound (generous)");

    // driver tick, the ledger part: before 47 statements and 3329 rows.
    const driver = await measure("driver tick", () =>
      lib.driverPass(distDir, core, built.credential, built.timers, "pm-1"),
    );
    assert.ok(driver.counters.statements <= 80, "driver statement budget");
    assert.ok(driver.counters.rows <= 400, "driver row budget");
    assert.ok(driver.ms < 1500, "driver time bound (generous)");
  } finally {
    await served.close();
    core.close();
    fs.rmSync(dir, { recursive: true, force: true });
  }
});

test("the four queries behind migration 0036 are planned on its indexes", async () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "capstan-daemon-cost-"));
  const { buildLedger } = await load<{
    buildLedger(dist: string, dir: string): Promise<Built>;
  }>("ledger.mjs");
  const built = await buildLedger(distDir, dir);
  const db = openSqlite(path.join(built.stateDirectory, "controller.sqlite"));
  try {
    const plan = (sql: string): string =>
      (
        db.prepare(`EXPLAIN QUERY PLAN ${sql}`).all("p") as {
          detail: string;
        }[]
      )
        .map((row) => row.detail)
        .join("\n");
    assert.match(
      plan(
        `SELECT * FROM messages INDEXED BY messages_by_state WHERE project_id = ? AND recipient_agent_id = 'pm-1' AND state IN ('queued', 'deferred', 'sent', 'unacked', 'expired', 'failed') ORDER BY sequence`,
      ),
      /USING INDEX messages_by_state \(project_id=\? AND state=\? AND recipient_agent_id=\?\)/,
    );
    // The exact text of the plan-package query: before the index SQLite walked agent_reports newest first.
    assert.match(
      plan(
        `SELECT r.report_id FROM agent_reports r JOIN agents a ON a.project_id = r.project_id AND a.agent_id = r.agent_id AND a.generation = r.generation
         WHERE r.project_id = ? AND r.agent_id = 'developer-2' AND r.state = 'accepted' AND r.created_at >= '2026-01-01' ORDER BY r.sequence DESC LIMIT 1`,
      ),
      /agent_reports_by_agent_state/,
    );
    assert.match(
      plan(
        `SELECT state FROM reviews WHERE project_id = ? AND subject_report_id = 'report-1' AND state IN ('passed', 'findings') ORDER BY sequence DESC LIMIT 1`,
      ),
      /reviews_by_subject_report/,
    );
    assert.match(
      plan(
        `SELECT MAX(sequence) FROM controller_events WHERE project_id = ? AND entity_type = 'run_control' AND to_state = 'degraded'`,
      ),
      /controller_events_by_entity_state/,
    );
    assert.deepEqual(db.pragma("integrity_check"), [{ integrity_check: "ok" }]);
  } finally {
    db.close();
    fs.rmSync(dir, { recursive: true, force: true });
  }
});
