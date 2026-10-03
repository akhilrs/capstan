import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { test } from "node:test";
import { openDatabase } from "../src/controller/database.js";
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
    assert.deepEqual(
      listing(directory).filter((n) => n.includes(".pre-v")),
      [],
      `${label}: no migration ran, so no backup was written`,
    );
    const after = header(target);
    // Bytes 16..27: page size, file-format versions, reserved space and payload fractions; 60: user_version.
    assert.deepEqual(after.subarray(16, 28), before.subarray(16, 28));
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
  "a copy of the live ledger opens without migrating",
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
