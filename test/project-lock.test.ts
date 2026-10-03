import assert from "node:assert/strict";
import { spawn, spawnSync, type ChildProcess } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { after, test } from "node:test";
import { pathToFileURL } from "node:url";
import {
  ControllerOwnershipError,
  ProjectLock,
  ProjectLockHeldError,
} from "../src/controller/ownership.js";

const moduleUrl = pathToFileURL(
  path.resolve(import.meta.dirname, "../src/controller/ownership.js"),
).href;
const dirs: string[] = [];
const children: ChildProcess[] = [];

function tempDir(): string {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "project-lock-"));
  fs.chmodSync(dir, 0o700);
  dirs.push(dir);
  return dir;
}

after(() => {
  for (const child of children) child.kill("SIGKILL");
  for (const dir of dirs) fs.rmSync(dir, { recursive: true, force: true });
});

const childScript = `
import(${JSON.stringify(moduleUrl)}).then(({ ProjectLock, ProjectLockHeldError }) => {
  try {
    ProjectLock.acquire(process.argv[1]);
    process.stdout.write("held\\n");
    setInterval(() => undefined, 1000);
  } catch (error) {
    process.stdout.write((error instanceof ProjectLockHeldError ? "ProjectLockHeldError" : error.name) + "\\n");
    process.exit(0);
  }
});
`;

function runChild(lockPath: string): {
  child: ChildProcess;
  first: Promise<string>;
  stderr: () => string;
} {
  const child = spawn(
    process.execPath,
    ["--input-type=module", "-e", childScript, lockPath],
    { stdio: ["ignore", "pipe", "pipe"] },
  );
  children.push(child);
  let err = "";
  child.stderr!.on("data", (chunk: Buffer) => (err += chunk.toString()));
  const first = new Promise<string>((resolve, reject) => {
    child.stdout!.once("data", (chunk: Buffer) =>
      resolve(chunk.toString().trim()),
    );
    child.once("error", reject);
    child.once("exit", () => resolve("exited"));
  });
  return { child, first, stderr: () => err };
}

function exited(child: ChildProcess): Promise<void> {
  return new Promise((resolve) => {
    if (child.exitCode !== null || child.signalCode !== null) resolve();
    else child.once("exit", () => resolve());
  });
}

test("contention: in-process and child-process acquires are refused and the holder keeps the lock", async () => {
  const lockPath = path.join(tempDir(), "controller.lock");
  const holder = ProjectLock.acquire(lockPath);
  try {
    assert.throws(() => ProjectLock.acquire(lockPath), ProjectLockHeldError);
    holder.assertHeld();
    const { child, first } = runChild(lockPath);
    assert.equal(await first, "ProjectLockHeldError");
    await exited(child);
    holder.assertHeld();
    assert.throws(() => ProjectLock.acquire(lockPath), ProjectLockHeldError);
  } finally {
    holder.close();
  }
});

test("stale recovery: a SIGKILLed holder leaves no cleanup step", async () => {
  const lockPath = path.join(tempDir(), "controller.lock");
  const { child, first } = runChild(lockPath);
  assert.equal(await first, "held");
  assert.throws(() => ProjectLock.acquire(lockPath), ProjectLockHeldError);
  child.kill("SIGKILL");
  await exited(child);
  const lock = ProjectLock.acquire(lockPath);
  lock.assertHeld();
  lock.close();
});

test("a lock file replaced or removed fails assertHeld", () => {
  const dir = tempDir();
  const lockPath = path.join(dir, "controller.lock");
  const lock = ProjectLock.acquire(lockPath);
  try {
    const other = path.join(dir, "other");
    fs.writeFileSync(other, "", { mode: 0o600 });
    fs.renameSync(other, lockPath);
    assert.throws(
      () => lock.assertHeld(),
      /no longer identifies the owned inode/,
    );
    fs.unlinkSync(lockPath);
    assert.throws(() => lock.assertHeld(), /was removed/);
  } finally {
    lock.close();
  }
});

test("symlinked lock path, symlinked parent and a 0644 lock file are refused", () => {
  const dir = tempDir();
  const target = path.join(dir, "target");
  fs.writeFileSync(target, "", { mode: 0o600 });
  const linkPath = path.join(dir, "controller.lock");
  fs.symlinkSync(target, linkPath);
  assert.throws(() => ProjectLock.acquire(linkPath), ControllerOwnershipError);

  const realDir = path.join(dir, "real");
  fs.mkdirSync(realDir, { mode: 0o700 });
  const linkDir = path.join(dir, "linkdir");
  fs.symlinkSync(realDir, linkDir);
  assert.throws(
    () => ProjectLock.acquire(path.join(linkDir, "controller.lock")),
    ControllerOwnershipError,
  );

  const open = path.join(realDir, "controller.lock");
  fs.writeFileSync(open, "", { mode: 0o644 });
  fs.chmodSync(open, 0o644);
  assert.throws(() => ProjectLock.acquire(open), /private to the current user/);
});

test("a pre-existing zero-byte mode-600 lock file is acquired and stays mode 600", () => {
  const lockPath = path.join(tempDir(), "controller.lock");
  fs.writeFileSync(lockPath, "", { mode: 0o600 });
  fs.chmodSync(lockPath, 0o600);
  const lock = ProjectLock.acquire(lockPath);
  try {
    lock.assertHeld();
    assert.equal(fs.statSync(lockPath).mode & 0o777, 0o600);
  } finally {
    lock.close();
  }
});

test("a fresh lock file is created mode 600 with no side files", () => {
  const dir = tempDir();
  const lockPath = path.join(dir, "controller.lock");
  const lock = ProjectLock.acquire(lockPath);
  try {
    assert.equal(fs.statSync(lockPath).mode & 0o777, 0o600);
    assert.deepEqual(fs.readdirSync(dir), ["controller.lock"]);
  } finally {
    lock.close();
  }
});

test("a non-empty non-SQLite file is refused and left untouched", () => {
  const lockPath = path.join(tempDir(), "controller.lock");
  const content =
    "this is not a database, just some text padding it out\n".repeat(40);
  fs.writeFileSync(lockPath, content, { mode: 0o600 });
  assert.throws(
    () => ProjectLock.acquire(lockPath),
    (error: unknown) =>
      error instanceof ControllerOwnershipError &&
      /not a lock database: remove it with the daemon stopped/.test(
        error.message,
      ),
  );
  assert.equal(fs.readFileSync(lockPath, "utf8"), content);
});

test("after close another process can acquire", async () => {
  const lockPath = path.join(tempDir(), "controller.lock");
  const lock = ProjectLock.acquire(lockPath);
  lock.close();
  assert.throws(() => lock.assertHeld(), /released/);
  const { child, first } = runChild(lockPath);
  assert.equal(await first, "held");
  child.kill("SIGKILL");
  await exited(child);
  const again = ProjectLock.acquire(lockPath);
  again.close();
});

test("acquiring prints nothing on stderr", async () => {
  const lockPath = path.join(tempDir(), "controller.lock");
  const { child, first, stderr } = runChild(lockPath);
  assert.equal(await first, "held");
  child.kill("SIGKILL");
  await exited(child);
  assert.equal(stderr(), "");
  const result = spawnSync(
    process.execPath,
    [
      "--input-type=module",
      "-e",
      childScript.replace(
        "setInterval(() => undefined, 1000);",
        "process.exit(0);",
      ),
      lockPath,
    ],
    { encoding: "utf8" },
  );
  assert.equal(result.stderr, "");
});

test("a restrictive umask still yields a writable mode-600 lock file", () => {
  const lockPath = path.join(tempDir(), "controller.lock");
  const previous = process.umask(0o277);
  let lock: ProjectLock;
  try {
    lock = ProjectLock.acquire(lockPath);
  } finally {
    process.umask(previous);
  }
  try {
    assert.equal(fs.statSync(lockPath).mode & 0o777, 0o600);
  } finally {
    lock.close();
  }
});

test("a corrupted lock database is refused like a non-database", () => {
  const lockPath = path.join(tempDir(), "controller.lock");
  ProjectLock.acquire(lockPath).close();
  const fd = fs.openSync(lockPath, "r+");
  try {
    // Keep the SQLite header intact but wreck the schema page that follows it.
    fs.writeSync(fd, Buffer.alloc(400, 0xff), 0, 400, 100);
  } finally {
    fs.closeSync(fd);
  }
  assert.throws(
    () => ProjectLock.acquire(lockPath),
    (error: unknown) =>
      error instanceof ControllerOwnershipError &&
      /not a lock database: remove it with the daemon stopped/.test(
        error.message,
      ),
  );
});
