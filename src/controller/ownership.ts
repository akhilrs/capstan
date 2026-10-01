import fs from "node:fs";
import path from "node:path";
import { flockSync } from "fs-ext";

export const DAEMON_OWNS_PROJECT =
  "another cooperating controller owns this project";

export class ControllerOwnershipError extends Error {
  override readonly name = "ControllerOwnershipError";
}

function isSystemError(error: unknown): error is NodeJS.ErrnoException {
  return error instanceof Error && "code" in error;
}

export class ProjectLock {
  readonly #fd: number;
  readonly #lockPath: string;
  readonly #device: number;
  readonly #inode: number;
  #closed = false;

  private constructor(
    fd: number,
    lockPath: string,
    device: number,
    inode: number,
  ) {
    this.#fd = fd;
    this.#lockPath = lockPath;
    this.#device = device;
    this.#inode = inode;
  }

  static acquire(lockPath: string): ProjectLock {
    if (!path.isAbsolute(lockPath))
      throw new ControllerOwnershipError("lock path must be absolute");
    const directory = path.dirname(lockPath);
    const directoryStat = fs.lstatSync(directory);
    if (!directoryStat.isDirectory() || directoryStat.isSymbolicLink()) {
      throw new ControllerOwnershipError(
        "controller state directory must be a real directory",
      );
    }

    const flags =
      fs.constants.O_CREAT | fs.constants.O_RDWR | fs.constants.O_NOFOLLOW;
    const fd = fs.openSync(lockPath, flags, 0o600);
    try {
      const fileStat = fs.fstatSync(fd);
      const pathStat = fs.lstatSync(lockPath);
      if (
        !fileStat.isFile() ||
        pathStat.isSymbolicLink() ||
        fileStat.dev !== pathStat.dev ||
        fileStat.ino !== pathStat.ino
      ) {
        throw new ControllerOwnershipError(
          "controller lock path must identify the opened regular file",
        );
      }
      if (
        (fileStat.mode & 0o077) !== 0 ||
        (process.getuid && fileStat.uid !== process.getuid())
      ) {
        throw new ControllerOwnershipError(
          "controller lock file must be private to the current user",
        );
      }
      try {
        flockSync(fd, "exnb");
      } catch (error) {
        if (
          isSystemError(error) &&
          ["EAGAIN", "EWOULDBLOCK"].includes(error.code ?? "")
        ) {
          throw new ControllerOwnershipError(DAEMON_OWNS_PROJECT);
        }
        throw error;
      }
      return new ProjectLock(fd, lockPath, fileStat.dev, fileStat.ino);
    } catch (error) {
      fs.closeSync(fd);
      throw error;
    }
  }

  assertHeld(): void {
    if (this.#closed)
      throw new ControllerOwnershipError(
        "controller ownership has been released",
      );
    const fileStat = fs.fstatSync(this.#fd);
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
      fileStat.dev !== this.#device ||
      fileStat.ino !== this.#inode ||
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
    try {
      flockSync(this.#fd, "un");
    } finally {
      fs.closeSync(this.#fd);
    }
  }
}
