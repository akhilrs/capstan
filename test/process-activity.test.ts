import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import path from "node:path";
import { test } from "node:test";
import {
  HerdrProcessProbe,
  MIN_CHILD_CPU_MS,
  ProcessActivityTracker,
  parseProcStat,
  parsePsTable,
  parsePsTime,
  toolProcesses,
  type ProcessEntry,
  type ProcessTable,
} from "../src/herdr/process-activity.js";
import type { HerdrRunner } from "../src/herdr/runner.js";

const SHELL_PID = 31358;
const p = (
  pid: number,
  ppid: number,
  comm: string,
  cpuMs = 0,
  startKey = String(pid),
): ProcessEntry => ({ pid, ppid, comm, cpuMs, startKey });

/** Modelled on the live capture: bash -> claude -> {npm exec MCP, codebase-memory, zsh -> npm test -> node}. */
function table(testCpu = 0): ProcessTable {
  return [
    p(1, 0, "systemd"),
    p(SHELL_PID, 1, "bash"),
    p(100, SHELL_PID, "claude", 5000),
    p(101, 100, "npm exec", 300),
    p(102, 100, "codebase-memory", 400),
    p(103, 100, "zsh", 10),
    p(104, 103, "npm test", 20),
    p(105, 104, "node", testCpu),
    p(200, 1, "node", 99999),
  ];
}

test("only the shell subtree under the host process is counted", () => {
  assert.deepEqual(
    toolProcesses(table(), SHELL_PID).map((e) => e.pid),
    [103, 104, 105],
  );
  assert.deepEqual(toolProcesses(table(), 4242), []);
});

test("a CPU increase of at least the minimum marks activity and a flat total does not", () => {
  const tracker = new ProcessActivityTracker();
  const sample = (cpu: number) => ({
    processes: toolProcesses(table(cpu), SHELL_PID),
  });
  tracker.record("dev-1", sample(0), 500);
  tracker.record("dev-1", sample(0), 1000);
  assert.equal(tracker.lastChildActivity("dev-1"), undefined);
  tracker.record("dev-1", sample(MIN_CHILD_CPU_MS - 1), 2000);
  assert.equal(tracker.lastChildActivity("dev-1"), undefined);
  tracker.record("dev-1", sample(MIN_CHILD_CPU_MS - 1 + 250), 3000);
  assert.equal(tracker.lastChildActivity("dev-1"), 3000);
  tracker.record("dev-1", sample(MIN_CHILD_CPU_MS - 1 + 250), 4000);
  assert.equal(tracker.lastChildActivity("dev-1"), 3000, "flat CPU keeps it");
  tracker.forget("dev-1");
  assert.equal(tracker.lastChildActivity("dev-1"), undefined);
});

test("an exited pid, a new pid with fresh CPU and a reused pid are handled", () => {
  const tracker = new ProcessActivityTracker();
  tracker.record("dev-1", { processes: [p(1, 0, "zsh", 1000, "a")] }, 500);
  assert.equal(
    tracker.lastChildActivity("dev-1"),
    undefined,
    "the first sample is a baseline",
  );
  tracker.record("dev-1", { processes: [p(1, 0, "zsh", 1200, "a")] }, 1000);
  // The process exited: CPU cannot go down into activity.
  tracker.record("dev-1", { processes: [] }, 2000);
  assert.equal(tracker.lastChildActivity("dev-1"), 1000);
  // A new pid counts its full CPU.
  tracker.record("dev-1", { processes: [p(2, 0, "zsh", 500, "b")] }, 3000);
  assert.equal(tracker.lastChildActivity("dev-1"), 3000);
  // Same pid, flat CPU: nothing.
  tracker.record("dev-1", { processes: [p(2, 0, "zsh", 500, "b")] }, 4000);
  assert.equal(tracker.lastChildActivity("dev-1"), 3000);
  // Same pid, different start key: a new process, its CPU is new.
  tracker.record("dev-1", { processes: [p(2, 0, "zsh", 400, "c")] }, 5000);
  assert.equal(tracker.lastChildActivity("dev-1"), 5000);
  // A decrease on the same process never counts.
  tracker.record("dev-1", { processes: [p(2, 0, "zsh", 100, "c")] }, 6000);
  assert.equal(tracker.lastChildActivity("dev-1"), 5000);
});

test("the probe reads shell_pid from the recorded herdr output and counts the tool subtree", async () => {
  const stdout = readFileSync(
    path.resolve("test/fixtures/herdr-process-info.json"),
    "utf8",
  ).replace(/\n$/, "");
  const calls: string[][] = [];
  const runner: HerdrRunner = async (args) => {
    calls.push([...args]);
    return { code: 0, stdout, stderr: "" };
  };
  const shell = (
    JSON.parse(stdout) as {
      result: { process_info: { shell_pid: number } };
    }
  ).result.process_info.shell_pid;
  const probe = new HerdrProcessProbe(runner, async () => [
    p(shell, 1, "bash"),
    p(shell + 1, shell, "claude"),
    p(shell + 2, shell + 1, "zsh", 700),
  ]);
  const sample = await probe.sample("w39:p1");
  assert.deepEqual(calls, [["pane", "process-info", "--pane", "w39:p1"]]);
  assert.deepEqual(
    sample.processes.map((e) => e.pid),
    [shell + 2],
  );
});

test("a malformed or missing shell_pid fails the sample", async () => {
  for (const body of [
    '{"result":{"process_info":{"pane_id":"x"},"type":"pane_process_info"}}',
    '{"result":{"process_info":{"shell_pid":"12"}}}',
    '{"result":{"process_info":{"shell_pid":-1}}}',
    '{"result":{}}',
    "not json",
  ]) {
    const probe = new HerdrProcessProbe(
      async () => ({ code: 0, stdout: body, stderr: "" }),
      async () => [],
    );
    await assert.rejects(probe.sample("w1:p1"), body);
  }
});

test("the ps parser reads the macOS capture into the table shape", () => {
  const text = readFileSync(path.resolve("test/fixtures/ps-macos.txt"), "utf8");
  const parsed = parsePsTable(text);
  assert.deepEqual(
    parsed.map((e) => [e.pid, e.ppid, e.cpuMs, e.comm]),
    [
      [1, 0, 12_340, "/sbin/launchd"],
      [100, 1, ((24 + 2) * 3600 + 3 * 60 + 4) * 1000, "/bin/zsh"],
      [
        101,
        100,
        1_500,
        "/Applications/Visual Studio Code.app/Contents/MacOS/Electron",
      ],
      [102, 101, (12 * 3600 + 34 * 60 + 56) * 1000, "npm test"],
    ],
  );
  assert.equal(parsed[3]!.startKey, "npm test");
  assert.equal(parsePsTime("1:02"), 62_000);
  assert.equal(parsePsTime("garbage"), undefined);
});

test("the /proc parser reads ppid, comm with spaces, CPU and start time", () => {
  const stat =
    "4242 (my (odd) cmd) S 100 4242 4242 0 -1 4194560 1 0 0 0 150 50 0 0 20 0 1 0 987654 1000 10 18446744073709551615";
  assert.deepEqual(parseProcStat(4242, stat), {
    pid: 4242,
    ppid: 100,
    comm: "my (odd) cmd",
    cpuMs: 2000,
    startKey: "987654",
  });
  assert.equal(parseProcStat(1, "garbage"), undefined);
});

test("CPU of exited children (cutime, cstime) counts, so an idle parent with growing child totals is active", () => {
  const stat = (cutime: number, cstime: number) =>
    `50 (node) S 40 50 50 0 -1 0 0 0 0 0 10 5 ${cutime} ${cstime} 20 0 1 0 777 1000 10`;
  const read = (cutime: number, cstime: number) => ({
    processes: [parseProcStat(50, stat(cutime, cstime))!],
  });
  assert.equal(read(100, 50).processes[0]!.cpuMs, 1650);
  const tracker = new ProcessActivityTracker();
  tracker.record("dev-1", read(0, 0), 1000);
  tracker.record("dev-1", read(0, 0), 2000);
  assert.equal(tracker.lastChildActivity("dev-1"), undefined, "all four flat");
  tracker.record("dev-1", read(30, 0), 3000);
  assert.equal(tracker.lastChildActivity("dev-1"), 3000);
});
