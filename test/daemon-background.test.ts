import assert from "node:assert/strict";
import { spawn, spawnSync, type ChildProcess } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { mock, test } from "node:test";
import { pathToFileURL } from "node:url";
import {
  ADDED_NODE_OPTION_VARIABLE,
  DAEMON_SEMI_SPACE_FLAG,
  daemonCommand,
  restoreNodeOptions,
} from "../src/client.js";
import {
  CHILDREN_RETRY_MS,
  ToolProcessWalker,
  readProcTable,
  readToolProcesses,
  toolProcesses,
  type ProcIo,
} from "../src/herdr/process-activity.js";

// MERGE GATE for plan-24/daemon-bg-profile. The background cost of the daemon was cut without changing what it does:
//  - the driver reads a PM's mail only when the ledger changed, and keeps a digest instead of the rows;
//  - the process probe follows the pane's process tree instead of reading every process on the machine;
//  - the daemon starts with a capped young generation.
// drive.mjs runs a real DeliveryDriver for 80 ticks on the generated large ledger (test/fixtures/daemon-cost) with a scripted
// Herdr and a virtual clock; golden.json is what the code before the fixes (git 8b04ed0) did on it. CAPSTAN_DAEMON_BACKGROUND_DIST
// names another build to run the same checks against; the row budget fails on the old one.
const root = path.resolve(import.meta.dirname, "..", "..");
const fixtureDir = path.join(root, "test", "fixtures", "daemon-background");
const distDir =
  process.env.CAPSTAN_DAEMON_BACKGROUND_DIST ??
  path.resolve(import.meta.dirname, "..");

interface Driven {
  result: {
    calls: [number, string, unknown][];
    log: [number, string, unknown][];
  };
  statements: number[];
  rowCounts: number[];
}

test("the driver does what it did before the fixes, with the same Herdr calls, and reads far fewer rows", async () => {
  const dir = fs.mkdtempSync(
    path.join(os.tmpdir(), "capstan-daemon-background-"),
  );
  try {
    const { drive, TICKS } = (await import(
      pathToFileURL(path.join(fixtureDir, "drive.mjs")).href
    )) as {
      drive(dist: string, dir: string): Promise<Driven>;
      TICKS: number;
    };
    const driven = await drive(distDir, dir);
    const golden = JSON.parse(
      fs.readFileSync(path.join(fixtureDir, "golden.json"), "utf8"),
    ) as unknown;
    assert.deepEqual(driven.result, golden, "results match the pre-fix golden");

    // Spawns: one `herdr agent get` per active agent per tick (7 agents), one `pane process-info` per working agent
    // per PROCESS_SAMPLE_MS (3 working agents, 160 s of ticks: at most 3 x 11).
    const count = (kind: string): number =>
      driven.result.calls.filter(([, k]) => k === kind).length;
    const perTick = new Map<number, number>();
    for (const [tick, kind] of driven.result.calls)
      if (kind === "agent_get") perTick.set(tick, (perTick.get(tick) ?? 0) + 1);
    assert.ok(Math.max(...perTick.values()) <= 7, "agent get spawns per tick");
    assert.ok(count("agent_get") <= 7 * TICKS, "agent get spawns in all");
    assert.ok(count("process_info") <= 3 * 11, "process-info spawns in all");

    // Ledger reads: ticks 0-4 and the ticks after a scripted change re-read the PM's mail; the quiet ones must not.
    // Before the fixes every tick read about 1355 rows.
    const changed = new Set([0, 1, 2, 3, 4, 30, 31, 35, 36]);
    const quiet = driven.rowCounts.filter((_, tick) => !changed.has(tick));
    assert.ok(quiet.length > 60);
    assert.ok(Math.max(...quiet) <= 400, "rows per quiet tick");
    assert.ok(Math.max(...driven.statements) <= 260, "statements per tick");
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
});

/** A shell (the pane's pid) with a host under it, a tool shell under that running `sleep`, and a non-shell sibling of the tool shell. */
async function paneTree(): Promise<{ child: ChildProcess; shellPid: number }> {
  const host = `
    const { spawn } = require("node:child_process");
    spawn("sh", ["-c", "sleep 60; :"], { stdio: "ignore" });
    spawn("sleep", ["60"], { stdio: "ignore" });
    setTimeout(() => {}, 60000);
  `;
  const shell = `
    const { spawn } = require("node:child_process");
    spawn(process.execPath, ["-e", ${JSON.stringify(host)}], { stdio: "ignore" });
    setTimeout(() => {}, 60000);
  `;
  const child = spawn(process.execPath, ["-e", shell], {
    detached: true,
    stdio: "ignore",
  });
  const shellPid = child.pid!;
  const deadline = Date.now() + 15_000;
  while (Date.now() < deadline) {
    if (toolProcesses(await readProcTable(), shellPid).length >= 2)
      return { child, shellPid };
    await new Promise((resolve) => setTimeout(resolve, 50));
  }
  process.kill(-shellPid, "SIGKILL");
  throw new Error("the process tree did not come up");
}

test("the process probe finds the same tool processes as a scan of the whole process table, reading a handful of files", async () => {
  const { child, shellPid } = await paneTree();
  try {
    const key = (
      list: readonly {
        pid: number;
        ppid: number;
        comm: string;
        startKey: string;
      }[],
    ) =>
      list
        .map(({ pid, ppid, comm, startKey }) => ({ pid, ppid, comm, startKey }))
        .sort((a, b) => a.pid - b.pid);
    const scan = mock.method(fs.promises, "readFile");
    const table = await readProcTable();
    const wholeTable = toolProcesses(table, shellPid);
    const scanReads = scan.mock.callCount();
    scan.mock.resetCalls();
    const targeted = await readToolProcesses(shellPid);
    const targetedReads = scan.mock.callCount();
    scan.mock.restore();
    assert.ok(targeted !== undefined, "the kernel lists children");
    assert.deepEqual(key(targeted), key(wholeTable));
    assert.deepEqual(
      key(targeted).map((e) => e.comm),
      ["sh", "sleep"],
      "the shell and its sleep are counted; the host's own sleep is not",
    );
    assert.ok(targetedReads <= 30, `targeted reads ${targetedReads}`);
    // The scan reads one file per process, so what it reads grows with the host; the targeted probe reads only the
    // pane's tree, whatever the host runs (a fixed ratio between the two would fail on a small CI runner).
    assert.ok(
      scanReads >= table.length,
      `the table scan read ${scanReads} files for ${table.length} processes`,
    );
    assert.ok(
      targetedReads < table.length || table.length <= 30,
      `the targeted probe read ${targetedReads} files against a table of ${table.length} processes`,
    );
    assert.deepEqual(await readToolProcesses(2_147_483_000), [], "no such pid");
  } finally {
    process.kill(-shellPid, "SIGKILL");
    child.unref();
  }
});

test("the daemon starts with the young-generation cap exactly once", () => {
  const plain = daemonCommand({ CAPSTAN_TOKEN: "x", KEEP: "1" }, "/x/cli.js");
  assert.equal(
    plain.args.filter((a) => a === DAEMON_SEMI_SPACE_FLAG).length,
    1,
  );
  assert.deepEqual(plain.args.slice(-2), ["/x/cli.js", "daemon"]);
  assert.equal(plain.env.CAPSTAN_TOKEN, undefined);
  assert.equal(plain.env.KEEP, "1");
  assert.match(DAEMON_SEMI_SPACE_FLAG, /^--max-semi-space-size=\d+$/);
});

test("the standalone binary's NODE_OPTIONS cap is taken back out by the daemon, keeping the user's own options", () => {
  const spawned = daemonCommand(
    { NODE_OPTIONS: "--no-warnings", CAPSTAN_TOKEN: "x" },
    "/x/cli.js",
    true,
  );
  assert.equal(
    spawned.env.NODE_OPTIONS,
    `--no-warnings ${DAEMON_SEMI_SPACE_FLAG}`,
  );
  assert.equal(spawned.env[ADDED_NODE_OPTION_VARIABLE], DAEMON_SEMI_SPACE_FLAG);
  assert.equal(spawned.env.CAPSTAN_TOKEN, undefined);
  restoreNodeOptions(spawned.env);
  assert.equal(spawned.env.NODE_OPTIONS, "--no-warnings");
  assert.equal(spawned.env[ADDED_NODE_OPTION_VARIABLE], undefined);

  const bare = daemonCommand({}, "/x/cli.js", true);
  assert.equal(bare.env.NODE_OPTIONS, DAEMON_SEMI_SPACE_FLAG);
  restoreNodeOptions(bare.env);
  assert.equal("NODE_OPTIONS" in bare.env, false);

  // The user's own cap is respected: nothing is added, so nothing is taken out.
  const own = daemonCommand(
    { NODE_OPTIONS: "--max-semi-space-size=16" },
    "/x/cli.js",
    true,
  );
  assert.equal(own.env.NODE_OPTIONS, "--max-semi-space-size=16");
  assert.equal(own.env[ADDED_NODE_OPTION_VARIABLE], undefined);
  restoreNodeOptions(own.env);
  assert.equal(own.env.NODE_OPTIONS, "--max-semi-space-size=16");

  // The node run (a flag, not NODE_OPTIONS) sets no marker, and the daemon's restore is then a no-op.
  const plain: NodeJS.ProcessEnv = { NODE_OPTIONS: "--no-warnings" };
  const node = daemonCommand(plain, "/x/cli.js", false);
  assert.equal(node.env.NODE_OPTIONS, "--no-warnings");
  assert.equal(node.env[ADDED_NODE_OPTION_VARIABLE], undefined);
  restoreNodeOptions(plain);
  assert.deepEqual(plain, { NODE_OPTIONS: "--no-warnings" });

  // A stale marker in the parent's environment never reaches the daemon; a changed NODE_OPTIONS is not mangled.
  const stale = daemonCommand(
    { [ADDED_NODE_OPTION_VARIABLE]: "--stale" },
    "/x/cli.js",
    false,
  );
  assert.equal(stale.env[ADDED_NODE_OPTION_VARIABLE], undefined);
  const changed: NodeJS.ProcessEnv = {
    [ADDED_NODE_OPTION_VARIABLE]: DAEMON_SEMI_SPACE_FLAG,
    NODE_OPTIONS: "--something-else",
  };
  restoreNodeOptions(changed);
  assert.equal(changed.NODE_OPTIONS, "--something-else");
});

test("restoreNodeOptions removes only the token the daemon added, wherever it is", () => {
  const flag = DAEMON_SEMI_SPACE_FLAG;
  const restore = (options: string): string | undefined => {
    const env: NodeJS.ProcessEnv = {
      [ADDED_NODE_OPTION_VARIABLE]: flag,
      NODE_OPTIONS: options,
    };
    restoreNodeOptions(env);
    assert.equal(env[ADDED_NODE_OPTION_VARIABLE], undefined);
    return env.NODE_OPTIONS;
  };
  assert.equal(restore(`--a --b ${flag}`), "--a --b");
  assert.equal(restore(`${flag} --a --b`), "--a --b");
  assert.equal(restore(`--a ${flag} --b`), "--a --b");
  assert.equal(restore(`--a   ${flag}   --b  --c`), "--a   --b  --c");
  assert.equal(restore(`  ${flag}  --a`), "--a");
  assert.equal(restore(`--a ${flag} --b ${flag}`), `--a ${flag} --b`);
  assert.equal(restore(`--a ${flag}x --b`), `--a ${flag}x --b`);
  assert.equal(
    restore(`--a --max-semi-space-size=16 ${flag}`),
    "--a --max-semi-space-size=16",
  );
  assert.equal(
    restore(`--max-semi-space-size=16 ${flag} --b`),
    "--max-semi-space-size=16 --b",
  );
  assert.equal(restore(flag), undefined);
  assert.equal(restore(` ${flag} `), undefined);
  // The user's own options stay when the marker is absent.
  const own: NodeJS.ProcessEnv = { NODE_OPTIONS: `--a ${flag} --b` };
  restoreNodeOptions(own);
  assert.equal(own.NODE_OPTIONS, `--a ${flag} --b`);
});

// A heap-statistics check, not a run of the standalone binary (none is built here): node itself stands in for the binary
// and reads the flag from NODE_OPTIONS exactly as a single-executable app does (execArgvExtension "env", the default).
test("a daemon started with the standalone binary's environment has the young-generation cap, and a child it spawns does not", () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "capstan-semi-"));
  try {
    const clientUrl = pathToFileURL(
      path.join(import.meta.dirname, "..", "src", "client.js"),
    ).href;
    const script = path.join(dir, "daemon-stand-in.mjs");
    fs.writeFileSync(
      script,
      `import v8 from "node:v8";
import { spawnSync } from "node:child_process";
let keep;
const started = Date.now();
while (Date.now() - started < 2500) {
  keep = [];
  for (let i = 0; i < 40000; i++) keep.push({ a: i, b: "message body " + i + "x".repeat(100), c: [i, i + 1] });
  await new Promise((resolve) => setImmediate(resolve));
}
const young = v8.getHeapSpaceStatistics().find((s) => s.space_name === "new_space").space_size / 1048576;
const { restoreNodeOptions } = await import(${JSON.stringify(clientUrl)});
restoreNodeOptions();
const child = spawnSync(process.execPath, ["-p", "JSON.stringify([process.env.NODE_OPTIONS ?? null, process.env.${ADDED_NODE_OPTION_VARIABLE} ?? null])"], { encoding: "utf8" });
console.log(JSON.stringify({ young, own: [process.env.NODE_OPTIONS ?? null, process.env.${ADDED_NODE_OPTION_VARIABLE} ?? null], child: JSON.parse(child.stdout) }));
`,
    );
    const run = (env: NodeJS.ProcessEnv) =>
      JSON.parse(
        spawnSync(process.execPath, [script], { env, encoding: "utf8" }).stdout,
      ) as { young: number; own: unknown[]; child: unknown[] };
    const base = { PATH: process.env.PATH ?? "" };
    const control = run(base);
    assert.ok(
      control.young > 16,
      `without the cap the young generation grew to ${control.young} MB`,
    );
    const capped = run(
      daemonCommand(
        { ...base, NODE_OPTIONS: "--no-warnings" },
        "/x/cli.js",
        true,
      ).env,
    );
    assert.ok(
      capped.young <= 8,
      `with the cap the young generation is ${capped.young} MB`,
    );
    assert.deepEqual(
      capped.own,
      ["--no-warnings", null],
      "the daemon's own environment is restored",
    );
    assert.deepEqual(
      capped.child,
      ["--no-warnings", null],
      "a child sees only the user's options",
    );
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
});

/** A scripted /proc: pid 10 is the pane's shell, 11 its host, 12 a tool shell, 13 a process under it. */
function scriptedProc(
  failures: { children?: Error[]; supported?: boolean } = {},
): ProcIo & { calls: number } {
  const tree: Record<number, number[]> = {
    10: [11],
    11: [12],
    12: [13],
    13: [],
  };
  const comm: Record<number, string> = { 11: "node", 12: "sh", 13: "sleep" };
  const io = {
    calls: 0,
    async threads(pid: number) {
      io.calls += 1;
      if (tree[pid] === undefined && pid !== process.pid)
        throw Object.assign(new Error("gone"), { code: "ENOENT" });
      return [String(pid)];
    },
    async children(pid: number, thread: string) {
      io.calls += 1;
      if (failures.supported === false)
        throw Object.assign(new Error("no such file"), { code: "ENOENT" });
      const next = pid === process.pid ? undefined : failures.children?.shift();
      if (next !== undefined) throw next;
      void thread;
      return (tree[pid] ?? []).join(" ");
    },
    async stat(pid: number) {
      io.calls += 1;
      return `${pid} (${comm[pid] ?? "x"}) S ${Object.entries(tree).find(([, kids]) => kids.includes(pid))?.[0] ?? 1} 0 0 0 0 0 0 0 0 0 1 1 0 0 20 0 1 0 ${pid} 0 0`;
    },
  };
  return io;
}

test("the process probe falls back to the whole table for good only when the kernel has no children lists, and retries after any other failure", async () => {
  // No CONFIG_PROC_CHILDREN: ENOENT on the probe's own list, so the whole table is read and the answer is not asked again.
  const missing = scriptedProc({ supported: false });
  const noLists = new ToolProcessWalker(missing, () => 0);
  assert.equal(await noLists.read(10), undefined);
  const calls = missing.calls;
  assert.equal(await noLists.read(10), undefined);
  assert.equal(
    missing.calls,
    calls,
    "no further reads once the kernel is known to have no lists",
  );

  // A transient EMFILE: this sample reads the whole table, nothing is latched, and after the backoff the walker works again.
  let clock = 1_000;
  const flaky = scriptedProc({
    children: [
      Object.assign(new Error("too many open files"), { code: "EMFILE" }),
    ],
  });
  const walker = new ToolProcessWalker(flaky, () => clock);
  assert.equal(
    await walker.read(10),
    undefined,
    "the failed sample reads the table",
  );
  clock += CHILDREN_RETRY_MS - 1;
  const quiet = flaky.calls;
  assert.equal(
    await walker.read(10),
    undefined,
    "inside the backoff it does not try",
  );
  assert.equal(flaky.calls, quiet);
  clock += 1;
  const found = await walker.read(10);
  assert.deepEqual(
    found?.map((e) => [e.pid, e.comm]),
    [
      [12, "sh"],
      [13, "sleep"],
    ],
    "after the backoff it follows the lists again",
  );
});

// The call has to come before the first thing that reads the config or captures the environment for a child
// (createHerdrRunner copies it when it is built), so the order inside the daemon command is what is checked.
test("the daemon command takes the cap out of NODE_OPTIONS before it loads the project or builds the Herdr runner", () => {
  const source = fs.readFileSync(path.join(root, "src", "cli.ts"), "utf8");
  const branch = source.slice(source.indexOf('if (command === "daemon") {'));
  const restore = branch.indexOf("restoreNodeOptions()");
  assert.ok(restore > 0, "the daemon command restores NODE_OPTIONS");
  assert.ok(restore < branch.indexOf("loadOperator("), "before loadOperator");
  assert.ok(
    restore < branch.indexOf("createHerdrRunner("),
    "before the Herdr runner copies the environment",
  );
});
