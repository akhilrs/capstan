import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { test } from "node:test";
import {
  maxEmbeddedMigration,
  openDatabase,
} from "../src/controller/database.js";
import { packageNaming, packageOfBody, parsePlanBody } from "../src/plans.js";
import { openSqlite } from "../src/controller/sqlite.js";

const root = path.resolve(import.meta.dirname, "..", "..");
const fixture = path.join(
  root,
  "test",
  "fixtures",
  "ledger-better-sqlite3.sqlite",
);
// The live ledger of the main checkout, when this machine has one. Only ever copied, never opened.
const live = path.join(
  process.env.CAPSTAN_LIVE_LEDGER_ROOT ??
    "/home/akhil/Workspace/github.com/akhilrs/capstan",
  ".capstan",
  "state",
  "controller.sqlite",
);

function header(file: string): Buffer {
  const fd = fs.openSync(file, "r");
  try {
    const buffer = Buffer.alloc(100);
    fs.readSync(fd, buffer, 0, 100, 0);
    return buffer;
  } finally {
    fs.closeSync(fd);
  }
}

function listing(directory: string): string[] {
  return fs.readdirSync(directory).sort();
}

async function checkOpens(
  source: string,
  label: string,
  skip?: (reason: string) => void,
): Promise<void> {
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), "capstan-compat-"));
  try {
    const target = path.join(directory, "controller.sqlite");
    try {
      fs.copyFileSync(source, target);
      for (const suffix of ["-wal"])
        if (fs.existsSync(source + suffix))
          fs.copyFileSync(source + suffix, target + suffix);
    } catch (error) {
      // The live daemon may be mid-write; the committed fixture is the required gate.
      if (skip === undefined) throw error;
      skip(`could not copy the live ledger: ${String(error)}`);
      return;
    }
    const before = header(target);
    const pending = openSqlite(target, { readOnly: true });
    const behind = (
      pending
        .prepare("SELECT MAX(version) AS v FROM schema_migrations")
        .get() as {
        v: number;
      }
    ).v;
    pending.close();
    const pendingVersions = Array.from(
      { length: Math.max(0, maxEmbeddedMigration() - behind) },
      (_, index) => String(behind + 1 + index),
    );
    const migrated = pendingVersions.length > 0;
    const db = await openDatabase(target);
    try {
      const ledger = db
        .prepare(
          "SELECT version, name, checksum FROM schema_migrations ORDER BY version",
        )
        .all() as { version: number; name: string; checksum: string }[];
      assert.ok(ledger.length >= 31, `${label}: ledger has every migration`);
      for (const [index, row] of ledger.entries()) {
        assert.equal(row.version, index + 1);
        const bytes = fs.readFileSync(path.join(root, "migrations", row.name));
        assert.equal(
          createHash("sha256").update(bytes).digest("hex"),
          row.checksum,
          `${label}: ${row.name} checksum verifies`,
        );
      }
      assert.deepEqual(db.pragma("foreign_key_check"), []);
      assert.deepEqual(db.pragma("integrity_check"), [
        { integrity_check: "ok" },
      ]);
      assert.deepEqual(db.pragma("journal_mode"), [{ journal_mode: "wal" }]);
    } finally {
      db.close();
    }
    // The ledger may be behind this build; each pending migration writes one backup.
    assert.deepEqual(
      listing(directory)
        .filter((n) => n.includes(".pre-v"))
        .map((n) => /\.pre-v(\d+)-/.exec(n)![1]),
      pendingVersions,
      `${label}: only the pending migrations wrote a backup`,
    );
    const after = header(target);
    // Bytes 16..27: page size, file-format versions, reserved space and payload fractions; 60: user_version.
    // Bytes 24..27 are the file change counter, which a migration moves.
    assert.deepEqual(after.subarray(16, 24), before.subarray(16, 24));
    if (!migrated)
      assert.deepEqual(after.subarray(24, 28), before.subarray(24, 28));
    assert.deepEqual(after.subarray(60, 64), before.subarray(60, 64));
  } finally {
    fs.rmSync(directory, { recursive: true, force: true });
  }
}

test("a ledger written by better-sqlite3 opens, verifies and reads back field for field", async () => {
  await checkOpens(fixture, "fixture");
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), "capstan-compat-"));
  try {
    const target = path.join(directory, "controller.sqlite");
    fs.copyFileSync(fixture, target);
    const db = await openDatabase(target);
    try {
      const count = (table: string) =>
        (
          db.prepare(`SELECT COUNT(*) AS n FROM ${table}`).get() as {
            n: number;
          }
        ).n;
      assert.equal(count("projects"), 1);
      assert.equal(count("actors"), 4);
      assert.equal(count("messages"), 1);
      assert.equal(count("plans"), 1);
      assert.equal(count("agent_reports"), 1);
      const project = db.prepare("SELECT name FROM projects").get();
      assert.deepStrictEqual(project, { name: "Ledger fixture project" });
      const plan = db
        .prepare("SELECT plan_id, sequence, title, tier, state FROM plans")
        .get();
      assert.deepStrictEqual(plan, {
        plan_id: "plan-1",
        sequence: 1,
        title: "Fixture plan",
        tier: "normal",
        state: "draft",
      });
      const report = db
        .prepare("SELECT commit_sha, summary, state FROM agent_reports")
        .get();
      assert.deepStrictEqual(report, {
        commit_sha: "b".repeat(40),
        summary: "fixture report",
        state: "accepted",
      });
      const roles = (
        db.prepare("SELECT role FROM actors ORDER BY role, actor_id").all() as {
          role: string;
        }[]
      ).map((r) => r.role);
      assert.deepEqual([...roles].sort(), [
        "Developer",
        "PM",
        "controller",
        "operator",
      ]);
    } finally {
      db.close();
    }
  } finally {
    fs.rmSync(directory, { recursive: true, force: true });
  }
});

test(
  "a copy of the live ledger opens and migrates",
  { skip: !fs.existsSync(live) },
  async (t) => {
    await checkOpens(live, "live", (reason) => t.skip(reason));
  },
);

test("opening read-only reads the fixture copy", () => {
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), "capstan-compat-"));
  try {
    const target = path.join(directory, "controller.sqlite");
    fs.copyFileSync(fixture, target);
    const db = openSqlite(target, { readOnly: true });
    assert.equal(
      (
        db.prepare("SELECT COUNT(*) AS n FROM schema_migrations").get() as {
          n: number;
        }
      ).n,
      31,
    );
    db.close();
  } finally {
    fs.rmSync(directory, { recursive: true, force: true });
  }
});

test("a ledger with capstan/integration/<id> branches migrates to 0032 keeping every row, trigger and index", async () => {
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), "capstan-compat-"));
  try {
    const target = path.join(directory, "controller.sqlite");
    fs.copyFileSync(fixture, target);
    const old = openSqlite(target);
    const project = (
      old.prepare("SELECT project_id FROM projects").get() as {
        project_id: string;
      }
    ).project_id;
    const report = (
      old.prepare("SELECT report_id FROM agent_reports").get() as {
        report_id: string;
      }
    ).report_id;
    // A body as the controller stored it before the naming keys existed.
    const legacy = parsePlanBody(
      JSON.stringify({
        summary: "s",
        packages: [
          {
            id: "wp1",
            title: "One",
            owns: ["src/a"],
            estimate_hours: 1,
            acceptance: ["works"],
          },
        ],
      }),
      { maxPackages: 8 },
    );
    assert.ok(legacy.ok);
    const stored = JSON.stringify(legacy.plan);
    const legacyBody = stored
      .replace(',"type":null', "")
      .replace(',"scope":null', "")
      .replace(',"breaking":false', "");
    assert.ok(!legacyBody.includes('"type"'));
    const author = old
      .prepare("SELECT agent_id, actor_id FROM agents LIMIT 1")
      .get() as { agent_id: string; actor_id: string };
    old
      .prepare(
        `INSERT INTO plan_revisions(project_id, plan_id, revision, base_sha, body_json, body_sha, author_agent_id, author_actor_id, created_at)
         VALUES (?, 'plan-1', 1, ?, ?, ?, ?, ?, '2026-01-01T00:00:00Z')`,
      )
      .run(
        project,
        "a".repeat(40),
        legacyBody,
        createHash("sha256").update(legacyBody).digest("hex"),
        author.agent_id,
        author.actor_id,
      );
    old
      .prepare(
        `INSERT INTO integrations(project_id, integration_id, sequence, base_sha, branch, requested_by, state, head_sha, created_at, completed_at)
         VALUES (?, 'int-old', 1, ?, 'capstan/integration/int-old', 'operator', 'running', NULL, '2026-01-01T00:00:00Z', NULL)`,
      )
      .run(project, "a".repeat(40));
    old
      .prepare(
        "INSERT INTO integration_reports(project_id, integration_id, position, report_id) VALUES (?, 'int-old', 1, ?)",
      )
      .run(project, report);
    old
      .prepare(
        "UPDATE integrations SET state = 'merged', head_sha = ?, completed_at = '2026-01-01T00:00:01Z' WHERE integration_id = 'int-old'",
      )
      .run("c".repeat(40));
    old.close();
    const db = await openDatabase(target);
    try {
      assert.deepEqual(db.pragma("foreign_key_check"), []);
      // Stored plan bodies are never rewritten, and the readers take them without the naming keys.
      for (const row of db
        .prepare("SELECT body_json, body_sha FROM plan_revisions")
        .all() as { body_json: string; body_sha: string }[]) {
        assert.equal(row.body_json, legacyBody);
        assert.equal(
          createHash("sha256").update(row.body_json).digest("hex"),
          row.body_sha,
        );
        const ids = (
          JSON.parse(row.body_json) as { packages: { id: string }[] }
        ).packages.map((p) => p.id);
        for (const id of ids) {
          assert.equal(packageOfBody(row.body_json, id)?.title, "One");
          assert.deepEqual(packageNaming(row.body_json, id), {
            type: null,
            scope: null,
            breaking: false,
          });
        }
      }
      assert.deepEqual(
        db
          .prepare(
            "SELECT integration_id, branch, state, head_sha FROM integrations",
          )
          .all(),
        [
          {
            integration_id: "int-old",
            branch: "capstan/integration/int-old",
            state: "merged",
            head_sha: "c".repeat(40),
          },
        ],
      );
      assert.equal(
        (
          db.prepare("SELECT COUNT(*) AS n FROM integration_reports").get() as {
            n: number;
          }
        ).n,
        1,
      );
      const names = (
        db
          .prepare(
            "SELECT name FROM sqlite_master WHERE tbl_name = 'integrations' AND type IN ('trigger', 'index') AND name NOT LIKE 'sqlite_%' ORDER BY name",
          )
          .all() as { name: string }[]
      ).map((r) => r.name);
      assert.deepEqual(names, [
        "immutable_integrations_delete",
        "immutable_integrations_identity",
        "integrations_conflict_report_is_a_member",
        "integrations_move_forward",
        "one_running_integration",
      ]);
      assert.throws(
        () => db.prepare("DELETE FROM integrations").run(),
        /immutable/,
      );
      assert.throws(
        () =>
          db.prepare("UPDATE integrations SET branch = 'integration/x'").run(),
        /immutable/,
      );
      // A name that is not capstan/integration/<id> is now allowed; the branch stays unique.
      const insert = (id: string, seq: number, branch: string) =>
        db
          .prepare(
            `INSERT INTO integrations(project_id, integration_id, sequence, base_sha, branch, requested_by, state, created_at)
             SELECT project_id, ?, ?, ?, ?, 'operator', 'running', '2026-01-02T00:00:00Z' FROM projects`,
          )
          .run(id, seq, "a".repeat(40), branch);
      insert("int-new", 2, "integration/plan-1-x");
      assert.throws(
        () => insert("int-dup", 3, "integration/plan-1-x"),
        /UNIQUE|constraint/,
      );
      db.prepare(
        "UPDATE integrations SET state = 'confirmed' WHERE integration_id = 'int-old'",
      ).run();
    } finally {
      db.close();
    }
  } finally {
    fs.rmSync(directory, { recursive: true, force: true });
  }
});
