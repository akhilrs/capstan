import { randomUUID } from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import type { MutationContext } from "./types.js";
import type { SchedulerMutationContextStore } from "./scheduler.js";

interface StoredContext {
  readonly requestId: string;
  readonly idempotencyKey: string;
  readonly expectedVersion: number;
  readonly inputRevision: number;
}

interface StateFile {
  readonly schemaVersion: 1;
  readonly planHash: string;
  readonly startedAtMs: number;
  readonly contexts: Record<string, StoredContext>;
}

export class FileMutationContextStore implements SchedulerMutationContextStore {
  readonly #file: string;
  readonly #credential: string;
  readonly #planHash: string;
  readonly #state: {
    schemaVersion: 1;
    planHash: string;
    startedAtMs: number;
    contexts: Record<string, StoredContext>;
  };
  #persistenceFailed = false;

  constructor(file: string, credential: string, planHash: string) {
    if (!path.isAbsolute(file) || !/^[a-f0-9]{64}$/.test(planHash))
      throw new TypeError("invalid durable mutation context store options");
    this.#file = file;
    this.#credential = credential;
    this.#planHash = planHash;
    try {
      const stat = fs.lstatSync(file);
      if (
        !stat.isFile() ||
        stat.isSymbolicLink() ||
        (process.getuid && stat.uid !== process.getuid()) ||
        (stat.mode & 0o077) !== 0
      )
        throw new Error(
          "mutation context state must be a private regular file owned by this user",
        );
      const parsed = JSON.parse(fs.readFileSync(file, "utf8")) as StateFile;
      if (
        parsed.schemaVersion !== 1 ||
        parsed.planHash !== planHash ||
        !Number.isSafeInteger(parsed.startedAtMs) ||
        parsed.startedAtMs <= 0 ||
        !parsed.contexts ||
        typeof parsed.contexts !== "object" ||
        Array.isArray(parsed.contexts)
      )
        throw new Error(
          "mutation context state does not match the accepted plan",
        );
      this.#state = { ...parsed, contexts: { ...parsed.contexts } };
    } catch (error) {
      if (
        !(error instanceof Error) ||
        !("code" in error) ||
        error.code !== "ENOENT"
      )
        throw error;
      this.#state = {
        schemaVersion: 1,
        planHash,
        startedAtMs: Date.now(),
        contexts: {},
      };
      this.#persist();
    }
  }

  get startedAtMs(): number {
    return this.#state.startedAtMs;
  }

  getOrCreate(key: string, create: () => MutationContext): MutationContext {
    if (!/^[a-f0-9]{32}$/.test(key))
      throw new TypeError("mutation key must be a stable SHA-256 prefix");
    if (this.#persistenceFailed)
      throw new Error("mutation context persistence previously failed");
    let stored = this.#state.contexts[key];
    if (!stored) {
      const context = create();
      if (
        context.credential !== this.#credential ||
        !context.requestId ||
        !context.idempotencyKey ||
        !Number.isSafeInteger(context.expectedVersion) ||
        !Number.isSafeInteger(context.inputRevision)
      )
        throw new TypeError(
          "mutation context factory returned an invalid context",
        );
      stored = {
        requestId: context.requestId,
        idempotencyKey: context.idempotencyKey,
        expectedVersion: context.expectedVersion,
        inputRevision: context.inputRevision,
      };
      this.#state.contexts[key] = stored;
      try {
        this.#persist();
      } catch (error) {
        this.#persistenceFailed = true;
        delete this.#state.contexts[key];
        throw error;
      }
    }
    return { ...stored, credential: this.#credential };
  }

  #persist(): void {
    const directory = path.dirname(this.#file);
    const temp = `${this.#file}.${randomUUID()}.tmp`;
    const fd = fs.openSync(
      temp,
      fs.constants.O_WRONLY |
        fs.constants.O_CREAT |
        fs.constants.O_EXCL |
        fs.constants.O_NOFOLLOW,
      0o600,
    );
    try {
      fs.writeFileSync(fd, `${JSON.stringify(this.#state)}\n`, "utf8");
      fs.fsyncSync(fd);
    } finally {
      fs.closeSync(fd);
    }
    fs.renameSync(temp, this.#file);
    fs.chmodSync(this.#file, 0o600);
    const dirFd = fs.openSync(
      directory,
      fs.constants.O_RDONLY | fs.constants.O_DIRECTORY,
    );
    try {
      fs.fsyncSync(dirFd);
    } finally {
      fs.closeSync(dirFd);
    }
  }
}
