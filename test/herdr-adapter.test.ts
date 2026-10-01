import assert from "node:assert/strict";
import {
  existsSync,
  mkdirSync,
  readdirSync,
  mkdtempSync,
  readFileSync,
  realpathSync,
  rmSync,
  statSync,
} from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { test } from "node:test";
import {
  AgentPaneMismatch,
  ClearFailed,
  DeferralNotElapsed,
  DialogStillOpen,
  HerdrAdapter,
  INPUT_UNREADABLE_DETAIL,
  InputUnreadable,
  InvalidArgumentError,
  NotBlocked,
  PaneGone,
  NotIdle,
  PhaseError,
  PmPaneError,
  SendAfterRecordError,
  PromptUnrecognized,
  ShellNotReady,
  UnknownPaneError,
  UnsupportedHostError,
  herdrStateOf,
  isAgentName,
  buildAgentEnvironment,
  claudeArguments,
  type KeyLogEntry,
} from "../src/herdr/adapter.js";
import { HerdrError, type HerdrResult } from "../src/herdr/runner.js";
import { TRUST_NO, TRUST_YES } from "../src/herdr/screen.js";

const RULE = "─".repeat(40);
const NBSP = " ";
const DIM = "\u001b[2m";
const RESET = "\u001b[0m";
const TOKEN = "tok_ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";

const fixture = (name: string): string =>
  readFileSync(path.resolve("test/fixtures", name), "utf8");

function idleScreen(...typed: string[]): string {
  const first =
    typed.length === 0
      ? `❯${NBSP}${RESET}${DIM}Try "x"${RESET}`
      : `❯${NBSP}${typed[0]}`;
  return [
    "",
    RULE,
    first,
    ...typed.slice(1).map((line) => `  ${line}`),
    RULE,
    "  footer",
  ].join("\r\n");
}

const SHELL_READY = "user in dir\r\n❯ ";
const CLEAN_ENV = { HOME: "/h", PATH: "/p", TERM: "t" };

interface FakePane {
  paneId: string;
  workspaceId: string;
  agent?: string;
  status: string;
  screen: string;
  checkout: string;
}

class FakeHerdr {
  readonly calls: string[][] = [];
  readonly events: string[] = [];
  readonly panes = new Map<string, FakePane>();
  readonly agentStates = new Map<
    string,
    { paneId: string; statuses: string[] }
  >();
  readonly root = realpathSync(
    mkdtempSync(path.join(tmpdir(), "capstan-fake-herdr-")),
  );
  startError: { code: string; message: string } | undefined;
  notification: Record<string, unknown> = { shown: true };
  onKey: ((pane: FakePane, key: string) => void) | undefined;
  onRun: ((pane: FakePane, command: string) => void) | undefined = (
    pane,
    command,
  ) => {
    const rc = /--rcfile '([^']+)'/.exec(command);
    if (rc) {
      rmSync(path.dirname(rc[1]!), { recursive: true, force: true });
      pane.screen = "❯ ";
    }
  };
  private counter = 0;

  cleanup(): void {
    rmSync(this.root, { recursive: true, force: true });
  }

  callsTo(command: string, sub?: string): string[][] {
    return this.calls.filter(
      (call) => call[0] === command && (sub === undefined || call[1] === sub),
    );
  }

  add(role: string, screen: string, status = "unknown"): FakePane {
    this.counter += 1;
    const pane: FakePane = {
      paneId: `w${this.counter}:p1`,
      workspaceId: `w${this.counter}`,
      status,
      screen,
      checkout: path.join(this.root, `${role}-${this.counter}`),
    };
    mkdirSync(pane.checkout, { recursive: true });
    this.panes.set(pane.paneId, pane);
    return pane;
  }

  private json(result: unknown): HerdrResult {
    return { code: 0, stdout: JSON.stringify({ id: "x", result }), stderr: "" };
  }

  private failure(code: string, message: string): HerdrResult {
    return {
      code: 1,
      stdout: JSON.stringify({ id: "x", error: { code, message } }),
      stderr: "",
    };
  }

  readonly run = async (args: readonly string[]): Promise<HerdrResult> => {
    this.calls.push([...args]);
    const [command, sub] = args;
    if (command === "--version")
      return { code: 0, stdout: "herdr 0.9.1\n", stderr: "" };
    const flag = (name: string): string | undefined => {
      const at = args.indexOf(name);
      return at < 0 ? undefined : args[at + 1];
    };
    if (command === "worktree" && sub === "create") {
      const pane = this.add("worker", SHELL_READY);
      return this.json({
        root_pane: { pane_id: pane.paneId },
        workspace: {
          workspace_id: pane.workspaceId,
          worktree: { checkout_path: pane.checkout },
        },
      });
    }
    if (command === "workspace" && sub === "create") {
      const pane = this.add("workspace", SHELL_READY);
      return this.json({
        root_pane: { pane_id: pane.paneId },
        workspace: { workspace_id: pane.workspaceId },
      });
    }
    if (command === "worktree" && sub === "remove")
      return this.json({ forced: true });
    if (command === "pane" && sub === "get") {
      const pane = this.panes.get(args[2]!);
      if (!pane) return this.failure("pane_not_found", "no such pane");
      return this.json({
        pane: {
          pane_id: pane.paneId,
          agent_status: pane.status,
          ...(pane.agent ? { agent: "claude" } : {}),
        },
      });
    }
    if (command === "agent" && sub === "get") {
      const state = this.agentStates.get(args[2]!);
      if (!state) return this.failure("agent_not_found", "no such agent");
      const status =
        state.statuses.length > 1
          ? state.statuses.shift()!
          : state.statuses[0]!;
      return this.json({
        agent: { name: args[2], pane_id: state.paneId, agent_status: status },
      });
    }
    if (command === "pane" && sub === "read") {
      const pane = this.panes.get(args[2]!);
      if (!pane) return this.failure("pane_not_found", "no such pane");
      return { code: 0, stdout: pane.screen, stderr: "" };
    }
    if (command === "pane" && sub === "send-keys") {
      const pane = this.panes.get(args[2]!)!;
      this.events.push(`key:${args[3]}`);
      this.onKey?.(pane, args[3]!);
      return { code: 0, stdout: "", stderr: "" };
    }
    if (command === "pane" && sub === "run") {
      const pane = this.panes.get(args[2]!)!;
      this.events.push("run");
      this.onRun?.(pane, args[3]!);
      return { code: 0, stdout: "", stderr: "" };
    }
    if (command === "agent" && sub === "prompt") {
      this.events.push(`prompt:${args[3]}`);
      return this.json({});
    }
    if (command === "agent" && sub === "start") {
      const name = args[2]!;
      const pane = this.panes.get(flag("--pane")!)!;
      if (this.startError)
        return this.failure(this.startError.code, this.startError.message);
      pane.agent = name;
      pane.status = "idle";
      this.agentStates.set(name, { paneId: pane.paneId, statuses: ["idle"] });
      return this.json({ agent: { name } });
    }
    if (command === "pane" && sub === "close") return this.json({});
    if (command === "notification" && sub === "show")
      return this.json(this.notification);
    return this.failure("unknown", `unhandled ${args.join(" ")}`);
  };
}

interface Harness {
  readonly fake: FakeHerdr;
  readonly adapter: HerdrAdapter;
}

function harness(): Harness {
  const fake = new FakeHerdr();
  let clock = 0;
  const adapter = new HerdrAdapter({
    run: fake.run,
    tempRoot: fake.root,
    sleep: async () => {
      clock += 100;
    },
    now: () => clock,
    pollMs: 100,
  });
  return { fake, adapter };
}

async function startedWorker(
  h: Harness,
  screen = idleScreen(),
  statuses = ["idle"],
): Promise<{ paneId: string; agent: string; pane: FakePane }> {
  const { paneId } = await h.adapter.createWorktree({
    workspaceId: "w9",
    branch: "cap/task/dev-g1",
    label: "dev",
  });
  await h.adapter.startAgent({
    name: "dev",
    kind: "claude",
    paneId,
    args: [],
    environment: CLEAN_ENV,
  });
  const pane = h.fake.panes.get(paneId)!;
  pane.screen = screen;
  h.fake.agentStates.set("dev", { paneId, statuses });
  h.fake.calls.length = 0;
  h.fake.events.length = 0;
  return { paneId, agent: "dev", pane };
}

test("the adapter reads the Herdr version and creates worktrees and workspaces as registered fresh panes", async () => {
  const h = harness();
  try {
    assert.equal(await h.adapter.version(), "herdr 0.9.1");
    const worktree = await h.adapter.createWorktree({
      workspaceId: "w9",
      branch: "cap/task/dev-g1",
      label: "dev",
      base: "main",
    });
    assert.deepEqual(h.fake.callsTo("worktree", "create")[0], [
      "worktree",
      "create",
      "--workspace",
      "w9",
      "--branch",
      "cap/task/dev-g1",
      "--label",
      "dev",
      "--base",
      "main",
      "--no-focus",
    ]);
    assert.equal(worktree.path, h.fake.panes.get(worktree.paneId)!.checkout);
    assert.deepEqual(h.adapter.paneEntry(worktree.paneId), {
      role: "worker",
      phase: "fresh",
      kind: "shell",
      worktreePath: worktree.path,
      workspaceId: worktree.workspaceId,
    });
    const pm = await h.adapter.createWorkspace({
      cwd: h.fake.root,
      label: "pm",
      role: "PM",
    });
    assert.equal(h.adapter.paneEntry(pm.paneId)!.role, "PM");
    assert.deepEqual(await h.adapter.paneState(pm.paneId), {
      status: "unknown",
    });
    for (const bad of [
      { workspaceId: "x9", branch: "b", label: "l" },
      { workspaceId: "w9", branch: "../evil", label: "l" },
      { workspaceId: "w9", branch: "b", label: "has space" },
      { workspaceId: "w9", branch: "b", label: "l", base: "a b" },
    ])
      await assert.rejects(h.adapter.createWorktree(bad), InvalidArgumentError);
    await assert.rejects(
      h.adapter.createWorkspace({ cwd: "relative", label: "x", role: "PM" }),
      InvalidArgumentError,
    );
    await h.adapter.removeWorktree(worktree.workspaceId, { force: true });
    assert.equal(h.adapter.paneEntry(worktree.paneId), undefined);
    assert.deepEqual(h.fake.callsTo("worktree", "remove").at(-1), [
      "worktree",
      "remove",
      "--workspace",
      worktree.workspaceId,
      "--force",
    ]);
    await h.adapter.closePane(pm.paneId);
    assert.equal(h.adapter.paneEntry(pm.paneId), undefined);
  } finally {
    h.adapter.close();
    h.fake.cleanup();
  }
});

test("no input method makes a Herdr call for an unregistered pane, a PM pane past its launch, or a fresh worker", async () => {
  const h = harness();
  try {
    const noop = async (): Promise<void> => {};
    const attempts = (paneId: string) => ({
      send: () =>
        h.adapter.guardedSend({ paneId, text: "hi", beforeSend: noop }),
      clear: () =>
        h.adapter.clearAfterDeferral({
          paneId,
          deferredForMs: 9e9,
          maxDeferralMs: 1,
          discard: noop,
          log: noop,
        }),
      dialog: () => h.adapter.answerTrustDialog({ paneId, log: noop }),
      prepare: () =>
        h.adapter.prepareShell({
          paneId,
          environment: { HOME: "/h", PATH: "/p", TERM: "t" },
        }),
      start: () =>
        h.adapter.startAgent({
          name: "x",
          kind: "claude",
          paneId,
          args: [],
          environment: CLEAN_ENV,
        }),
    });
    const before = () => h.fake.calls.length;

    const stranger = h.fake.add("stranger", SHELL_READY).paneId;
    const at = before();
    for (const run of Object.values(attempts(stranger)))
      await assert.rejects(run, UnknownPaneError);
    assert.equal(before(), at, "an unregistered pane gets no Herdr call");

    const worker = (
      await h.adapter.createWorktree({
        workspaceId: "w9",
        branch: "b",
        label: "w",
      })
    ).paneId;
    const workerAt = before();
    for (const name of ["send", "clear", "dialog"] as const)
      await assert.rejects(attempts(worker)[name], PhaseError);
    assert.equal(before(), workerAt, "a fresh worker pane accepts no messages");

    const pm = await h.adapter.createWorkspace({
      cwd: h.fake.root,
      label: "pm",
      role: "PM",
    });
    const pmAt = before();
    for (const name of ["send", "clear", "dialog"] as const)
      await assert.rejects(attempts(pm.paneId)[name], PmPaneError);
    assert.equal(
      before(),
      pmAt,
      "a PM pane never gets a message, a clear or a dialog answer",
    );

    await h.adapter.startAgent({
      name: "pm",
      kind: "claude",
      paneId: pm.paneId,
      args: [],
      environment: CLEAN_ENV,
    });
    assert.equal(h.adapter.paneEntry(pm.paneId)!.phase, "started");
    const started = before();
    for (const run of Object.values(attempts(pm.paneId)))
      await assert.rejects(run, PmPaneError);
    assert.equal(
      before(),
      started,
      "a started PM pane refuses every input method with no Herdr call",
    );
    assert.equal(
      h.fake.events.filter(
        (event) => event.startsWith("prompt") || event.startsWith("key"),
      ).length,
      0,
    );
  } finally {
    h.adapter.close();
    h.fake.cleanup();
  }
});

const STATUS_CASES: ReadonlyArray<[string, string | undefined]> = [
  ["idle", undefined],
  ["done", undefined],
  ["working", "agent_busy"],
  ["blocked", "agent_blocked"],
  ["unknown", "agent_busy"],
  ["something-new", "agent_busy"],
];
const INPUT_CASES: ReadonlyArray<[string, string, string | undefined]> = [
  ["empty", idleScreen(), undefined],
  ["typed", idleScreen("half typed"), "input_not_empty"],
  ["multi-line", idleScreen("a", "b"), "input_not_empty"],
  ["unreadable", "no input box here", "input_not_empty"],
];

test("guarded send delivers only when the state is idle or done and the input is empty; every other case defers and sends nothing", async () => {
  for (const [status, statusReason] of STATUS_CASES)
    for (const [label, screen, inputReason] of INPUT_CASES) {
      const h = harness();
      try {
        const worker = await startedWorker(h, screen, [status]);
        let recorded = 0;
        const outcome = await h.adapter.guardedSend({
          paneId: worker.paneId,
          text: "do the thing",
          beforeSend: () => {
            recorded += 1;
          },
        });
        const expected = statusReason ?? inputReason;
        if (expected === undefined) {
          assert.deepEqual(outcome, { sent: true }, `${status}/${label}`);
          assert.equal(recorded, 1);
          assert.equal(h.fake.callsTo("agent", "prompt").length, 1);
        } else {
          assert.equal(outcome.sent, false, `${status}/${label}`);
          assert.equal(
            (outcome as { reason: string }).reason,
            expected,
            `${status}/${label}`,
          );
          if (statusReason === undefined && label === "unreadable")
            assert.equal(
              (outcome as { detail?: string }).detail,
              "the input line is unreadable",
            );
          assert.equal(recorded, 0, "nothing is recorded for a deferral");
          assert.equal(
            h.fake.callsTo("agent", "prompt").length,
            0,
            `${status}/${label} sent`,
          );
          if (statusReason !== undefined)
            assert.equal(
              h.fake.callsTo("pane", "read").length,
              0,
              "a busy agent's screen is not even read",
            );
        }
      } finally {
        h.adapter.close();
        h.fake.cleanup();
      }
    }
});

test("the state is checked again after the screen read and a change to working stops the send", async () => {
  const h = harness();
  try {
    const worker = await startedWorker(h, idleScreen(), ["idle", "working"]);
    let recorded = 0;
    const outcome = await h.adapter.guardedSend({
      paneId: worker.paneId,
      text: "x",
      beforeSend: () => {
        recorded += 1;
      },
    });
    assert.deepEqual(outcome, { sent: false, reason: "agent_busy" });
    assert.equal(recorded, 0);
    assert.equal(h.fake.callsTo("agent", "prompt").length, 0);
    assert.equal(h.fake.callsTo("agent", "get").length, 2);
  } finally {
    h.adapter.close();
    h.fake.cleanup();
  }
});

test("a name that no longer points at the registered pane is refused before anything is typed", async () => {
  const h = harness();
  try {
    const worker = await startedWorker(h);
    h.fake.agentStates.set("dev", { paneId: "w99:p1", statuses: ["idle"] });
    await assert.rejects(
      h.adapter.guardedSend({
        paneId: worker.paneId,
        text: "x",
        beforeSend: () => {},
      }),
      AgentPaneMismatch,
    );
    assert.equal(h.fake.callsTo("agent", "prompt").length, 0);
    h.fake.agentStates.set("dev", {
      paneId: worker.paneId,
      statuses: ["idle", "idle"],
    });
    h.fake.agentStates.get("dev")!.statuses = ["idle"];
    const second = h.fake.agentStates.get("dev")!;
    let calls = 0;
    const original = h.fake.run;
    const flip = async (args: readonly string[]): Promise<HerdrResult> => {
      const result = await original(args);
      if (args[0] === "agent" && args[1] === "get") {
        calls += 1;
        if (calls === 1) second.paneId = "w98:p1";
      }
      return result;
    };
    const adapter = new HerdrAdapter({
      run: flip,
      tempRoot: h.fake.root,
      sleep: async () => {},
      now: () => 0,
    });
    const other = await adapter.createWorktree({
      workspaceId: "w9",
      branch: "b2",
      label: "d2",
    });
    await adapter.startAgent({
      name: "dev2",
      kind: "claude",
      paneId: other.paneId,
      args: [],
      environment: CLEAN_ENV,
    });
    h.fake.panes.get(other.paneId)!.screen = idleScreen();
    h.fake.agentStates.set("dev2", second);
    second.paneId = other.paneId;
    calls = 0;
    await assert.rejects(
      adapter.guardedSend({
        paneId: other.paneId,
        text: "x",
        beforeSend: () => {},
      }),
      AgentPaneMismatch,
    );
    assert.equal(h.fake.callsTo("agent", "prompt").length, 0);
  } finally {
    h.adapter.close();
    h.fake.cleanup();
  }
});

test("the record happens before the physical send, and a failing record sends nothing", async () => {
  const h = harness();
  try {
    const worker = await startedWorker(h);
    const order: string[] = [];
    const original = h.fake.run;
    const traced = async (args: readonly string[]): Promise<HerdrResult> => {
      if (args[0] === "agent" && args[1] === "prompt") order.push("prompt");
      return original(args);
    };
    const adapter = new HerdrAdapter({
      run: traced,
      tempRoot: h.fake.root,
      sleep: async () => {},
      now: () => 0,
    });
    const entry = await adapter.createWorktree({
      workspaceId: "w9",
      branch: "b3",
      label: "d3",
    });
    await adapter.startAgent({
      name: "dev3",
      kind: "claude",
      paneId: entry.paneId,
      args: [],
      environment: CLEAN_ENV,
    });
    h.fake.panes.get(entry.paneId)!.screen = idleScreen();
    h.fake.agentStates.set("dev3", {
      paneId: entry.paneId,
      statuses: ["idle"],
    });
    const outcome = await adapter.guardedSend({
      paneId: entry.paneId,
      text: "hello",
      beforeSend: async () => {
        order.push("record");
      },
    });
    assert.deepEqual(outcome, { sent: true });
    assert.deepEqual(order, ["record", "prompt"]);
    order.length = 0;
    await assert.rejects(
      adapter.guardedSend({
        paneId: entry.paneId,
        text: "hello",
        beforeSend: () => {
          throw new Error("ledger down");
        },
      }),
      /ledger down/,
    );
    assert.deepEqual(order, []);
    void worker;
  } finally {
    h.adapter.close();
    h.fake.cleanup();
  }
});

test("guarded send validates the text", async () => {
  const h = harness();
  try {
    const worker = await startedWorker(h);
    for (const text of ["", "a\u0000b", "x".repeat(16 * 1024 + 1), 5 as never])
      await assert.rejects(
        h.adapter.guardedSend({
          paneId: worker.paneId,
          text,
          beforeSend: () => {},
        }),
        InvalidArgumentError,
      );
    assert.equal(h.fake.calls.length, 0);
  } finally {
    h.adapter.close();
    h.fake.cleanup();
  }
});

function ctrlUClearsOneLine(lines: { value: string[] }, pane: FakePane) {
  return (target: FakePane, key: string): void => {
    if (target !== pane || key !== "ctrl+u") return;
    const last = lines.value.length - 1;
    if (lines.value[last] !== "") lines.value[last] = "";
    else if (lines.value.length > 1) lines.value.pop();
    pane.screen = lines.value.every((line) => line === "")
      ? idleScreen()
      : idleScreen(...lines.value);
  };
}

test("the clear waits for the deferral, needs an idle agent, logs the text before the key and checks the line afterwards", async () => {
  const h = harness();
  try {
    const worker = await startedWorker(h, idleScreen("half typed"));
    const lines = { value: ["half typed"] };
    h.fake.onKey = ctrlUClearsOneLine(lines, worker.pane);
    const order: string[] = [];
    const original = h.fake.run;
    const traced = async (args: readonly string[]): Promise<HerdrResult> => {
      if (args[0] === "pane" && args[1] === "send-keys")
        order.push(`sent:${args[3]}`);
      return original(args);
    };
    const adapter = new HerdrAdapter({
      run: traced,
      tempRoot: h.fake.root,
      sleep: async () => {},
      now: () => 0,
    });
    const entry = await adapter.createWorktree({
      workspaceId: "w9",
      branch: "c1",
      label: "c1",
    });
    await adapter.startAgent({
      name: "dev-c",
      kind: "claude",
      paneId: entry.paneId,
      args: [],
      environment: CLEAN_ENV,
    });
    const pane = h.fake.panes.get(entry.paneId)!;
    pane.screen = idleScreen("half typed");
    h.fake.agentStates.set("dev-c", {
      paneId: entry.paneId,
      statuses: ["idle"],
    });
    h.fake.onKey = ctrlUClearsOneLine(lines, pane);
    const log = (logEntry: KeyLogEntry): void => {
      order.push(`log:${logEntry.key}`);
    };
    const discard = (text: string): void => {
      order.push(`discard:${text}`);
    };
    const callsBefore = h.fake.calls.length;
    await assert.rejects(
      adapter.clearAfterDeferral({
        paneId: entry.paneId,
        deferredForMs: 99,
        maxDeferralMs: 100,
        discard,
        log,
      }),
      DeferralNotElapsed,
    );
    assert.equal(
      h.fake.calls.length,
      callsBefore,
      "too early: no Herdr call at all",
    );
    assert.deepEqual(
      await adapter.clearAfterDeferral({
        paneId: entry.paneId,
        deferredForMs: 100,
        maxDeferralMs: 100,
        discard,
        log,
      }),
      {
        cleared: true,
        text: "half typed",
      },
    );
    assert.deepEqual(order, [
      "discard:half typed",
      "log:ctrl+u",
      "sent:ctrl+u",
    ]);
    assert.deepEqual(
      await adapter.clearAfterDeferral({
        paneId: entry.paneId,
        deferredForMs: 500,
        maxDeferralMs: 100,
        discard,
        log,
      }),
      {
        cleared: false,
        text: "",
      },
    );
    void worker;
  } finally {
    h.adapter.close();
    h.fake.cleanup();
  }
});

test("the clear refuses a working or blocked agent and an unreadable line, and stops if discard or the log fails", async () => {
  for (const status of ["working", "blocked", "unknown"]) {
    const h = harness();
    try {
      const worker = await startedWorker(h, idleScreen("typed"), [status]);
      await assert.rejects(
        h.adapter.clearAfterDeferral({
          paneId: worker.paneId,
          deferredForMs: 9e9,
          maxDeferralMs: 1,
          discard: () => {},
          log: () => {},
        }),
        NotIdle,
      );
      assert.equal(h.fake.events.length, 0);
    } finally {
      h.adapter.close();
      h.fake.cleanup();
    }
  }
  const h = harness();
  try {
    const worker = await startedWorker(h, "nothing readable");
    await assert.rejects(
      h.adapter.clearAfterDeferral({
        paneId: worker.paneId,
        deferredForMs: 9e9,
        maxDeferralMs: 1,
        discard: () => {},
        log: () => {},
      }),
      InputUnreadable,
    );
    worker.pane.screen = idleScreen("typed text");
    await assert.rejects(
      h.adapter.clearAfterDeferral({
        paneId: worker.paneId,
        deferredForMs: 9e9,
        maxDeferralMs: 1,
        discard: () => {
          throw new Error("ledger down");
        },
        log: () => {},
      }),
      /ledger down/,
    );
    await assert.rejects(
      h.adapter.clearAfterDeferral({
        paneId: worker.paneId,
        deferredForMs: 9e9,
        maxDeferralMs: 1,
        discard: () => {},
        log: () => {
          throw new Error("log down");
        },
      }),
      /log down/,
    );
    assert.equal(h.fake.events.length, 0, "no key was ever sent");
  } finally {
    h.adapter.close();
    h.fake.cleanup();
  }
});

test("multi-line input takes several rounds and a line that never clears ends in ClearFailed after five", async () => {
  const h = harness();
  try {
    const worker = await startedWorker(h, idleScreen("one", "two"));
    const lines = { value: ["one", "two"] };
    h.fake.onKey = ctrlUClearsOneLine(lines, worker.pane);
    const keys: string[] = [];
    let discarded = "";
    const cleared = await h.adapter.clearAfterDeferral({
      paneId: worker.paneId,
      deferredForMs: 1,
      maxDeferralMs: 1,
      discard: (text) => {
        discarded = text;
      },
      log: (entry) => {
        keys.push(entry.key);
      },
    });
    assert.equal(cleared.cleared, true);
    assert.equal(discarded, "one\n  two");
    assert.deepEqual(keys, ["ctrl+u", "ctrl+u", "ctrl+u"]);

    worker.pane.screen = idleScreen("stuck");
    h.fake.onKey = undefined;
    keys.length = 0;
    await assert.rejects(
      h.adapter.clearAfterDeferral({
        paneId: worker.paneId,
        deferredForMs: 1,
        maxDeferralMs: 1,
        discard: () => {},
        log: (entry) => {
          keys.push(entry.key);
        },
      }),
      ClearFailed,
    );
    assert.equal(keys.length, 5);
  } finally {
    h.adapter.close();
    h.fake.cleanup();
  }
});

function dialogScreen(checkout: string, selected: "no" | "yes" = "no"): string {
  const text = fixture("claude-trust-dialog.txt").replace(
    "/home/user/.herdr/worktrees/probe-repo-FQ8H/probe-two",
    checkout,
  );
  return selected === "no"
    ? text
    : text
        .replace(`❯ ${TRUST_NO}`, `  ${TRUST_NO}`)
        .replace(`  ${TRUST_YES}`, `❯ ${TRUST_YES}`);
}

function dialogKeys(
  pane: FakePane,
  checkout: string,
): (target: FakePane, key: string) => void {
  let selected: "no" | "yes" = pane.screen.includes(`❯ ${TRUST_YES}`)
    ? "yes"
    : "no";
  return (target, key) => {
    if (target !== pane) return;
    if (key === "down") selected = "yes";
    if (key === "up") selected = "no";
    if (key === "enter") {
      pane.status = "idle";
      pane.screen =
        selected === "yes" ? idleScreen() : dialogScreen(checkout, selected);
      if (selected === "yes") return;
    } else pane.screen = dialogScreen(checkout, selected);
  };
}

async function blockedWorker(h: Harness, selected: "no" | "yes" = "no") {
  const worker = await startedWorker(h);
  const checkout = h.adapter.paneEntry(worker.paneId)!.worktreePath!;
  worker.pane.screen = dialogScreen(checkout, selected);
  worker.pane.status = "blocked";
  h.fake.agentStates.set("dev", {
    paneId: worker.paneId,
    statuses: ["blocked"],
  });
  h.fake.onKey = dialogKeys(worker.pane, checkout);
  return { ...worker, checkout };
}

test("the trust dialog is answered with logged keys in order and Enter only after the trusted option is selected", async () => {
  const h = harness();
  try {
    const worker = await blockedWorker(h);
    const log: string[] = [];
    const original = h.fake.run;
    const sent: string[] = [];
    const traced = async (args: readonly string[]): Promise<HerdrResult> => {
      if (args[0] === "pane" && args[1] === "send-keys")
        sent.push(`sent:${args[3]}`);
      return original(args);
    };
    const adapter = new HerdrAdapter({
      run: traced,
      tempRoot: h.fake.root,
      sleep: async () => {},
      now: () => 0,
    });
    const entry = await adapter.createWorktree({
      workspaceId: "w9",
      branch: "d1",
      label: "d1",
    });
    await adapter.startAgent({
      name: "dev-d",
      kind: "claude",
      paneId: entry.paneId,
      args: [],
      environment: CLEAN_ENV,
    });
    const pane = h.fake.panes.get(entry.paneId)!;
    pane.screen = dialogScreen(entry.path);
    pane.status = "blocked";
    h.fake.agentStates.set("dev-d", {
      paneId: entry.paneId,
      statuses: ["blocked"],
    });
    h.fake.onKey = dialogKeys(pane, entry.path);
    const outcome = await adapter.answerTrustDialog({
      paneId: entry.paneId,
      log: (entry_: KeyLogEntry) => {
        log.push(`log:${entry_.key}`);
        sent.push(`before:${entry_.key}`);
      },
    });
    assert.deepEqual(outcome, { handled: true, keys: ["down", "enter"] });
    assert.deepEqual(log, ["log:down", "log:enter"]);
    assert.deepEqual(sent, [
      "before:down",
      "sent:down",
      "before:enter",
      "sent:enter",
    ]);
    void worker;
  } finally {
    h.adapter.close();
    h.fake.cleanup();
  }
});

test("an already selected trusted option needs only Enter", async () => {
  const h = harness();
  try {
    const worker = await blockedWorker(h, "yes");
    const keys: string[] = [];
    const outcome = await h.adapter.answerTrustDialog({
      paneId: worker.paneId,
      log: (entry) => {
        keys.push(entry.key);
      },
    });
    assert.deepEqual(outcome, { handled: true, keys: ["enter"] });
    assert.deepEqual(keys, ["enter"]);
  } finally {
    h.adapter.close();
    h.fake.cleanup();
  }
});

test("a dialog for a different path, unknown option text, a wrapped path or a dialog that is not last is left alone", async () => {
  const cases: ReadonlyArray<
    [string, (text: string, checkout: string) => string, string]
  > = [
    [
      "different path",
      (text, checkout) => text.replace(checkout, `${checkout}-other`),
      "path_mismatch",
    ],
    [
      "unknown option text",
      (text) => text.replace(TRUST_YES, "Yes, always trust everything"),
      "unknown_options",
    ],
    [
      "renamed default option",
      (text) => text.replace(TRUST_NO, "No, quit"),
      "unknown_options",
    ],
    [
      "third option",
      (text) => text.replace(`  ${TRUST_YES}`, `  ${TRUST_YES}\n  Maybe later`),
      "unknown_options",
    ],
    [
      "wrapped path",
      (text, checkout) => text.replace(checkout, `${checkout}\n   -more`),
      "wrapped_path",
    ],
    [
      "text after the dialog",
      (text) => `${text}\nsomething an agent printed\n`,
      "dialog_not_last",
    ],
    ["no dialog", () => idleScreen(), "no_dialog"],
    [
      "both marked",
      (text) => text.replace(`  ${TRUST_YES}`, `❯ ${TRUST_YES}`),
      "unknown_options",
    ],
  ];
  for (const [label, edit, reason] of cases) {
    const h = harness();
    try {
      const worker = await blockedWorker(h);
      worker.pane.screen = edit(worker.pane.screen, worker.checkout);
      const keys: string[] = [];
      const outcome = await h.adapter.answerTrustDialog({
        paneId: worker.paneId,
        log: (entry) => {
          keys.push(entry.key);
        },
      });
      assert.deepEqual(outcome, { handled: false, reason }, label);
      assert.deepEqual(keys, [], `${label}: no keypress is logged`);
      assert.equal(h.fake.events.length, 0, `${label}: no keypress is sent`);
    } finally {
      h.adapter.close();
      h.fake.cleanup();
    }
  }
});

test("a symlinked or trailing-slash path to the same directory is accepted and a path that does not resolve is not", async () => {
  const h = harness();
  try {
    const worker = await blockedWorker(h, "yes");
    worker.pane.screen = worker.pane.screen.replace(
      worker.checkout,
      `${worker.checkout}/`,
    );
    assert.equal(
      (
        await h.adapter.answerTrustDialog({
          paneId: worker.paneId,
          log: () => {},
        })
      ).handled,
      true,
    );
    const again = harness();
    try {
      const second = await blockedWorker(again, "yes");
      second.pane.screen = second.pane.screen.replace(
        second.checkout,
        "/nonexistent/path/nowhere",
      );
      assert.deepEqual(
        await again.adapter.answerTrustDialog({
          paneId: second.paneId,
          log: () => {},
        }),
        { handled: false, reason: "path_mismatch" },
      );
    } finally {
      again.adapter.close();
      again.fake.cleanup();
    }
  } finally {
    h.adapter.close();
    h.fake.cleanup();
  }
});

test("the dialog handler needs a blocked agent at its own pane, a worktree pane and a worker", async () => {
  const h = harness();
  try {
    const worker = await blockedWorker(h);
    h.fake.agentStates.set("dev", {
      paneId: worker.paneId,
      statuses: ["idle"],
    });
    await assert.rejects(
      h.adapter.answerTrustDialog({ paneId: worker.paneId, log: () => {} }),
      NotBlocked,
    );
    h.fake.agentStates.set("dev", { paneId: "w77:p1", statuses: ["blocked"] });
    await assert.rejects(
      h.adapter.answerTrustDialog({ paneId: worker.paneId, log: () => {} }),
      NotBlocked,
    );
    assert.equal(h.fake.events.length, 0);

    const workspace = await h.adapter.createWorkspace({
      cwd: h.fake.root,
      label: "plain",
      role: "worker",
    });
    await h.adapter.startAgent({
      name: "plain",
      kind: "claude",
      paneId: workspace.paneId,
      args: [],
      environment: CLEAN_ENV,
    });
    h.fake.agentStates.set("plain", {
      paneId: workspace.paneId,
      statuses: ["blocked"],
    });
    await assert.rejects(
      h.adapter.answerTrustDialog({ paneId: workspace.paneId, log: () => {} }),
      PhaseError,
    );
  } finally {
    h.adapter.close();
    h.fake.cleanup();
  }
});

test("when the selection cannot be reached no Enter is sent, and a dialog that stays open is reported", async () => {
  const h = harness();
  try {
    const worker = await blockedWorker(h);
    h.fake.onKey = () => {};
    const keys: string[] = [];
    const outcome = await h.adapter.answerTrustDialog({
      paneId: worker.paneId,
      log: (entry) => {
        keys.push(entry.key);
      },
    });
    assert.deepEqual(outcome, {
      handled: false,
      reason: "selection_not_reached",
    });
    assert.deepEqual(keys, ["down"]);
    assert.ok(!h.fake.events.includes("key:enter"));

    h.fake.onKey = (pane, key) => {
      if (key === "down") pane.screen = dialogScreen(worker.checkout, "yes");
    };
    await assert.rejects(
      h.adapter.answerTrustDialog({
        paneId: worker.paneId,
        log: () => {},
        timeoutMs: 1000,
      }),
      DialogStillOpen,
    );

    const throwing = harness();
    try {
      const other = await blockedWorker(throwing);
      await assert.rejects(
        throwing.adapter.answerTrustDialog({
          paneId: other.paneId,
          log: () => {
            throw new Error("log down");
          },
        }),
        /log down/,
      );
      assert.equal(throwing.fake.events.length, 0);
    } finally {
      throwing.adapter.close();
      throwing.fake.cleanup();
    }
  } finally {
    h.adapter.close();
    h.fake.cleanup();
  }
});

function shellSwitch(h: Harness, options: { runs?: boolean } = {}): void {
  h.fake.onRun = (pane, command) => {
    const rc = /--rcfile '([^']+)'/.exec(command);
    if (options.runs !== false && rc) {
      const directory = path.dirname(rc[1]!);
      rmSync(directory, { recursive: true, force: true });
      pane.screen = "❯ ";
    }
  };
}

test("the shell is replaced by a clean bash through a self-deleting rc file and the token never appears on the command line", async () => {
  const h = harness();
  try {
    const { paneId } = await h.adapter.createWorktree({
      workspaceId: "w9",
      branch: "s1",
      label: "s1",
    });
    let envContent = "";
    let rcContent = "";
    let mode = 0;
    h.fake.onRun = (pane, command) => {
      const rc = /--rcfile '([^']+)'/.exec(command)!;
      const directory = path.dirname(rc[1]!);
      envContent = readFileSync(path.join(directory, "env"), "utf8");
      rcContent = readFileSync(rc[1]!, "utf8");
      mode = statSync(directory).mode & 0o777;
      assert.equal(statSync(path.join(directory, "env")).mode & 0o777, 0o600);
      assert.equal(statSync(rc[1]!).mode & 0o777, 0o600);
      rmSync(directory, { recursive: true, force: true });
      pane.screen = "❯ ";
    };
    const environment = buildAgentEnvironment(
      {
        PATH: "/usr/bin:/bin",
        HOME: "/home/x",
        TERM: "xterm-256color",
        SECRET_FROM_OPERATOR: "no",
        CAPSTAN_TOKEN: "stale",
        LANG: "C.UTF-8",
      },
      { CAPSTAN_TOKEN: TOKEN, CAPSTAN_SOCKET: "/tmp/state/control.sock" },
    );
    await h.adapter.prepareShell({ paneId, environment });
    const command = h.fake.callsTo("pane", "run")[0]![3]!;
    assert.ok(!command.includes(TOKEN), "the token is not typed");
    assert.match(
      command,
      /^exec env -i HOME='\/home\/x' PATH='\/usr\/bin:\/bin' TERM='xterm-256color' bash --noprofile --rcfile '[^']+' -i$/,
    );
    assert.ok(envContent.includes(`export CAPSTAN_TOKEN='${TOKEN}'`));
    assert.ok(
      !envContent.includes("SECRET_FROM_OPERATOR") &&
        !envContent.includes("stale"),
    );
    assert.match(rcContent, /^\. '.*\/env'\nPS1='❯ '\nrm -rf '.*'\n$/);
    assert.equal(mode, 0o700);
    assert.equal(h.adapter.paneEntry(paneId)!.phase, "prepared");
    assert.equal(h.adapter.paneEntry(paneId)!.kind, "shell");
    await assert.rejects(
      h.adapter.prepareShell({ paneId, environment }),
      PhaseError,
    );
    assert.deepEqual(readdirCount(h.fake.root, "capstan-shell-"), 0);
  } finally {
    h.adapter.close();
    h.fake.cleanup();
  }
});

function readdirCount(directory: string, prefix: string): number {
  return readdirSync(directory).filter((name) => name.startsWith(prefix))
    .length;
}

test("the shell preparation refuses a pane that is not at a bare prompt and types nothing", async () => {
  for (const screen of [
    "❯ half typed",
    "user@host:~$ ",
    "output\nrunning",
    "",
  ]) {
    const h = harness();
    try {
      const { paneId } = await h.adapter.createWorktree({
        workspaceId: "w9",
        branch: "s2",
        label: "s2",
      });
      h.fake.panes.get(paneId)!.screen = screen;
      await assert.rejects(
        h.adapter.prepareShell({
          paneId,
          environment: { HOME: "/h", PATH: "/p", TERM: "t" },
        }),
        PromptUnrecognized,
        JSON.stringify(screen),
      );
      assert.equal(h.fake.callsTo("pane", "run").length, 0);
      assert.equal(h.adapter.paneEntry(paneId)!.phase, "fresh");
    } finally {
      h.adapter.close();
      h.fake.cleanup();
    }
  }
});

test("bad environment values are refused before anything is typed", async () => {
  const h = harness();
  try {
    const { paneId } = await h.adapter.createWorktree({
      workspaceId: "w9",
      branch: "s3",
      label: "s3",
    });
    const base = { HOME: "/h", PATH: "/p", TERM: "t" };
    for (const environment of [
      { ...base, HOME: "/h'; rm -rf ~; '" },
      { ...base, PATH: "/a b" },
      { ...base, TERM: "x\ny" },
      { ...base, lower: "x" },
      { ...base, GOOD: "line\nbreak" },
      { PATH: "/p", TERM: "t" } as Record<string, string>,
    ])
      await assert.rejects(
        h.adapter.prepareShell({ paneId, environment }),
        InvalidArgumentError,
      );
    assert.equal(h.fake.callsTo("pane", "run").length, 0);
    assert.throws(
      () => buildAgentEnvironment({}, { "bad-name": "x" }),
      InvalidArgumentError,
    );
    assert.throws(
      () => buildAgentEnvironment({}, { GOOD: "a\u0007b" }),
      InvalidArgumentError,
    );
    assert.deepEqual(
      buildAgentEnvironment(
        { PATH: "/p", HOME: "/h", EDITOR: "vim", CAPSTAN_SOCKET: "/leak" },
        {},
      ),
      { PATH: "/p", HOME: "/h" },
    );
  } finally {
    h.adapter.close();
    h.fake.cleanup();
  }
});

test("a shell that never shows its prompt taints the pane, the temporary files are removed and the pane is unusable", async () => {
  const h = harness();
  try {
    const { paneId } = await h.adapter.createWorktree({
      workspaceId: "w9",
      branch: "s4",
      label: "s4",
    });
    shellSwitch(h, { runs: false });
    await assert.rejects(
      h.adapter.prepareShell({
        paneId,
        environment: { HOME: "/h", PATH: "/p", TERM: "t" },
        timeoutMs: 500,
      }),
      ShellNotReady,
    );
    assert.equal(h.adapter.paneEntry(paneId)!.phase, "tainted");
    assert.equal(readdirCount(h.fake.root, "capstan-shell-"), 0);
    await assert.rejects(
      h.adapter.startAgent({
        name: "a",
        kind: "claude",
        paneId,
        args: [],
        environment: CLEAN_ENV,
      }),
      PhaseError,
    );
    await assert.rejects(
      h.adapter.prepareShell({
        paneId,
        environment: { HOME: "/h", PATH: "/p", TERM: "t" },
      }),
      PhaseError,
    );
  } finally {
    h.adapter.close();
    h.fake.cleanup();
  }
});

test("a pane run that fails taints the pane and the failure is raised", async () => {
  const h = harness();
  try {
    const { paneId } = await h.adapter.createWorktree({
      workspaceId: "w9",
      branch: "s5",
      label: "s5",
    });
    const original = h.fake.run;
    const failing = async (args: readonly string[]): Promise<HerdrResult> =>
      args[0] === "pane" && args[1] === "run"
        ? {
            code: 1,
            stdout: JSON.stringify({
              error: { code: "pane_gone", message: "gone" },
            }),
            stderr: "",
          }
        : original(args);
    const adapter = new HerdrAdapter({
      run: failing,
      tempRoot: h.fake.root,
      sleep: async () => {},
      now: () => 0,
    });
    const entry = await adapter.createWorktree({
      workspaceId: "w9",
      branch: "s6",
      label: "s6",
    });
    await assert.rejects(
      adapter.prepareShell({
        paneId: entry.paneId,
        environment: { HOME: "/h", PATH: "/p", TERM: "t" },
      }),
      (error: unknown) =>
        error instanceof HerdrError && error.code === "pane_gone",
    );
    assert.equal(adapter.paneEntry(entry.paneId)!.phase, "tainted");
    void paneId;
  } finally {
    h.adapter.close();
    h.fake.cleanup();
  }
});

test("starting an agent registers it, reports a startup dialog, and taints the pane on any other failure", async () => {
  const h = harness();
  try {
    const { paneId } = await h.adapter.createWorktree({
      workspaceId: "w9",
      branch: "a1",
      label: "a1",
    });
    assert.deepEqual(
      await h.adapter.startAgent({
        name: "one",
        kind: "claude",
        paneId,
        args: ["--model", "opus"],
        environment: CLEAN_ENV,
        timeoutMs: 1000,
      }),
      { status: "started" },
    );
    assert.deepEqual(h.fake.callsTo("agent", "start")[0], [
      "agent",
      "start",
      "one",
      "--kind",
      "claude",
      "--pane",
      paneId,
      "--timeout",
      "5000",
      "--",
      "--model",
      "opus",
    ]);
    assert.deepEqual(h.adapter.paneEntry(paneId), {
      role: "worker",
      phase: "started",
      kind: "claude",
      agent: "one",
      worktreePath: h.fake.panes.get(paneId)!.checkout,
      workspaceId: h.fake.panes.get(paneId)!.workspaceId,
    });
    await assert.rejects(
      h.adapter.startAgent({
        name: "two",
        kind: "claude",
        paneId,
        args: [],
        environment: CLEAN_ENV,
      }),
      PhaseError,
    );

    const second = await h.adapter.createWorktree({
      workspaceId: "w9",
      branch: "a2",
      label: "a2",
    });
    await assert.rejects(
      h.adapter.startAgent({
        name: "one",
        kind: "claude",
        paneId: second.paneId,
        args: [],
        environment: CLEAN_ENV,
      }),
      InvalidArgumentError,
    );
    await assert.rejects(
      h.adapter.startAgent({
        name: "x",
        kind: "codex",
        paneId: second.paneId,
        args: [],
        environment: CLEAN_ENV,
      }),
      UnsupportedHostError,
    );
    for (const args of [[""], ["a\nb"], ["a\u0000b"]])
      await assert.rejects(
        h.adapter.startAgent({
          name: "x",
          kind: "claude",
          paneId: second.paneId,
          args,
          environment: CLEAN_ENV,
        }),
        InvalidArgumentError,
      );
    h.fake.startError = {
      code: "agent_not_ready",
      message: "blocked during startup",
    };
    assert.deepEqual(
      await h.adapter.startAgent({
        name: "blocked",
        kind: "claude",
        paneId: second.paneId,
        args: [],
        environment: CLEAN_ENV,
      }),
      { status: "blocked_at_startup" },
    );
    assert.equal(h.adapter.paneEntry(second.paneId)!.phase, "started");
    assert.equal(h.adapter.paneEntry(second.paneId)!.agent, "blocked");

    const third = await h.adapter.createWorktree({
      workspaceId: "w9",
      branch: "a3",
      label: "a3",
    });
    h.fake.startError = {
      code: "agent_start_failed",
      message: "no such binary",
    };
    await assert.rejects(
      h.adapter.startAgent({
        name: "broken",
        kind: "claude",
        paneId: third.paneId,
        args: [],
        environment: CLEAN_ENV,
      }),
      (error: unknown) =>
        error instanceof HerdrError && error.code === "agent_start_failed",
    );
    assert.equal(h.adapter.paneEntry(third.paneId)!.phase, "tainted");
  } finally {
    h.adapter.close();
    h.fake.cleanup();
  }
});

test("starting in a fresh pane needs a bare prompt and a prepared pane needs an empty one", async () => {
  const h = harness();
  try {
    const { paneId } = await h.adapter.createWorktree({
      workspaceId: "w9",
      branch: "b1",
      label: "b1",
    });
    h.fake.panes.get(paneId)!.screen = "❯ half typed";
    await assert.rejects(
      h.adapter.startAgent({
        name: "a",
        kind: "claude",
        paneId,
        args: [],
        environment: CLEAN_ENV,
      }),
      PromptUnrecognized,
    );
    assert.equal(h.fake.callsTo("agent", "start").length, 0);
    assert.equal(h.adapter.paneEntry(paneId)!.phase, "fresh");

    const prepared = await h.adapter.createWorktree({
      workspaceId: "w9",
      branch: "b2",
      label: "b2",
    });
    shellSwitch(h);
    await h.adapter.prepareShell({
      paneId: prepared.paneId,
      environment: { HOME: "/h", PATH: "/p", TERM: "t" },
    });
    h.fake.panes.get(prepared.paneId)!.screen = "❯ typed after";
    await assert.rejects(
      h.adapter.startAgent({
        name: "b",
        kind: "claude",
        paneId: prepared.paneId,
        args: [],
        environment: CLEAN_ENV,
      }),
      PromptUnrecognized,
    );
    assert.equal(h.adapter.paneEntry(prepared.paneId)!.phase, "tainted");

    const withEnvironment = await h.adapter.createWorktree({
      workspaceId: "w9",
      branch: "b3",
      label: "b3",
    });
    await h.adapter.startAgent({
      name: "c",
      kind: "claude",
      paneId: withEnvironment.paneId,
      args: [],
      environment: { HOME: "/h", PATH: "/p", TERM: "t" },
    });
    assert.equal(h.adapter.paneEntry(withEnvironment.paneId)!.phase, "started");
    assert.equal(h.fake.callsTo("pane", "run").length, 2);
  } finally {
    h.adapter.close();
    h.fake.cleanup();
  }
});

test("claudeArguments builds the per-role list and refuses control characters", () => {
  const base = {
    model: null,
    permissionMode: "default",
    allow: [],
    deny: [],
    hooks: "inherit",
  } as const;
  assert.deepEqual(claudeArguments(base), ["--permission-mode", "default"]);
  assert.deepEqual(
    claudeArguments(
      {
        model: "opus",
        permissionMode: "acceptEdits",
        allow: ["Bash(cstan *)", "Read"],
        deny: ["Bash(rm *)"],
        hooks: "off",
      },
      "/tmp/p.md",
    ),
    [
      "--model",
      "opus",
      "--permission-mode",
      "acceptEdits",
      "--allowedTools",
      "Bash(cstan *)",
      "Read",
      "--disallowedTools",
      "Bash(rm *)",
      "--settings",
      '{"disableAllHooks":true}',
      "--append-system-prompt-file",
      "/tmp/p.md",
    ],
  );
  assert.throws(
    () => claudeArguments({ ...base, allow: ["a\nb"] }),
    InvalidArgumentError,
  );
  assert.throws(
    () => claudeArguments({ ...base, model: "x\u0000" }),
    InvalidArgumentError,
  );
});

test("prompt files are private and removed on close", () => {
  const h = harness();
  try {
    const file = h.adapter.writePromptFile("Line one\nLine two\n");
    assert.equal(readFileSync(file, "utf8"), "Line one\nLine two\n");
    assert.equal(statSync(file).mode & 0o777, 0o600);
    assert.equal(statSync(path.dirname(file)).mode & 0o777, 0o700);
    assert.throws(() => h.adapter.writePromptFile(""), InvalidArgumentError);
    assert.throws(
      () => h.adapter.writePromptFile("a\u0000b"),
      InvalidArgumentError,
    );
    h.adapter.close();
    assert.equal(existsSync(file), false);
  } finally {
    h.adapter.close();
    h.fake.cleanup();
  }
});

test("a fresh pane cannot start an agent without a clean environment and no call is made", async () => {
  const h = harness();
  try {
    const { paneId } = await h.adapter.createWorktree({
      workspaceId: "w9",
      branch: "e1",
      label: "e1",
    });
    const before = h.fake.calls.length;
    await assert.rejects(
      h.adapter.startAgent({ name: "x", kind: "claude", paneId, args: [] }),
      InvalidArgumentError,
    );
    assert.equal(h.fake.calls.length, before);
    assert.equal(h.adapter.paneEntry(paneId)!.phase, "fresh");
  } finally {
    h.adapter.close();
    h.fake.cleanup();
  }
});

test("the agent start call gets a runner limit longer than the start timeout", async () => {
  const h = harness();
  try {
    const seen: Array<number | undefined> = [];
    const original = h.fake.run;
    const adapter = new HerdrAdapter({
      run: async (args, options) => {
        if (args[0] === "agent" && args[1] === "start")
          seen.push(options?.timeoutMs);
        return original(args);
      },
      tempRoot: h.fake.root,
      sleep: async () => {},
      now: () => 0,
    });
    const { paneId } = await adapter.createWorktree({
      workspaceId: "w9",
      branch: "t1",
      label: "t1",
    });
    await adapter.startAgent({
      name: "t1",
      kind: "claude",
      paneId,
      args: [],
      environment: CLEAN_ENV,
      timeoutMs: 40_000,
    });
    assert.deepEqual(seen, [50_000]);
  } finally {
    h.adapter.close();
    h.fake.cleanup();
  }
});

test("the clear stops when the agent stops being idle or the line becomes unreadable, and sends no further keys", async () => {
  const h = harness();
  try {
    const worker = await startedWorker(h, idleScreen("a", "b", "c"));
    const keys: string[] = [];
    h.fake.agentStates.set("dev", {
      paneId: worker.paneId,
      statuses: ["idle", "idle", "blocked"],
    });
    const lines = { value: ["a", "b", "c"] };
    h.fake.onKey = ctrlUClearsOneLine(lines, worker.pane);
    await assert.rejects(
      h.adapter.clearAfterDeferral({
        paneId: worker.paneId,
        deferredForMs: 1,
        maxDeferralMs: 1,
        discard: () => {},
        log: (entry) => {
          keys.push(entry.key);
        },
      }),
      NotIdle,
    );
    assert.equal(keys.length, 2);

    worker.pane.screen = idleScreen("x");
    h.fake.agentStates.set("dev", {
      paneId: worker.paneId,
      statuses: ["idle"],
    });
    h.fake.onKey = (pane) => {
      pane.screen = "the input box is gone";
    };
    keys.length = 0;
    await assert.rejects(
      h.adapter.clearAfterDeferral({
        paneId: worker.paneId,
        deferredForMs: 1,
        maxDeferralMs: 1,
        discard: () => {},
        log: (entry) => {
          keys.push(entry.key);
        },
      }),
      InputUnreadable,
    );
    assert.equal(keys.length, 1);
  } finally {
    h.adapter.close();
    h.fake.cleanup();
  }
});

test("a prompt Herdr refuses after the record is reported as its own error with the cause", async () => {
  const h = harness();
  try {
    const worker = await startedWorker(h);
    const original = h.fake.run;
    const failing = new HerdrAdapter({
      run: async (args) =>
        args[0] === "agent" && args[1] === "prompt"
          ? {
              code: 1,
              stdout: "",
              stderr: '{"error":{"code":"agent_gone","message":"gone"}}',
            }
          : original(args),
      tempRoot: h.fake.root,
      sleep: async () => {},
      now: () => 0,
    });
    const entry = await failing.createWorktree({
      workspaceId: "w9",
      branch: "f1",
      label: "f1",
    });
    await failing.startAgent({
      name: "f1",
      kind: "claude",
      paneId: entry.paneId,
      args: [],
      environment: CLEAN_ENV,
    });
    h.fake.panes.get(entry.paneId)!.screen = idleScreen();
    h.fake.agentStates.set("f1", { paneId: entry.paneId, statuses: ["idle"] });
    let recorded = 0;
    await assert.rejects(
      failing.guardedSend({
        paneId: entry.paneId,
        text: "x",
        beforeSend: () => {
          recorded += 1;
        },
      }),
      (error: unknown) =>
        error instanceof SendAfterRecordError &&
        error.cause instanceof HerdrError &&
        error.cause.code === "agent_gone",
    );
    assert.equal(recorded, 1);
    void worker;
  } finally {
    h.adapter.close();
    h.fake.cleanup();
  }
});

test("a failing pane command reports Herdr's stderr error code and no control characters", async () => {
  const h = harness();
  try {
    const { paneId } = await h.adapter.createWorktree({
      workspaceId: "w9",
      branch: "r1",
      label: "r1",
    });
    const original = h.fake.run;
    const adapter = new HerdrAdapter({
      run: async (args) =>
        args[0] === "pane" && args[1] === "read"
          ? { code: 1, stdout: "", stderr: "boom\u001b[31m red\u0007" }
          : original(args),
      tempRoot: h.fake.root,
    });
    await assert.rejects(adapter.readScreen(paneId), (error: unknown) => {
      assert.ok(error instanceof HerdrError);
      assert.ok(!/\p{Cc}/u.test(error.message));
      assert.match(error.message, /boom/);
      return true;
    });
    void paneId;
  } finally {
    h.adapter.close();
    h.fake.cleanup();
  }
});

test("message text must be well formed, printable and not blank, and a leading dash is ordinary text", async () => {
  const h = harness();
  try {
    const worker = await startedWorker(h);
    for (const text of [
      "   ",
      "\n\t",
      "a\u001bb",
      "a\rb",
      "a\u001b[201~b",
      "a\u007fb",
      "a\u202eb",
      "lone \ud800 surrogate",
    ])
      await assert.rejects(
        h.adapter.guardedSend({
          paneId: worker.paneId,
          text,
          beforeSend: () => {},
        }),
        InvalidArgumentError,
        JSON.stringify(text),
      );
    assert.equal(h.fake.calls.length, 0);
    for (const text of ["--help", "line one\n\tline two", "ünïcode ✓"])
      assert.deepEqual(
        await h.adapter.guardedSend({
          paneId: worker.paneId,
          text,
          beforeSend: () => {},
        }),
        { sent: true },
      );
    assert.throws(
      () => h.adapter.writePromptFile("a\u001bb"),
      InvalidArgumentError,
    );
    assert.throws(
      () => h.adapter.writePromptFile("  \n"),
      InvalidArgumentError,
    );
    assert.throws(
      () => h.adapter.writePromptFile("lone \ud800"),
      InvalidArgumentError,
    );
  } finally {
    h.adapter.close();
    h.fake.cleanup();
  }
});

test("a role value that starts with a dash is refused and an empty HOME, PATH or TERM is refused", async () => {
  const base = {
    model: null,
    permissionMode: "default",
    allow: [],
    deny: [],
    hooks: "inherit",
  } as const;
  assert.throws(
    () => claudeArguments({ ...base, model: "--settings" }),
    InvalidArgumentError,
  );
  assert.throws(
    () =>
      claudeArguments({
        ...base,
        allow: ["Read", "--dangerously-skip-permissions"],
      }),
    InvalidArgumentError,
  );
  assert.throws(
    () => claudeArguments({ ...base, deny: ["-x"] }),
    InvalidArgumentError,
  );
  const h = harness();
  try {
    const { paneId } = await h.adapter.createWorktree({
      workspaceId: "w9",
      branch: "v1",
      label: "v1",
    });
    for (const name of ["HOME", "PATH", "TERM"])
      await assert.rejects(
        h.adapter.prepareShell({
          paneId,
          environment: { ...CLEAN_ENV, [name]: "" },
        }),
        InvalidArgumentError,
        name,
      );
    assert.equal(h.fake.callsTo("pane", "run").length, 0);
  } finally {
    h.adapter.close();
    h.fake.cleanup();
  }
});

test("more invisible text characters, lone surrogates in arguments and environment, and a NaN start timeout are refused", async () => {
  const h = harness();
  try {
    const worker = await startedWorker(h);
    for (const text of [
      "a\u200eb",
      "a\u200fb",
      "a\u061cb",
      "a\u2028b",
      "a\u2029b",
      "a\ufeffb",
    ])
      await assert.rejects(
        h.adapter.guardedSend({
          paneId: worker.paneId,
          text,
          beforeSend: () => {},
        }),
        InvalidArgumentError,
        JSON.stringify(text),
      );
    assert.equal(h.fake.calls.length, 0);
    const base = {
      model: null,
      permissionMode: "default",
      allow: [],
      deny: [],
      hooks: "inherit",
    } as const;
    assert.throws(
      () => claudeArguments({ ...base, allow: ["a\ud800"] }),
      InvalidArgumentError,
    );
    assert.throws(
      () => buildAgentEnvironment({}, { GOOD: "a\ud800" }),
      InvalidArgumentError,
    );
    const { paneId } = await h.adapter.createWorktree({
      workspaceId: "w9",
      branch: "n1",
      label: "n1",
    });
    await assert.rejects(
      h.adapter.prepareShell({
        paneId,
        environment: { ...CLEAN_ENV, GOOD: "a\ud800" },
      }),
      InvalidArgumentError,
    );
    for (const timeoutMs of [Number.NaN, Number.POSITIVE_INFINITY])
      await assert.rejects(
        h.adapter.startAgent({
          name: "n1",
          kind: "claude",
          paneId,
          args: [],
          environment: CLEAN_ENV,
          timeoutMs,
        }),
        InvalidArgumentError,
      );
    await assert.rejects(
      h.adapter.startAgent({
        name: "n1",
        kind: "claude",
        paneId,
        args: ["a\ud800"],
        environment: CLEAN_ENV,
      }),
      InvalidArgumentError,
    );
    assert.equal(h.fake.callsTo("pane", "run").length, 0);
  } finally {
    h.adapter.close();
    h.fake.cleanup();
  }
});

test("text that would run a Claude Code command, or hides format characters, is refused and joiners are kept", async () => {
  const h = harness();
  try {
    const worker = await startedWorker(h);
    for (const text of [
      "/clear",
      "  /exit now",
      "!rm -rf x",
      "# remember this",
      "a\u200bb",
      "a\u2060b",
      "a\u00adb",
      "a\u{e0041}b",
      "a\ufdd0b",
    ])
      await assert.rejects(
        h.adapter.guardedSend({
          paneId: worker.paneId,
          text,
          beforeSend: () => {},
        }),
        InvalidArgumentError,
        JSON.stringify(text),
      );
    assert.equal(h.fake.calls.length, 0);
    for (const text of [
      "see /clear later",
      "note: #1 and !x",
      "a\u200db\u200cc",
    ])
      assert.deepEqual(
        await h.adapter.guardedSend({
          paneId: worker.paneId,
          text,
          beforeSend: () => {},
        }),
        { sent: true },
      );
    assert.doesNotThrow(() =>
      h.adapter.writePromptFile("/system prompt may start with a slash"),
    );
  } finally {
    h.adapter.close();
    h.fake.cleanup();
  }
});

test("the clear refuses deferral times that are not finite, negative, or a zero maximum", async () => {
  const h = harness();
  try {
    const worker = await startedWorker(h, idleScreen("typed"));
    for (const [deferredForMs = 0, maxDeferralMs = 0] of [
      [0, 0],
      [5, -1],
      [-1, 5],
      [Number.NaN, 5],
      [5, Number.NaN],
      [Number.POSITIVE_INFINITY, Number.POSITIVE_INFINITY],
    ])
      await assert.rejects(
        h.adapter.clearAfterDeferral({
          paneId: worker.paneId,
          deferredForMs,
          maxDeferralMs,
          discard: () => {},
          log: () => {},
        }),
        InvalidArgumentError,
        `${deferredForMs}/${maxDeferralMs}`,
      );
    assert.equal(h.fake.events.length, 0);
  } finally {
    h.adapter.close();
    h.fake.cleanup();
  }
});

test("a dialog path that is not absolute, such as one starting with a tilde, is not answered", async () => {
  const h = harness();
  try {
    const worker = await blockedWorker(h, "yes");
    worker.pane.screen = worker.pane.screen.replace(
      worker.checkout,
      "~/some/worktree",
    );
    assert.deepEqual(
      await h.adapter.answerTrustDialog({
        paneId: worker.paneId,
        log: () => {},
      }),
      { handled: false, reason: "path_mismatch" },
    );
    assert.equal(h.fake.events.length, 0);
  } finally {
    h.adapter.close();
    h.fake.cleanup();
  }
});

test("a clear that meets text typed between rounds discards it before the next key", async () => {
  const h = harness();
  try {
    const worker = await startedWorker(h, idleScreen("first"));
    const discarded: string[] = [];
    const keys: string[] = [];
    let round = 0;
    h.fake.onKey = (pane) => {
      round += 1;
      pane.screen = round === 1 ? idleScreen("surprise") : idleScreen();
    };
    const cleared = await h.adapter.clearAfterDeferral({
      paneId: worker.paneId,
      deferredForMs: 1,
      maxDeferralMs: 1,
      discard: (text) => {
        discarded.push(text);
        assert.equal(keys.length, discarded.length - 1);
      },
      log: (entry) => {
        keys.push(entry.key);
      },
    });
    assert.deepEqual(cleared, { cleared: true, text: "first" });
    assert.deepEqual(discarded, ["first", "surprise"]);
    assert.equal(keys.length, 2);
  } finally {
    h.adapter.close();
    h.fake.cleanup();
  }
});

test("text that starts with a tab, question mark or at sign is refused", async () => {
  const h = harness();
  try {
    const worker = await startedWorker(h);
    for (const text of ["?help", "@file", "\tindented", "  ?x"])
      await assert.rejects(
        h.adapter.guardedSend({
          paneId: worker.paneId,
          text,
          beforeSend: () => {},
        }),
        InvalidArgumentError,
        JSON.stringify(text),
      );
    assert.deepEqual(
      await h.adapter.guardedSend({
        paneId: worker.paneId,
        text: "ok? see @file",
        beforeSend: () => {},
      }),
      { sent: true },
    );
  } finally {
    h.adapter.close();
    h.fake.cleanup();
  }
});

test("herdrStateOf maps known statuses and everything else to unknown, and isAgentName follows the Herdr name rule", () => {
  for (const status of ["idle", "working", "blocked", "done", "unknown"])
    assert.equal(herdrStateOf(status), status);
  for (const status of ["", "IDLE", "starting", "running", "\u0000"])
    assert.equal(herdrStateOf(status), "unknown");
  for (const good of ["dev", "developer-agent", "a.b_c-d", "A1"])
    assert.equal(isAgentName(good), true, good);
  for (const bad of [
    "",
    "-x",
    ".x",
    "a:b",
    "a b",
    "a/b",
    "x".repeat(65),
    5,
    undefined,
  ])
    assert.equal(isAgentName(bad), false, String(bad));
});

test("paneForAgent and agentObservation follow the registry and the pane check", async () => {
  const h = harness();
  try {
    assert.equal(h.adapter.paneForAgent("dev"), undefined);
    await assert.rejects(h.adapter.agentObservation("dev"), UnknownPaneError);
    const worker = await startedWorker(h);
    assert.equal(h.adapter.paneForAgent("dev"), worker.paneId);
    h.fake.agentStates.set("dev", {
      paneId: worker.paneId,
      statuses: ["working"],
    });
    assert.equal(await h.adapter.agentObservation("dev"), "working");
    h.fake.agentStates.set("dev", {
      paneId: worker.paneId,
      statuses: ["strange"],
    });
    assert.equal(await h.adapter.agentObservation("dev"), "unknown");
    h.fake.agentStates.set("dev", { paneId: "w99:p1", statuses: ["idle"] });
    await assert.rejects(h.adapter.agentObservation("dev"), AgentPaneMismatch);
    h.fake.agentStates.delete("dev");
    await assert.rejects(
      h.adapter.agentObservation("dev"),
      (error: unknown) => error instanceof HerdrError,
    );
  } finally {
    h.adapter.close();
    h.fake.cleanup();
  }
});

test("notify builds the exact Herdr command and refuses text that is blank, long, malformed or has control characters", async () => {
  const h = harness();
  try {
    await h.adapter.notify(
      "Capstan: PM message waiting",
      "Message m-1 is waiting",
    );
    assert.deepEqual(h.fake.callsTo("notification", "show")[0], [
      "notification",
      "show",
      "Capstan: PM message waiting",
      "--body",
      "Message m-1 is waiting",
      "--sound",
      "request",
    ]);
    const before = h.fake.calls.length;
    for (const [title, body] of [
      ["", "x"],
      ["t", "  "],
      ["t".repeat(101), "x"],
      ["t", "x".repeat(501)],
      ["t\u001b[31m", "x"],
      ["t", "line\nbreak"],
      ["t", "lone \ud800"],
      ["t", "bidi \u202e"],
    ] as const)
      await assert.rejects(
        h.adapter.notify(title, body),
        InvalidArgumentError,
        JSON.stringify([title, body]),
      );
    assert.equal(h.fake.calls.length, before, "nothing reaches Herdr");
  } finally {
    h.adapter.close();
    h.fake.cleanup();
  }
});

test("the unreadable-line detail is the exported constant", async () => {
  const h = harness();
  try {
    const worker = await startedWorker(h, "no input box here");
    const outcome = await h.adapter.guardedSend({
      paneId: worker.paneId,
      text: "hi",
      beforeSend: () => {},
    });
    assert.deepEqual(outcome, {
      sent: false,
      reason: "input_not_empty",
      detail: INPUT_UNREADABLE_DETAIL,
    });
  } finally {
    h.adapter.close();
    h.fake.cleanup();
  }
});

test("notify fails when Herdr accepts the command but shows nothing", async () => {
  const h = harness();
  try {
    h.fake.notification = { shown: false, reason: "disabled" };
    await assert.rejects(
      h.adapter.notify("Capstan: PM message waiting", "body"),
      (error: unknown) =>
        error instanceof HerdrError &&
        error.code === "notification_not_shown" &&
        /disabled/.test(error.message),
    );
    h.fake.notification = { shown: true };
    await h.adapter.notify("Capstan: PM message waiting", "body");
  } finally {
    h.adapter.close();
    h.fake.cleanup();
  }
});

function secondAdapter(h: Harness): HerdrAdapter {
  return new HerdrAdapter({
    run: h.fake.run,
    tempRoot: h.fake.root,
    sleep: async () => {},
    now: () => 0,
  });
}

test("a second adapter adopts a pane the first one started, after Herdr confirms it, and can then deliver to it", async () => {
  const h = harness();
  try {
    const worker = await startedWorker(h);
    const entry = h.adapter.paneEntry(worker.paneId)!;
    const next = secondAdapter(h);
    assert.equal(next.paneForAgent("dev"), undefined);
    await next.adoptPane({
      paneId: worker.paneId,
      role: "worker",
      agent: "dev",
      workspaceId: entry.workspaceId ?? null,
      worktreePath: entry.worktreePath ?? null,
    });
    assert.equal(next.paneForAgent("dev"), worker.paneId);
    assert.deepEqual(next.paneEntry(worker.paneId), {
      role: "worker",
      phase: "started",
      kind: "claude",
      agent: "dev",
      workspaceId: entry.workspaceId,
      worktreePath: entry.worktreePath,
    });
    assert.deepEqual(
      await next.guardedSend({
        paneId: worker.paneId,
        text: "after the restart",
        beforeSend: () => {},
      }),
      { sent: true },
    );
    await assert.rejects(
      next.adoptPane({
        paneId: worker.paneId,
        role: "worker",
        agent: "dev",
        workspaceId: null,
        worktreePath: null,
      }),
      PhaseError,
    );
  } finally {
    h.adapter.close();
    h.fake.cleanup();
  }
});

test("adoption registers nothing when the pane is gone, the name points elsewhere, or the name is unusable", async () => {
  const h = harness();
  try {
    const worker = await startedWorker(h);
    const next = secondAdapter(h);
    await assert.rejects(
      next.adoptPane({
        paneId: "w77:p1",
        role: "worker",
        agent: "dev",
        workspaceId: null,
        worktreePath: null,
      }),
      PaneGone,
    );
    h.fake.agentStates.set("dev", { paneId: "w78:p1", statuses: ["idle"] });
    await assert.rejects(
      next.adoptPane({
        paneId: worker.paneId,
        role: "worker",
        agent: "dev",
        workspaceId: null,
        worktreePath: null,
      }),
      AgentPaneMismatch,
    );
    await assert.rejects(
      next.adoptPane({
        paneId: worker.paneId,
        role: "worker",
        agent: "a:b",
        workspaceId: null,
        worktreePath: null,
      }),
      InvalidArgumentError,
    );
    await assert.rejects(
      next.adoptPane({
        paneId: "bad",
        role: "worker",
        agent: "dev",
        workspaceId: null,
        worktreePath: null,
      }),
      InvalidArgumentError,
    );
    assert.equal(next.paneEntry(worker.paneId), undefined);
    assert.equal(next.paneForAgent("dev"), undefined);
  } finally {
    h.adapter.close();
    h.fake.cleanup();
  }
});

test("a fallback shell pane is adopted for display only and accepts no agent input", async () => {
  const h = harness();
  try {
    const watch = h.fake.add("watch", SHELL_READY);
    const next = secondAdapter(h);
    await next.adoptShellPane(watch.paneId, watch.workspaceId);
    assert.equal(next.paneEntry(watch.paneId)!.kind, "shell");
    await assert.rejects(
      next.guardedSend({
        paneId: watch.paneId,
        text: "x",
        beforeSend: () => {},
      }),
      PhaseError,
    );
    await assert.rejects(next.adoptShellPane("w88:p1", null), PaneGone);
  } finally {
    h.adapter.close();
    h.fake.cleanup();
  }
});

test("runInPane runs one command in a fresh shell pane only, and then retires the pane from input", async () => {
  const h = harness();
  try {
    const watch = await h.adapter.createWorkspace({
      cwd: h.fake.root,
      label: "watch",
      role: "worker",
    });
    await h.adapter.runInPane(
      watch.paneId,
      "'/usr/bin/node' '/x/cli.js' status --watch",
    );
    assert.deepEqual(h.fake.callsTo("pane", "run").at(-1), [
      "pane",
      "run",
      watch.paneId,
      "'/usr/bin/node' '/x/cli.js' status --watch",
    ]);
    assert.equal(h.adapter.paneEntry(watch.paneId)!.phase, "started");
    await assert.rejects(
      h.adapter.runInPane(watch.paneId, "echo again"),
      PhaseError,
    );
    const pm = await h.adapter.createWorkspace({
      cwd: h.fake.root,
      label: "pm",
      role: "PM",
    });
    await assert.rejects(h.adapter.runInPane(pm.paneId, "echo pm"), PhaseError);
    const fresh = await h.adapter.createWorkspace({
      cwd: h.fake.root,
      label: "w2",
      role: "worker",
    });
    for (const command of ["", "   ", "a\nb", "x".repeat(2001), "lone \ud800"])
      await assert.rejects(
        h.adapter.runInPane(fresh.paneId, command),
        InvalidArgumentError,
        JSON.stringify(command),
      );
    await assert.rejects(
      h.adapter.runInPane("w99:p1", "echo x"),
      UnknownPaneError,
    );
  } finally {
    h.adapter.close();
    h.fake.cleanup();
  }
});
