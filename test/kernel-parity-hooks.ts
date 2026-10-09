/**
 * The determinism hooks of the kernel parity export (rust/crates/kernel). The controller reads the clock with
 * `new Date()` and takes randomness from `randomUUID`/`randomBytes` of node:crypto directly, so a run is made
 * reproducible by replacing those for the length of a call, without touching src/:
 *
 *  - randomness is a SHA-256 counter stream: block i is SHA-256(seed UTF-8 || i as 8 bytes, big endian); `randomBytes(n)`
 *    takes whole blocks until it has n bytes and drops the rest of the last one; `randomUUID()` formats 16 such bytes as
 *    a version 4 UUID;
 *  - the clock returns 2026-01-01T00:00:00.000Z on its first reading and one millisecond more on each later one, for
 *    `new Date()` without arguments and for `Date.now()`.
 *
 * rust/crates/kernel/src/env.rs `SeededEnv` is the same stream and the same clock; both consume them in the same order.
 * `ProjectLock.acquire` (a lock token and a timestamp the Rust ledger takes from the system) runs with the hooks
 * paused. node:crypto's exports are replaced and `syncBuiltinESMExports` makes the named imports of src/ see them.
 */
import crypto, { createHash } from "node:crypto";
import { syncBuiltinESMExports } from "node:module";
import { ProjectLock } from "../src/controller/ownership.js";

export const SEEDED_EPOCH_MS = Date.UTC(2026, 0, 1);

export class SeededStream {
  readonly #seed: Buffer;
  #counter = 0n;

  constructor(seed: string) {
    this.#seed = Buffer.from(seed, "utf8");
  }

  bytes(n: number): Buffer {
    const blocks: Buffer[] = [];
    let have = 0;
    while (have < n) {
      const counter = Buffer.alloc(8);
      counter.writeBigUInt64BE(this.#counter);
      this.#counter += 1n;
      const block = createHash("sha256")
        .update(this.#seed)
        .update(counter)
        .digest();
      blocks.push(block);
      have += block.length;
    }
    return Buffer.concat(blocks).subarray(0, n);
  }

  uuid(): string {
    const bytes = Buffer.from(this.bytes(16));
    bytes[6] = (bytes[6]! & 0x0f) | 0x40;
    bytes[8] = (bytes[8]! & 0x3f) | 0x80;
    const hex = bytes.toString("hex");
    return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20, 32)}`;
  }
}

export interface Hooks {
  /** Runs `fn` with the seeded clock and randomness active. */
  run<T>(fn: () => T): T;
  /** Runs `fn` with the real clock and randomness, inside a `run`. */
  paused<T>(fn: () => T): T;
  /** The next bytes of the stream without the hooks being active (for expectations that need them). */
  stream: SeededStream;
  /** Moves the seeded clock forward without a reading (a timer that elapsed). */
  advance(ms: number): void;
  restore(): void;
}

const RealDate = Date;
type Acquire = typeof ProjectLock.acquire;

/** Installs the hooks for one sequence. Call `restore()` when it is done. */
export function installHooks(seed: string): Hooks {
  const stream = new SeededStream(seed);
  let clock = SEEDED_EPOCH_MS;
  let active = 0;
  let paused = 0;
  const live = (): boolean => active > 0 && paused === 0;
  const tick = (): number => {
    const now = clock;
    clock += 1;
    return now;
  };

  const original = {
    randomUUID: crypto.randomUUID,
    randomBytes: crypto.randomBytes,
    acquire: ProjectLock.acquire,
    Date: globalThis.Date,
  };

  class SeededDate extends RealDate {
    constructor(...args: unknown[]) {
      if (args.length === 0 && live()) super(tick());
      else super(...(args as [number]));
    }

    static override now(): number {
      return live() ? tick() : RealDate.now();
    }
  }

  const randomUUID = ((...args: unknown[]) =>
    live()
      ? stream.uuid()
      : (original.randomUUID as (...a: unknown[]) => string)(
          ...args,
        )) as typeof crypto.randomUUID;
  const randomBytes = ((size: number, ...rest: unknown[]) =>
    live()
      ? stream.bytes(size)
      : (original.randomBytes as (...a: unknown[]) => Buffer)(
          size,
          ...rest,
        )) as typeof crypto.randomBytes;

  const pausedCall = <T>(fn: () => T): T => {
    paused += 1;
    try {
      return fn();
    } finally {
      paused -= 1;
    }
  };

  (crypto as { randomUUID: unknown }).randomUUID = randomUUID;
  (crypto as { randomBytes: unknown }).randomBytes = randomBytes;
  syncBuiltinESMExports();
  globalThis.Date = SeededDate as unknown as DateConstructor;
  (ProjectLock as { acquire: Acquire }).acquire = ((lockPath: string) =>
    pausedCall(() => original.acquire.call(ProjectLock, lockPath))) as Acquire;

  return {
    stream,
    run: <T>(fn: () => T): T => {
      active += 1;
      let pending = false;
      try {
        const result = fn();
        if (result instanceof Promise) {
          pending = true;
          return result.finally(() => {
            active -= 1;
          }) as T;
        }
        return result;
      } finally {
        if (!pending) active -= 1;
      }
    },
    paused: pausedCall,
    advance: (ms: number) => {
      clock += ms;
    },
    restore: () => {
      (crypto as { randomUUID: unknown }).randomUUID = original.randomUUID;
      (crypto as { randomBytes: unknown }).randomBytes = original.randomBytes;
      syncBuiltinESMExports();
      globalThis.Date = original.Date;
      (ProjectLock as { acquire: Acquire }).acquire = original.acquire;
    },
  };
}
