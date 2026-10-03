import { randomBytes } from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import type { DatabaseSync } from "node:sqlite";
import { loadNodeSqlite } from "./sqlite.js";

export class ControllerOwnershipError extends Error {
  override readonly name = "ControllerOwnershipError";
}

/** Another process holds the project lock, usually the running daemon. */
export class ProjectLockHeldError extends ControllerOwnershipError {
  constructor() {
    super("another cooperating controller owns this project");
  }
}

const REQUIRED_NODE = "Node 24.6 or newer";
const SQLITE_BUSY = 5;
const SQLITE_LOCKED = 6;
const SQLITE_CORRUPT = 11;
const SQLITE_NOTADB = 26;

function isSystemError(error: unknown): error is NodeJS.ErrnoException {
  return error instanceof Error && "code" in error;
}

function sqliteErrcode(error: unknown): number | undefined {
  if (!(error instanceof Error)) return undefined;
  const code = (error as { errcode?: unknown }).errcode;
  return typeof code === "number" ? code : undefined;
}

/** Creates the lock file private to the user, or reports that it already exists. */
function createIfMissing(lockPath: string): void {
  try {
    const fd = fs.openSync(
      lockPath,
      fs.constants.O_CREAT |
        fs.constants.O_EXCL |
        fs.constants.O_WRONLY |
        fs.constants.O_NOFOLLOW,
      0o600,
    );
    try {
      // The umask may have narrowed the mode; SQLite needs to write the file.
      fs.fchmodSync(fd, 0o600);
    } finally {
      fs.closeSync(fd);
    }
  } catch (error) {
    if (!(isSystemError(error) && error.code === "EEXIST")) throw error;
  }
}

function checkedLstat(lockPath: string): fs.Stats {
  const stat = fs.lstatSync(lockPath);
  if (stat.isSymbolicLink() || !stat.isFile()) {
    throw new ControllerOwnershipError(
      "controller lock path must identify a regular file",
    );
  }
  if (
    (stat.mode & 0o077) !== 0 ||
    (process.getuid && stat.uid !== process.getuid())
  ) {
    throw new ControllerOwnershipError(
      "controller lock file must be private to the current user",
    );
  }
  return stat;
}

/**
 * The project lock is an exclusive SQLite lock on controller.lock. The
 * connection stays open for the life of the lock. POSIX drops every fcntl lock
 * a process holds on a file when it closes any fd on it, so this file is never
 * opened or read through fs while the lock may be held; identity checks use
 * lstat by path.
 */
export class ProjectLock {
  readonly #db: DatabaseSync;
  readonly #lockPath: string;
  readonly #device: number;
  readonly #inode: number;
  #closed = false;

  private constructor(
    db: DatabaseSync,
    lockPath: string,
    device: number,
    inode: number,
  ) {
    this.#db = db;
    this.#lockPath = lockPath;
    this.#device = device;
    this.#inode = inode;
  }

  static acquire(lockPath: string): ProjectLock {
    if (!path.isAbsolute(lockPath))
      throw new ControllerOwnershipError("lock path must be absolute");
    const directoryStat = fs.lstatSync(path.dirname(lockPath));
    if (!directoryStat.isDirectory() || directoryStat.isSymbolicLink()) {
      throw new ControllerOwnershipError(
        "controller state directory must be a real directory",
      );
    }
    let sqlite: ReturnType<typeof loadNodeSqlite>;
    try {
      sqlite = loadNodeSqlite();
    } catch (error) {
      if (error instanceof ControllerOwnershipError) throw error;
      throw new ControllerOwnershipError(
        `controller lock needs node:sqlite (${REQUIRED_NODE})`,
      );
    }

    createIfMissing(lockPath);
    const before = checkedLstat(lockPath);

    let db: DatabaseSync | undefined;
    try {
      db = new sqlite.DatabaseSync(lockPath, { timeout: 0 });
      db.exec("PRAGMA journal_mode=OFF");
      db.exec("PRAGMA locking_mode=EXCLUSIVE");
      db.exec("BEGIN EXCLUSIVE");
      const after = fs.lstatSync(lockPath);
      if (
        after.isSymbolicLink() ||
        after.dev !== before.dev ||
        after.ino !== before.ino
      ) {
        throw new ControllerOwnershipError(
          "controller lock path changed while it was being locked",
        );
      }
      db.exec(
        "CREATE TABLE IF NOT EXISTS owner (pid INTEGER NOT NULL, started_at TEXT NOT NULL, token TEXT NOT NULL)",
      );
      db.exec("DELETE FROM owner");
      db.prepare(
        "INSERT INTO owner (pid, started_at, token) VALUES (?, ?, ?)",
      ).run(
        process.pid,
        new Date().toISOString(),
        randomBytes(16).toString("hex"),
      );
      db.exec("COMMIT");
      return new ProjectLock(db, lockPath, before.dev, before.ino);
    } catch (error) {
      try {
        db?.close();
      } catch {
        // The original failure is the one to report.
      }
      const code = sqliteErrcode(error);
      if (code === SQLITE_BUSY || code === SQLITE_LOCKED)
        throw new ProjectLockHeldError();
      if (code === SQLITE_NOTADB || code === SQLITE_CORRUPT) {
        throw new ControllerOwnershipError(
          "controller lock file is not a lock database: remove it with the daemon stopped",
        );
      }
      throw error;
    }
  }

  assertHeld(): void {
    if (this.#closed)
      throw new ControllerOwnershipError(
        "controller ownership has been released",
      );
    let pathStat: fs.Stats;
    try {
      pathStat = fs.lstatSync(this.#lockPath);
    } catch (error) {
      if (isSystemError(error) && error.code === "ENOENT") {
        throw new ControllerOwnershipError("controller lock path was removed");
      }
      throw error;
    }
    if (
      pathStat.isSymbolicLink() ||
      pathStat.dev !== this.#device ||
      pathStat.ino !== this.#inode
    ) {
      throw new ControllerOwnershipError(
        "controller lock path no longer identifies the owned inode",
      );
    }
  }

  close(): void {
    if (this.#closed) return;
    this.#closed = true;
    this.#db.close();
  }
}
