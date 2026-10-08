/**
 * Tells whether a tool command started by an agent (a test run, a build) is
 * using CPU, so a long silent command is not reported as a stalled agent. The
 * probe reads the pane's shell pid from Herdr and the process table from the
 * operating system; the tracker is pure and keeps the last CPU activity.
 */
import fs from "node:fs";
import { execFile } from "node:child_process";
import { runJson, type HerdrRunner } from "./runner.js";

/** The least summed CPU increase between two samples that counts as activity. */
export const MIN_CHILD_CPU_MS = 200;

export interface ProcessEntry {
  readonly pid: number;
  readonly ppid: number;
  readonly comm: string;
  readonly cpuMs: number;
  /** Tells a reused pid from the same process: the start time on Linux, the command name elsewhere. */
  readonly startKey: string;
}

export type ProcessTable = readonly ProcessEntry[];

/** The counted processes of one pane: the tool subtrees under the agent. */
export interface ProcessSample {
  readonly processes: readonly ProcessEntry[];
}

export interface ProcessActivityProbe {
  sample(paneId: string): Promise<ProcessSample>;
}

export type ProcessTableReader = () => Promise<ProcessTable>;

const SHELLS: ReadonlySet<string> = new Set([
  "sh",
  "bash",
  "zsh",
  "dash",
  "fish",
  "ksh",
]);

function shellName(comm: string): string {
  const base = comm.split("/").pop() ?? comm;
  return base.replace(/^-/, "");
}

/**
 * Processes counted under `shellPid`: a depth-2 descendant (a child of the host
 * process) that is a shell, plus all of its descendants. Other depth-2 children
 * (MCP servers) are not counted.
 */
export function toolProcesses(
  table: ProcessTable,
  shellPid: number,
): ProcessEntry[] {
  const children = new Map<number, ProcessEntry[]>();
  for (const entry of table) {
    const list = children.get(entry.ppid);
    if (list === undefined) children.set(entry.ppid, [entry]);
    else list.push(entry);
  }
  const counted: ProcessEntry[] = [];
  const seen = new Set<number>([shellPid]);
  const walk = (entry: ProcessEntry): void => {
    if (seen.has(entry.pid)) return;
    seen.add(entry.pid);
    counted.push(entry);
    for (const child of children.get(entry.pid) ?? []) walk(child);
  };
  for (const host of children.get(shellPid) ?? [])
    for (const tool of children.get(host.pid) ?? [])
      if (SHELLS.has(shellName(tool.comm))) walk(tool);
  return counted;
}

/** Parses the content of /proc/<pid>/stat; the command name sits in parentheses and may contain spaces and parentheses. */
export function parseProcStat(
  pid: number,
  text: string,
  ticksPerSecond = 100,
): ProcessEntry | undefined {
  const open = text.indexOf("(");
  const close = text.lastIndexOf(")");
  if (open < 0 || close < open) return undefined;
  const comm = text.slice(open + 1, close);
  const fields = text
    .slice(close + 1)
    .trim()
    .split(/\s+/);
  // fields[0] is the state (field 3 of stat): ppid = 1, utime = 11, stime = 12, starttime = 19.
  const ppid = Number(fields[1]);
  const utime = Number(fields[11]);
  const stime = Number(fields[12]);
  // Children that already exited and were waited for: a test runner's short-lived test processes.
  const cutime = Number(fields[13]);
  const cstime = Number(fields[14]);
  const start = fields[19];
  if (
    !Number.isInteger(ppid) ||
    !Number.isFinite(utime) ||
    !Number.isFinite(stime) ||
    !Number.isFinite(cutime) ||
    !Number.isFinite(cstime) ||
    start === undefined
  )
    return undefined;
  return {
    pid,
    ppid,
    comm,
    cpuMs: Math.round(
      ((utime + stime + cutime + cstime) * 1000) / ticksPerSecond,
    ),
    startKey: start,
  };
}

/** Reads /proc once. */
export async function readProcTable(): Promise<ProcessTable> {
  const names = await fs.promises.readdir("/proc");
  const entries = await Promise.all(
    names
      .filter((name) => /^[0-9]+$/.test(name))
      .map(async (name) => {
        try {
          const text = await fs.promises.readFile(`/proc/${name}/stat`, "utf8");
          return parseProcStat(Number(name), text);
        } catch {
          return undefined; // the process exited
        }
      }),
  );
  return entries.filter((entry): entry is ProcessEntry => entry !== undefined);
}

/** The reads the walker makes of `/proc`; tests replace them. */
export interface ProcIo {
  /** Thread ids of a process; throws ENOENT when it is gone. */
  threads(pid: number): Promise<string[]>;
  /** The `children` list of one thread; throws ENOENT when the thread is gone or the kernel has no such file. */
  children(pid: number, thread: string): Promise<string>;
  /** The content of `/proc/<pid>/stat`; throws ENOENT or ESRCH when the process is gone. */
  stat(pid: number): Promise<string>;
}

const procIo: ProcIo = {
  threads: (pid) => fs.promises.readdir(`/proc/${pid}/task`),
  children: (pid, thread) =>
    fs.promises.readFile(`/proc/${pid}/task/${thread}/children`, "utf8"),
  stat: (pid) => fs.promises.readFile(`/proc/${pid}/stat`, "utf8"),
};

const GONE: ReadonlySet<string> = new Set(["ENOENT", "ESRCH"]);
const isGone = (error: unknown): boolean =>
  GONE.has((error as NodeJS.ErrnoException).code ?? "");

/** How long after a failed read the walker lets the caller read the whole table before it tries `children` lists again. */
export const CHILDREN_RETRY_MS = 60_000;

/**
 * Finds the processes `toolProcesses(await readProcTable(), shellPid)` counts by following `children` lists from the
 * shell pid instead of reading the stat of every process on the machine. `read` returns undefined when the caller has
 * to read the whole table: for good when the kernel has no `children` lists (no CONFIG_PROC_CHILDREN), and for
 * CHILDREN_RETRY_MS after any other failure (a transient EMFILE, a permission error), after which it tries again.
 */
export class ToolProcessWalker {
  #supported: boolean | undefined;
  #retryAt = 0;

  constructor(
    private readonly io: ProcIo = procIo,
    private readonly now: () => number = Date.now,
    private readonly retryMs: number = CHILDREN_RETRY_MS,
    private readonly self: number = process.pid,
  ) {}

  async read(shellPid: number): Promise<ProcessEntry[] | undefined> {
    if (this.#supported === false || this.now() < this.#retryAt)
      return undefined;
    try {
      if (this.#supported === undefined) this.#supported = await this.#probe();
      if (!this.#supported) return undefined;
      return await this.#walk(shellPid);
    } catch {
      this.#retryAt = this.now() + this.retryMs;
      return undefined;
    }
  }

  /** The kernel has `children` lists when this process's own list can be read: a missing file is the answer, any other failure throws. */
  async #probe(): Promise<boolean> {
    try {
      await this.io.children(this.self, String(this.self));
      return true;
    } catch (error) {
      if ((error as NodeJS.ErrnoException).code === "ENOENT") return false;
      throw error;
    }
  }

  async #childPids(pid: number): Promise<number[]> {
    let threads: string[];
    try {
      threads = await this.io.threads(pid);
    } catch (error) {
      if (isGone(error)) return [];
      throw error;
    }
    const lists = await Promise.all(
      threads.map(async (thread) => {
        try {
          return await this.io.children(pid, thread);
        } catch (error) {
          if (isGone(error)) return ""; // the thread exited
          throw error;
        }
      }),
    );
    return lists
      .flatMap((text) => text.split(" ").filter((id) => id !== ""))
      .map(Number);
  }

  async #entries(pid: number): Promise<ProcessEntry[]> {
    const found = await Promise.all(
      (await this.#childPids(pid)).map(async (id) => {
        try {
          return parseProcStat(id, await this.io.stat(id));
        } catch (error) {
          if (isGone(error)) return undefined; // the process exited
          throw error;
        }
      }),
    );
    return found.filter((entry): entry is ProcessEntry => entry !== undefined);
  }

  async #walk(shellPid: number): Promise<ProcessEntry[]> {
    const counted: ProcessEntry[] = [];
    const seen = new Set<number>([shellPid]);
    const add = async (entry: ProcessEntry): Promise<void> => {
      if (seen.has(entry.pid)) return;
      seen.add(entry.pid);
      counted.push(entry);
      for (const child of await this.#entries(entry.pid)) await add(child);
    };
    for (const host of await this.#entries(shellPid))
      for (const tool of await this.#entries(host.pid))
        if (SHELLS.has(shellName(tool.comm))) await add(tool);
    return counted;
  }
}

const defaultWalker = new ToolProcessWalker();

/** The tool processes under a pane's shell pid, or undefined when the whole table has to be read. */
export async function readToolProcesses(
  shellPid: number,
): Promise<ProcessEntry[] | undefined> {
  if (process.platform !== "linux") return undefined;
  return defaultWalker.read(shellPid);
}

/** `[[dd-]hh:]mm:ss[.cc]` in milliseconds. */
export function parsePsTime(text: string): number | undefined {
  const match = /^(?:(\d+)-)?(?:(\d+):)?(\d+):(\d+(?:\.\d+)?)$/.exec(text);
  if (match === null) return undefined;
  const [, days, hours, minutes, seconds] = match;
  return Math.round(
    (Number(days ?? 0) * 86_400 +
      Number(hours ?? 0) * 3_600 +
      Number(minutes) * 60 +
      Number(seconds)) *
      1000,
  );
}

/** Parses `ps -A -o pid=,ppid=,time=,comm=` output (a header line is skipped); the command name is the rest of the line and may hold spaces. */
export function parsePsTable(text: string): ProcessTable {
  const entries: ProcessEntry[] = [];
  for (const line of text.split("\n")) {
    const match = /^\s*(\d+)\s+(\d+)\s+(\S+)\s+(.+?)\s*$/.exec(line);
    if (match === null) continue;
    const cpuMs = parsePsTime(match[3]!);
    if (cpuMs === undefined) continue;
    const comm = match[4]!;
    entries.push({
      pid: Number(match[1]),
      ppid: Number(match[2]),
      comm,
      cpuMs,
      startKey: comm,
    });
  }
  return entries;
}

async function readPsTable(): Promise<ProcessTable> {
  const stdout = await new Promise<string>((resolve, reject) => {
    execFile(
      "ps",
      ["-A", "-o", "pid=,ppid=,time=,comm="],
      { maxBuffer: 16 * 1024 * 1024, timeout: 10_000 },
      (error, out) => (error ? reject(error) : resolve(out)),
    );
  });
  return parsePsTable(stdout);
}

export const defaultProcessTableReader: ProcessTableReader = () =>
  process.platform === "linux" ? readProcTable() : readPsTable();

/** Finds the tool processes under a shell pid, or undefined when the caller has to read the whole table. */
export type ToolProcessReader = (
  shellPid: number,
) => Promise<ProcessEntry[] | undefined>;

export class HerdrProcessProbe implements ProcessActivityProbe {
  constructor(
    private readonly runner: HerdrRunner,
    private readonly readTable: ProcessTableReader = defaultProcessTableReader,
    // Only the production probe follows `children` lists; a probe given its own table reader (a test) reads that table.
    private readonly readTools: ToolProcessReader | undefined = readTable ===
    defaultProcessTableReader
      ? readToolProcesses
      : undefined,
  ) {}

  async sample(paneId: string): Promise<ProcessSample> {
    const result = await runJson(this.runner, [
      "pane",
      "process-info",
      "--pane",
      paneId,
    ]);
    const info = result.process_info;
    const shellPid =
      typeof info === "object" && info !== null
        ? (info as { shell_pid?: unknown }).shell_pid
        : undefined;
    if (
      typeof shellPid !== "number" ||
      !Number.isInteger(shellPid) ||
      shellPid <= 0
    )
      throw new Error("herdr process info has no shell_pid");
    const direct = await this.readTools?.(shellPid);
    return {
      processes: direct ?? toolProcesses(await this.readTable(), shellPid),
    };
  }
}

export function createProcessProbe(
  runner: HerdrRunner,
  readTable?: ProcessTableReader,
): ProcessActivityProbe {
  return new HerdrProcessProbe(runner, readTable);
}

/** Keeps each agent's last CPU activity of its tool processes; pure, in memory. */
export class ProcessActivityTracker {
  readonly #previous = new Map<string, Map<string, number>>();
  readonly #lastActivity = new Map<string, number>();

  record(agentId: string, sample: ProcessSample, nowMs: number): void {
    const before = this.#previous.get(agentId);
    const next = new Map<string, number>();
    let increase = 0;
    for (const entry of sample.processes) {
      const key = `${entry.pid}|${entry.startKey}`;
      next.set(key, entry.cpuMs);
      const was = before?.get(key);
      // The first sample of an agent only sets the baseline; later, a process not seen before counts its whole CPU time as new work.
      if (before === undefined) continue;
      increase += Math.max(0, entry.cpuMs - (was ?? 0));
    }
    this.#previous.set(agentId, next);
    if (increase >= MIN_CHILD_CPU_MS) this.#lastActivity.set(agentId, nowMs);
  }

  lastChildActivity(agentId: string): number | undefined {
    return this.#lastActivity.get(agentId);
  }

  forget(agentId: string): void {
    this.#previous.delete(agentId);
    this.#lastActivity.delete(agentId);
  }

  /** Every agent with a last activity, for the messaging evaluation. */
  activity(): ReadonlyMap<string, number> {
    return this.#lastActivity;
  }
}
