/**
 * Captures the sequences of test/kernel-sequences/captured.json from the existing controller suites.
 *
 * With CAPSTAN_KERNEL_CAPTURE_DIR set, importing this module wraps `ControllerCore` (`open`, `close` and every public
 * method) and appends what a controller is asked to do, and what it answers, to `<dir>/<pid>.jsonl`. Without it the
 * module does nothing. `merge` then turns the log into parity sequences (language: test/kernel-sequences/README.md):
 *
 *   npm run build
 *   CAPSTAN_KERNEL_CAPTURE_DIR=/tmp/capture NODE_OPTIONS="--import=$PWD/dist/test/kernel-capture-hooks.js" \
 *     node --test dist/test/controller-*.test.js dist/test/messages.test.js dist/test/findings.test.js \
 *       dist/test/reports.test.js dist/test/reviews.test.js
 *   node dist/test/kernel-capture-hooks.js merge /tmp/capture [--out test/kernel-sequences/captured.json] [--max-bytes 215000]
 *   node dist/test/kernel-parity-export.js
 *
 * What a capture keeps. One sequence per state directory (and process), in the order of the calls: `open` and `close`
 * of every controller handle (`h1`, `h2`, ...) and every call with its arguments. Values that the real run drew at random
 * (ids, credentials) are replaced by references to the result of the earlier step that returned them (`as`, `$rN.path`),
 * so the sequence replays under the seeded clock and randomness of the exporter; the owner credential becomes
 * `$owner.credential`, the internal controller's `$internal.credential` (told apart with `identify` while capturing),
 * request and idempotency keys become `rq-N` / `ik-N` (the same captured key stays the same key), and an expected
 * version or input revision stays literal only where the call did not use the controller's current one. What it leaves out:
 * calls with a function argument (`sweepFindings`, `abandonRunningOperatorRuns`), read-only controllers, directories that
 * held a ledger before the first open, controllers given a workspace or a clock, and calls nested inside another call.
 * What the real run answered is not kept: the exporter records what Node answers to the replayed steps, and the Rust
 * kernel must agree with that.
 *
 * `merge` drops duplicates, cuts a long sequence to a prefix of at most an eighth of the budget, and fills the budget
 * greedily with the sequences that add the most (operation, outcome) pairs per byte, so every suite contributes where it
 * covers something the others do not. The budget is for the committed text (formatted like every other file under
 * test/) and defaults to 2 MB; the committed file is made with `--max-bytes 215000` because its export, which adds the
 * results and the ledger differences, is about 2.9 times larger and the parity files together stay under 8 MB.
 */
import { createHash } from "node:crypto";
import {
  appendFileSync,
  existsSync,
  mkdirSync,
  readdirSync,
  readFileSync,
  writeFileSync,
} from "node:fs";
import path from "node:path";
import { format } from "prettier";
import { ControllerCore } from "../src/controller/core.js";

const root = path.resolve(import.meta.dirname, "..", "..");
const DEFAULT_OUT = path.join(
  root,
  "test",
  "kernel-sequences",
  "captured.json",
);
/** The largest captured.json, source text, in bytes. */
const DEFAULT_MAX_BYTES = 2 * 1024 * 1024;
/** A result larger than this is not kept (it can only be referenced through its strings, which are then literal). */
const MAX_RESULT_BYTES = 64 * 1024;
const MIN_REFERENCED = 8;

type Json = null | boolean | number | string | Json[] | { [key: string]: Json };

// ------------------------------------------------------------------------------------------------ capture

type Event =
  | {
      k: "open";
      handle: number;
      stateDir: string;
      pre: boolean;
      readOnly: boolean;
      supported: boolean;
      project: Json;
      ok: boolean;
    }
  | { k: "close"; handle: number }
  | {
      k: "call";
      handle: number;
      op: string;
      args: Json[];
      version: number | null;
      revision: number | null;
      role: string | null;
      result?: Json;
      error?: string;
    };

function clone(value: unknown): Json | undefined {
  try {
    const text = JSON.stringify(value);
    return text === undefined ? null : (JSON.parse(text) as Json);
  } catch {
    return undefined;
  }
}

/** Whether reading `args` could be seen by the caller: a function (not JSON), or a getter (reading it is a side effect). */
function unsafeToRead(args: unknown[]): boolean {
  const seen = new Set<unknown>();
  const visit = (value: unknown): boolean => {
    if (typeof value === "function") return true;
    if (value === null || typeof value !== "object" || seen.has(value))
      return false;
    seen.add(value);
    return Object.values(Object.getOwnPropertyDescriptors(value)).some(
      (descriptor) =>
        descriptor.get !== undefined || visit(descriptor.value as unknown),
    );
  };
  return args.some(visit);
}

function isContext(value: unknown): value is Record<string, unknown> {
  return (
    value !== null &&
    typeof value === "object" &&
    typeof (value as Record<string, unknown>).credential === "string" &&
    "requestId" in value &&
    "idempotencyKey" in value
  );
}

export function installCapture(directory: string): void {
  mkdirSync(directory, { recursive: true });
  const file = path.join(directory, `${process.pid}.jsonl`);
  const emit = (event: Event): void => {
    appendFileSync(file, JSON.stringify(event) + "\n");
  };
  const handles = new WeakMap<object, number>();
  const nested = new WeakSet<object>();
  const known = new WeakMap<object, Map<string, string | null>>();
  let counter = 0;

  const prototype = ControllerCore.prototype as unknown as Record<
    string,
    unknown
  >;
  const original = new Map<string, (...args: unknown[]) => unknown>();

  const roleOf = (core: ControllerCore, credential: string): string | null => {
    let cache = known.get(core);
    if (cache === undefined) known.set(core, (cache = new Map()));
    if (cache.has(credential)) return cache.get(credential) ?? null;
    let role: string | null = null;
    try {
      const identify = original.get("identify");
      const identity = identify?.call(core, credential) as
        { role?: string } | undefined;
      role = identity?.role ?? null;
    } catch {
      role = null;
    }
    cache.set(credential, role);
    return role;
  };

  for (const name of Object.getOwnPropertyNames(prototype)) {
    if (name === "constructor") continue;
    const descriptor = Object.getOwnPropertyDescriptor(prototype, name);
    const fn = descriptor?.value as unknown;
    if (typeof fn !== "function") continue;
    original.set(name, fn as (...args: unknown[]) => unknown);
  }

  for (const [name, fn] of original) {
    const wrapped = function (this: ControllerCore, ...args: unknown[]) {
      const handle = handles.get(this);
      if (handle === undefined || nested.has(this)) return fn.apply(this, args);
      if (name === "close") {
        try {
          return fn.apply(this, args);
        } finally {
          emit({ k: "close", handle });
        }
      }
      if (unsafeToRead(args)) return fn.apply(this, args);
      nested.add(this);
      try {
        const cloned = args.map((a) => clone(a) ?? null);
        let version: number | null = null;
        let revision: number | null = null;
        try {
          version = this.stateVersion;
          revision = this.inputRevision;
        } catch {
          // a closed controller has no counters
        }
        const first = args[0];
        const role = isContext(first)
          ? roleOf(this, first.credential as string)
          : typeof first === "string" && name !== "identify"
            ? roleOf(this, first)
            : null;
        let result: unknown;
        try {
          result = fn.apply(this, args);
        } catch (error) {
          emit({
            k: "call",
            handle,
            op: name,
            args: cloned,
            version,
            revision,
            role,
            error: (error as { name?: string } | null)?.name ?? "Error",
          });
          throw error;
        }
        const kept = clone(result);
        const small =
          kept !== undefined && JSON.stringify(kept).length <= MAX_RESULT_BYTES;
        emit({
          k: "call",
          handle,
          op: name,
          args: cloned,
          version,
          revision,
          role,
          result: small ? kept : null,
        });
        return result;
      } finally {
        nested.delete(this);
      }
    };
    Object.defineProperty(wrapped, "name", { value: name });
    Object.defineProperty(prototype, name, {
      value: wrapped,
      writable: true,
      configurable: true,
    });
  }

  const wrapOpen = (name: "open" | "openReadOnly"): void => {
    const real = ControllerCore[name].bind(ControllerCore) as unknown as (
      options: Record<string, unknown>,
    ) => Promise<ControllerCore>;
    Object.defineProperty(ControllerCore, name, {
      value: async (options: Record<string, unknown>) => {
        const stateDir = String(options.stateDirectory);
        const pre = existsSync(path.join(stateDir, "controller.sqlite"));
        const handle = ++counter;
        const supported =
          options.workspaceRoot === undefined &&
          options.runtimeWorkspacePath === undefined &&
          options.clock === undefined &&
          !unsafeToRead([options.project]);
        const project = supported ? (clone(options.project) ?? null) : null;
        try {
          const core = await real(options);
          handles.set(core, handle);
          emit({
            k: "open",
            handle,
            stateDir,
            pre,
            readOnly: name === "openReadOnly",
            supported,
            project,
            ok: true,
          });
          return core;
        } catch (error) {
          emit({
            k: "open",
            handle,
            stateDir,
            pre,
            readOnly: name === "openReadOnly",
            supported,
            project,
            ok: false,
          });
          throw error;
        }
      },
      writable: true,
      configurable: true,
    });
  };
  wrapOpen("open");
  wrapOpen("openReadOnly");
}

// ------------------------------------------------------------------------------------------------ merge

interface Step {
  op: string;
  on?: string;
  context?: Record<string, Json>;
  args?: Json[];
  as?: string;
}

interface Candidate {
  name: string;
  open: false;
  steps: Step[];
  /** The (operation, outcome) pairs of the sequence, as the real run answered them. */
  covers: Set<string>;
}

const PLAIN_REFERENCE = /^\$[A-Za-z]/;

/** Every string of a value with the path of keys and indices to it; a key with a dot cannot be referenced. */
function leaves(
  value: Json,
  visit: (text: string, at: string[]) => void,
  at: string[] = [],
): void {
  if (typeof value === "string") visit(value, at);
  else if (Array.isArray(value))
    value.forEach((x, i) => leaves(x, visit, [...at, String(i)]));
  else if (value !== null && typeof value === "object")
    for (const [key, x] of Object.entries(value))
      leaves(x, visit, [...at, key]);
}

function mapStrings(value: Json, change: (text: string) => Json): Json {
  if (typeof value === "string") return change(value);
  if (Array.isArray(value)) return value.map((x) => mapStrings(x, change));
  if (value !== null && typeof value === "object")
    return Object.fromEntries(
      Object.entries(value).map(([key, x]) => [key, mapStrings(x, change)]),
    );
  return value;
}

function sequenceOf(
  events: Event[],
): { steps: Step[]; covers: Set<string> } | undefined {
  const opens = events.filter((e) => e.k === "open");
  if (opens.length === 0) return undefined;
  const firstOpen = opens[0];
  if (firstOpen?.k !== "open" || firstOpen.pre) return undefined;
  // A read-only controller or an unsupported option makes the sequence a different experiment from the replay.
  const skipped = new Set<number>();
  for (const event of opens)
    if (event.k === "open" && (event.readOnly || !event.ok))
      skipped.add(event.handle);
  if (opens.some((e) => e.k === "open" && !e.supported)) return undefined;
  const ownerCredential = (
    firstOpen.project as { ownerCredential?: unknown } | null
  )?.ownerCredential;
  if (typeof ownerCredential !== "string") return undefined;

  /** First result that returned a string: its step index and the path to it. */
  const returned = new Map<string, { step: number; at: string[] }>();
  const names = new Map<number, string>();
  const keys = new Map<string, string>();
  const key = (prefix: string, value: string): string => {
    const id = `${prefix}\u0000${value}`;
    let short = keys.get(id);
    if (short === undefined) {
      short = `${prefix}-${keys.size + 1}`;
      keys.set(id, short);
    }
    return short;
  };
  const steps: Step[] = [];
  const covers = new Set<string>();
  const handleName = (handle: number): string => `h${handle}`;

  const reference = (text: string): Json => {
    if (text === ownerCredential) return "$owner.credential";
    const found = returned.get(text);
    if (found === undefined || text.length < MIN_REFERENCED) return text;
    let bound = names.get(found.step);
    if (bound === undefined) {
      bound = `r${names.size + 1}`;
      names.set(found.step, bound);
    }
    return ["$" + bound, ...found.at].join(".");
  };

  for (const event of events) {
    if (event.k === "open") {
      if (skipped.has(event.handle)) continue;
      const project = mapStrings(event.project, (text) =>
        text === ownerCredential ? "$owner.credential" : text,
      );
      steps.push({
        op: "open",
        on: handleName(event.handle),
        args: [{ project }],
      });
    } else if (event.k === "close") {
      if (skipped.has(event.handle)) continue;
      steps.push({ op: "close", on: handleName(event.handle) });
    } else {
      if (skipped.has(event.handle)) continue;
      let invalid = false;
      leaves(event.args as Json, (text) => {
        if (PLAIN_REFERENCE.test(text)) invalid = true;
      });
      if (invalid) return undefined;
      const step: Step = { op: event.op, on: handleName(event.handle) };
      let args = event.args;
      const first = args[0];
      const contextual =
        first !== null &&
        typeof first === "object" &&
        !Array.isArray(first) &&
        typeof first.credential === "string" &&
        "requestId" in first &&
        "idempotencyKey" in first;
      const credentialOf = (credential: string): Json => {
        if (event.role === "controller") {
          return "$internal.credential";
        }
        return reference(credential);
      };
      if (contextual) {
        const context = first as { [key: string]: Json };
        const made: Record<string, Json> = {
          credential: credentialOf(context.credential as string),
          requestId: key("rq", String(context.requestId)),
          idempotencyKey: key("ik", String(context.idempotencyKey)),
        };
        if (context.expectedVersion !== event.version)
          made.expectedVersion = context.expectedVersion ?? null;
        if (context.inputRevision !== event.revision)
          made.inputRevision = context.inputRevision ?? null;
        step.context = made;
        args = args.slice(1);
      } else if (typeof first === "string" && event.role === "controller") {
        // A read that takes a credential first (`pullPending(credential)`) with the internal controller's.
        args = ["$internal.credential", ...args.slice(1)];
      }
      step.args = args.map((a) =>
        mapStrings(a, (text) =>
          text === "$internal.credential" ? text : reference(text),
        ),
      );
      steps.push(step);
      covers.add(`${event.op}:${event.error ?? "ok"}`);
      if (event.result !== undefined && event.result !== null) {
        const index = steps.length - 1;
        leaves(event.result, (text, at) => {
          if (
            text.length >= MIN_REFERENCED &&
            !returned.has(text) &&
            !at.some((part) => part.includes("."))
          )
            returned.set(text, { step: index, at });
        });
      }
    }
  }
  for (const [index, bound] of names) {
    const step = steps[index];
    if (step !== undefined) step.as = bound;
  }
  return { steps, covers };
}

function readEvents(directory: string): Map<string, Event[]> {
  const groups = new Map<string, Event[]>();
  for (const file of readdirSync(directory).sort()) {
    if (!file.endsWith(".jsonl")) continue;
    const lines = readFileSync(path.join(directory, file), "utf8").split("\n");
    // Handles are per process; their state directory is the one of their open event.
    const owner = new Map<number, string>();
    for (const line of lines) {
      if (line === "") continue;
      const event = JSON.parse(line) as Event;
      if (event.k === "open") owner.set(event.handle, event.stateDir);
      const stateDir = owner.get(event.handle);
      if (stateDir === undefined) continue;
      const id = `${file}\u0000${stateDir}`;
      const list = groups.get(id) ?? [];
      list.push(event);
      groups.set(id, list);
    }
  }
  return groups;
}

function textOf(candidate: Pick<Candidate, "name" | "open" | "steps">): string {
  return JSON.stringify(candidate);
}

export interface MergeResult {
  readonly text: string;
  readonly sequences: number;
  readonly candidates: number;
  readonly bytes: number;
}

/** Builds the text of captured.json from a capture directory. */
export async function merge(
  directory: string,
  maxBytes = DEFAULT_MAX_BYTES,
): Promise<MergeResult> {
  const unique = new Map<string, Candidate>();
  for (const [, events] of readEvents(directory)) {
    const built = sequenceOf(events);
    if (built === undefined || built.steps.length < 3) continue;
    const cap = Math.floor(maxBytes / 8);
    let steps = built.steps;
    let size = 0;
    const cut: Step[] = [];
    for (const step of steps) {
      size += JSON.stringify(step).length + 1;
      if (size > cap) break;
      cut.push(step);
    }
    steps = cut;
    if (steps.length < 3) continue;
    const covers = new Set<string>();
    for (const step of steps) covers.add(`${step.op}:`);
    for (const c of built.covers)
      if (steps.some((s) => c.startsWith(`${s.op}:`))) covers.add(c);
    const digest = createHash("sha256")
      .update(JSON.stringify(steps))
      .digest("hex");
    if (unique.has(digest)) continue;
    unique.set(digest, {
      name: `captured-${digest.slice(0, 10)}`,
      open: false,
      steps,
      covers,
    });
  }
  const candidates = [...unique.values()];
  // The budget is for the committed text, which prettier lays out wider than the compact form the selection measures.
  let budget = maxBytes;
  for (;;) {
    const chosen = select(candidates, budget);
    const sequences = chosen.map(({ name, open, steps }) => ({
      name,
      open,
      steps,
    }));
    const text = await format(JSON.stringify({ sequences }), {
      parser: "json",
    });
    const bytes = Buffer.byteLength(text);
    if (bytes <= maxBytes || sequences.length === 0)
      return {
        text,
        sequences: sequences.length,
        candidates: candidates.length,
        bytes,
      };
    budget = Math.floor(budget * (maxBytes / bytes) * 0.97);
  }
}

/** The sequences that add the most (operation, outcome) pairs per byte, while their compact text fits `budget`. */
function select(candidates: Candidate[], budget: number): Candidate[] {
  const chosen: Candidate[] = [];
  const have = new Set<string>();
  let bytes = 2_000;
  const left = new Set(candidates);
  while (left.size > 0) {
    let best: Candidate | undefined;
    let bestScore = 0;
    for (const candidate of left) {
      const size = textOf(candidate).length;
      if (bytes + size > budget) {
        left.delete(candidate);
        continue;
      }
      let gain = 0;
      for (const c of candidate.covers) if (!have.has(c)) gain += 1;
      const score = gain / size;
      if (score > bestScore) {
        best = candidate;
        bestScore = score;
      }
    }
    if (best === undefined) break;
    left.delete(best);
    chosen.push(best);
    bytes += textOf(best).length;
    for (const c of best.covers) have.add(c);
  }
  return chosen.sort((a, b) => (a.name < b.name ? -1 : 1));
}

// ------------------------------------------------------------------------------------------------ entry points

const capture = process.env.CAPSTAN_KERNEL_CAPTURE_DIR;
if (capture !== undefined && capture !== "") installCapture(capture);

if (import.meta.filename === process.argv[1]) {
  const [command, directory, ...rest] = process.argv.slice(2);
  if (command !== "merge" || directory === undefined) {
    process.stderr.write(
      "usage: node dist/test/kernel-capture-hooks.js merge <capture dir> [--out <file>] [--max-bytes <n>]\n",
    );
    process.exit(2);
  }
  const option = (name: string): string | undefined => {
    const at = rest.indexOf(name);
    return at === -1 ? undefined : rest[at + 1];
  };
  const out = path.resolve(option("--out") ?? DEFAULT_OUT);
  const limit = Number(option("--max-bytes") ?? DEFAULT_MAX_BYTES);
  const result = await merge(path.resolve(directory), limit);
  mkdirSync(path.dirname(out), { recursive: true });
  writeFileSync(out, result.text);
  process.stdout.write(
    `captured ${result.sequences} of ${result.candidates} sequences, ${result.bytes} bytes, into ${out}\n`,
  );
}
