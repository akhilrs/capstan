import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { test } from "node:test";
import {
  openDatabase,
  pruneMigrationBackups,
} from "../src/controller/database.js";

function withDirectory(run: (directory: string) => void): void {
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), "capstan-backups-"));
  try {
    run(directory);
  } finally {
    fs.rmSync(directory, { recursive: true, force: true });
  }
}

const backup = (version: number, at: number) =>
  `controller.sqlite.pre-v${version}-${at}.sqlite`;

function touch(directory: string, names: readonly string[]): void {
  for (const name of names) fs.writeFileSync(path.join(directory, name), "x");
}

const listing = (directory: string) => fs.readdirSync(directory).sort();

test("prune keeps the newest K backups by schema version, then timestamp", () => {
  withDirectory((directory) => {
    const all = [
      backup(2, 500),
      backup(9, 100),
      backup(10, 50),
      backup(10, 60),
      backup(3, 900),
    ];
    touch(directory, all);
    const deleted = pruneMigrationBackups(
      path.join(directory, "controller.sqlite"),
      3,
    );
    assert.deepEqual(deleted.sort(), [backup(2, 500), backup(3, 900)].sort());
    assert.deepEqual(
      listing(directory),
      [backup(9, 100), backup(10, 50), backup(10, 60)].sort(),
    );
  });
});

test("prune never touches files that do not match the backup name exactly", () => {
  withDirectory((directory) => {
    const others = [
      "controller.sqlite",
      "controller.sqlite-wal",
      "controller.sqlite-shm",
      "controller.lock",
      "controller.sqlite.known-good",
      "controller.sqlite.restart",
      `${backup(1, 1)}-wal`,
      `${backup(1, 1)}.bak`,
      `x${backup(1, 1)}`,
      "controller.sqlite.pre-v1-abc.sqlite",
      "other.sqlite.pre-v1-1.sqlite",
    ];
    touch(directory, [...others, backup(1, 1), backup(2, 2), backup(3, 3)]);
    pruneMigrationBackups(path.join(directory, "controller.sqlite"), 1);
    assert.deepEqual(listing(directory), [...others, backup(3, 3)].sort());
  });
});

test("prune does nothing when there are K or fewer backups", () => {
  withDirectory((directory) => {
    touch(directory, [backup(1, 1), backup(2, 2), backup(3, 3)]);
    const databasePath = path.join(directory, "controller.sqlite");
    assert.deepEqual(pruneMigrationBackups(databasePath, 3), []);
    assert.deepEqual(pruneMigrationBackups(databasePath, 10), []);
    assert.equal(listing(directory).length, 3);
  });
});

test("a backup-named directory is left alone and the other deletions still happen", () => {
  withDirectory((directory) => {
    // Only regular files are deleted: a directory with a backup's name is left alone.
    const stuck = backup(1, 1);
    fs.mkdirSync(path.join(directory, stuck));
    touch(directory, [backup(2, 2), backup(3, 3), backup(4, 4)]);
    const deleted = pruneMigrationBackups(
      path.join(directory, "controller.sqlite"),
      1,
    );
    assert.deepEqual(deleted.sort(), [backup(2, 2), backup(3, 3)].sort());
    assert.ok(fs.existsSync(path.join(directory, stuck)));
    assert.deepEqual(listing(directory), [stuck, backup(4, 4)].sort());
  });
});

test("an unreadable directory is reported and does not throw", () => {
  const errors: string[] = [];
  const deleted = pruneMigrationBackups(
    path.join(os.tmpdir(), "capstan-no-such-directory", "controller.sqlite"),
    3,
    (message) => errors.push(message),
  );
  assert.deepEqual(deleted, []);
  assert.equal(errors.length, 1);
});

test("a failing unlink is logged, never thrown, and leaves the files", async () => {
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), "capstan-backups-"));
  const unlink = fs.unlinkSync;
  try {
    const databasePath = path.join(directory, "controller.sqlite");
    (await openDatabase(databasePath)).close();
    for (const name of listing(directory))
      if (name.includes(".pre-v")) fs.unlinkSync(path.join(directory, name));
    touch(directory, [backup(1, 1), backup(1, 2)]);
    const errors: string[] = [];
    fs.unlinkSync = () => {
      throw new Error("EPERM");
    };
    // Reopening an up-to-date ledger runs no migration, so prune directly with the failing unlink.
    const deleted = pruneMigrationBackups(databasePath, 1, (m) =>
      errors.push(m),
    );
    assert.deepEqual(deleted, []);
    assert.equal(errors.length, 1);
    assert.match(errors[0]!, /EPERM/);
  } finally {
    fs.unlinkSync = unlink;
    fs.rmSync(directory, { recursive: true, force: true });
  }
});

test("migrating prunes the backups it took down to the configured count", async () => {
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), "capstan-backups-"));
  try {
    const databasePath = path.join(directory, "controller.sqlite");
    // A fresh ledger takes one backup before every migration after the first.
    const database = await openDatabase(databasePath, {
      keepMigrationBackups: 2,
    });
    database.close();
    const backups = listing(directory).filter((n) => n.includes(".pre-v"));
    assert.equal(backups.length, 2);
    const versions = backups.map((n) => Number(/pre-v(\d+)-/.exec(n)![1]));
    assert.equal(Math.min(...versions) + 1, Math.max(...versions));
    assert.ok(fs.existsSync(databasePath));
  } finally {
    fs.rmSync(directory, { recursive: true, force: true });
  }
});
