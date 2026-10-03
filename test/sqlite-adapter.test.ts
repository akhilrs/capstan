import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { test } from "node:test";
import { openDatabase } from "../src/controller/database.js";
import { openSqlite } from "../src/controller/sqlite.js";

function withDirectory<T>(
  run: (directory: string) => T | Promise<T>,
): Promise<T> {
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), "capstan-sqlite-"));
  return Promise.resolve(run(directory)).finally(() =>
    fs.rmSync(directory, { recursive: true, force: true }),
  );
}

function memory() {
  const db = openSqlite(":memory:");
  db.exec("CREATE TABLE t(id INTEGER PRIMARY KEY, v TEXT)");
  return db;
}

const count = (db: ReturnType<typeof memory>) =>
  (db.prepare("SELECT COUNT(*) AS n FROM t").get() as { n: number }).n;

test("a transaction commits and returns the function's result", () => {
  const db = memory();
  const insert = db.prepare("INSERT INTO t(v) VALUES (?)");
  const result = db.transaction((a: string, b: string) => {
    insert.run(a);
    insert.run(b);
    return "done";
  })("x", "y");
  assert.equal(result, "done");
  assert.equal(count(db), 2);
  db.close();
});

test("a transaction rolls back and rethrows when the function throws", () => {
  const db = memory();
  assert.throws(
    db.transaction(() => {
      db.prepare("INSERT INTO t(v) VALUES ('a')").run();
      throw new Error("boom");
    }),
    /boom/,
  );
  assert.equal(count(db), 0);
  db.close();
});

test("a nested transaction is a savepoint: its rollback keeps the outer work", () => {
  const db = memory();
  const insert = db.prepare("INSERT INTO t(v) VALUES (?)");
  db.transaction(() => {
    insert.run("outer");
    assert.throws(
      db.transaction(() => {
        insert.run("inner");
        throw new Error("inner failed");
      }),
      /inner failed/,
    );
    db.transaction(() => insert.run("inner-ok"))();
  })();
  const values = (
    db.prepare("SELECT v FROM t ORDER BY id").all() as { v: string }[]
  ).map((r) => r.v);
  assert.deepEqual(values, ["outer", "inner-ok"]);
  db.close();
});

test("run reports changes and lastInsertRowid as numbers", () => {
  const db = memory();
  const first = db.prepare("INSERT INTO t(v) VALUES (?)").run("a");
  assert.deepEqual(first, { changes: 1, lastInsertRowid: 1 });
  db.prepare("INSERT INTO t(v) VALUES (?)").run("b");
  const update = db.prepare("UPDATE t SET v = 'z'").run();
  assert.equal(update.changes, 2);
  assert.equal(db.prepare("DELETE FROM t WHERE id = ?").run(99).changes, 0);
  db.close();
});

test("rows are plain objects, and a missing row is undefined", () => {
  const db = memory();
  db.prepare("INSERT INTO t(v) VALUES (?)").run("a");
  const row = db.prepare("SELECT id, v FROM t").get();
  assert.equal(Object.getPrototypeOf(row), Object.prototype);
  assert.deepStrictEqual(row, { id: 1, v: "a" });
  assert.ok(Object.hasOwn(row as object, "v"));
  const rows = db.prepare("SELECT id, v FROM t").all();
  assert.deepStrictEqual(rows, [{ id: 1, v: "a" }]);
  assert.equal(Object.getPrototypeOf(rows[0]), Object.prototype);
  assert.equal(db.prepare("SELECT * FROM t WHERE id = 9").get(), undefined);
  db.close();
});

test("a read-only open of a missing file throws", async () => {
  await withDirectory((directory) => {
    assert.throws(() =>
      openSqlite(path.join(directory, "nope.sqlite"), { readOnly: true }),
    );
    assert.equal(fs.existsSync(path.join(directory, "nope.sqlite")), false);
  });
});

test("openDatabase sets journal_mode=wal, busy_timeout=5000, foreign keys and synchronous=FULL, and close ends the handle", async () => {
  await withDirectory(async (directory) => {
    const db = await openDatabase(path.join(directory, "controller.sqlite"));
    assert.deepEqual(db.pragma("journal_mode"), [{ journal_mode: "wal" }]);
    assert.deepEqual(db.pragma("busy_timeout"), [{ timeout: 5000 }]);
    assert.deepEqual(db.pragma("foreign_keys"), [{ foreign_keys: 1 }]);
    assert.deepEqual(db.pragma("synchronous"), [{ synchronous: 2 }]);
    assert.equal(db.open, true);
    db.close();
    assert.equal(db.open, false);
    db.close();
    assert.throws(() => db.prepare("SELECT 1"));
  });
});

test("backup copies a database to a new file", async () => {
  await withDirectory(async (directory) => {
    const db = openSqlite(path.join(directory, "a.sqlite"));
    db.exec("CREATE TABLE t(v TEXT); INSERT INTO t VALUES ('kept')");
    await db.backup(path.join(directory, "b.sqlite"));
    db.close();
    const copy = openSqlite(path.join(directory, "b.sqlite"), {
      readOnly: true,
    });
    assert.deepStrictEqual(copy.prepare("SELECT v FROM t").all(), [
      { v: "kept" },
    ]);
    copy.close();
  });
});
