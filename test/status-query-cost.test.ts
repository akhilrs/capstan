import assert from "node:assert/strict";
import path from "node:path";
import { test } from "node:test";
import { ControllerCore } from "../src/controller/core.js";
import { openSqlite, type Database } from "../src/controller/sqlite.js";
import { cleanup, fixture } from "./controller-harness.js";

const SEATS = 450;
const ROLES_SQL = `
      SELECT s.role, s.seat_id, s.state AS seat_state,
        EXISTS(SELECT 1 FROM actors a WHERE a.project_id = s.project_id
          AND a.seat_id = s.seat_id AND a.active = 1 AND a.revoked_at IS NULL) AS actor_active,
        (SELECT a.assignment_id FROM assignments a WHERE a.project_id = s.project_id
          AND a.seat_id = s.seat_id AND a.authority_state IN ('active', 'unknown')
          ORDER BY a.created_at DESC LIMIT 1) AS assignment_id
      FROM seats s WHERE s.project_id = ? ORDER BY s.role, s.seat_id`;

function seed(db: Database, projectId: string): void {
  const now = "2026-01-01T00:00:00.000Z";
  db.pragma("foreign_keys = OFF");
  db.transaction(() => {
    db.prepare(
      "INSERT INTO role_definitions VALUES (?, 'developer', 'Developer', 'claude', ?, 'active', ?, ?)",
    ).run(projectId, "c".repeat(64), now, now);
    for (let i = 0; i < SEATS; i++) {
      const seat = `bulk-seat-${i}`;
      db.prepare(
        "INSERT INTO seats VALUES (?, ?, ?, 'Developer', ?, 0, ?)",
      ).run(
        projectId,
        seat,
        `bulk ${i}`,
        i % 7 === 0 ? "disabled" : "active",
        now,
      );
      db.prepare(
        "INSERT INTO actors VALUES (?, ?, ?, 'Developer', ?, ?, ?, 0, ?, ?)",
      ).run(
        `bulk-actor-${i}`,
        projectId,
        `bulk ${i}`,
        seat,
        String(i).padStart(64, "0"),
        i % 5 === 0 ? 0 : 1,
        now,
        i % 5 === 0 ? now : null,
      );
      db.prepare(
        "INSERT INTO agents VALUES (?, ?, 'developer', 'Developer', ?, ?, 1, ?, ?, ?, NULL)",
      ).run(
        projectId,
        `bulk-agent-${i}`,
        seat,
        `bulk-actor-${i}`,
        i % 3 === 0 ? "ended" : "active",
        now,
        now,
      );
      if (i % 2 === 0)
        db.prepare(
          `INSERT INTO assignments (project_id, assignment_id, work_item_id, seat_id, state,
            state_version, input_revision, active_generation, authority_state, created_by, created_at)
          VALUES (?, ?, ?, ?, 'running', 0, 1, 1, ?, 'owner', ?)`,
        ).run(
          projectId,
          `bulk-assignment-${i}`,
          `bulk-work-${i}`,
          seat,
          ["active", "unknown", "revoked", "contained"][i % 4],
          now,
        );
    }
  })();
}

function rawLedger<T>(directory: string, use: (db: Database) => T): T {
  const db = openSqlite(path.join(directory, "controller.sqlite"));
  try {
    return use(db);
  } finally {
    db.close();
  }
}

test("the roles query uses both new indexes and status and listAgents are unchanged by them", async () => {
  const f = await fixture();
  const directory = f.stateDirectory;
  let core = f.core;
  try {
    core.close();
    rawLedger(directory, (db) => seed(db, f.project.projectId));
    const open = () =>
      ControllerCore.open({ stateDirectory: directory, project: f.project });

    core = await open();
    const withIndexes = {
      status: core.statusSnapshot(),
      agents: core.listAgents(),
    };
    assert.ok(withIndexes.agents.length >= SEATS);
    assert.ok(withIndexes.status.roles.length >= SEATS);
    assert.deepEqual(
      withIndexes.agents.map((a) => a.agentId),
      [...withIndexes.agents.map((a) => a.agentId)].sort(),
    );
    core.close();

    rawLedger(directory, (db) => {
      const plan = (
        db
          .prepare(`EXPLAIN QUERY PLAN ${ROLES_SQL}`)
          .all(f.project.projectId) as { detail: string }[]
      )
        .map((row) => row.detail)
        .join("\n");
      assert.match(plan, /actors_by_seat_active/);
      assert.match(plan, /assignments_by_seat_authority/);
      db.exec("DROP INDEX actors_by_seat_active");
      db.exec("DROP INDEX assignments_by_seat_authority");
      // A different text, so the statement cache cannot hand back the plan compiled with the indexes.
      const without = (
        db
          .prepare(`EXPLAIN QUERY PLAN ${ROLES_SQL} /* no indexes */`)
          .all(f.project.projectId) as { detail: string }[]
      )
        .map((row) => row.detail)
        .join("\n");
      assert.doesNotMatch(without, /actors_by_seat_active/);
    });

    core = await open();
    assert.deepEqual(core.statusSnapshot(), withIndexes.status);
    assert.deepEqual(core.listAgents(), withIndexes.agents);
  } finally {
    core.close();
    cleanup({ ...f, core });
  }
});

test("prepare reuses a cached statement, which still returns correct rows after DDL", () => {
  const db = openSqlite(":memory:");
  db.exec("CREATE TABLE t(id INTEGER PRIMARY KEY, v TEXT)");
  db.prepare("INSERT INTO t(v) VALUES (?)").run("a");
  const first = db.prepare("SELECT v FROM t ORDER BY id");
  assert.equal(db.prepare("SELECT v FROM t ORDER BY id"), first);
  assert.deepEqual(first.all(), [{ v: "a" }]);
  db.prepare("INSERT INTO t(v) VALUES (?)").run("b");
  assert.deepEqual(first.all(), [{ v: "a" }, { v: "b" }]);
  db.exec("CREATE INDEX t_by_v ON t(v); ALTER TABLE t ADD COLUMN w TEXT");
  assert.deepEqual(first.all(), [{ v: "a" }, { v: "b" }]);
  const star = db.prepare("SELECT * FROM t WHERE v = ?");
  assert.deepEqual(star.all("a"), [{ id: 1, v: "a", w: null }]);
  db.exec("DROP INDEX t_by_v");
  assert.deepEqual(star.all("b"), [{ id: 2, v: "b", w: null }]);
  db.close();
});

test("the statement cache holds at most 256 texts and is cleared on close", () => {
  const db = openSqlite(":memory:");
  const first = db.prepare("SELECT 0 AS n");
  for (let i = 1; i <= 256; i++) db.prepare(`SELECT ${i} AS n`);
  assert.notEqual(db.prepare("SELECT 0 AS n"), first, "oldest text evicted");
  assert.equal(db.prepare("SELECT 256 AS n"), db.prepare("SELECT 256 AS n"));
  const kept = db.prepare("SELECT 256 AS n");
  db.close();
  assert.throws(() => kept.all());
  assert.throws(() => db.prepare("SELECT 1"));
});
