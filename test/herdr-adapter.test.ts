import assert from "node:assert/strict";
import {
  existsSync,
  readdirSync,
  readFileSync,
  rmSync,
  statSync,
} from "node:fs";
import path from "node:path";
import { test } from "node:test";
import {
  AgentPaneMismatch,
  HerdrAdapter,
  InvalidArgumentError,
  PhaseError,
  PmPaneError,
  PromptUnrecognized,
  ShellNotReady,
  UnknownPaneError,
  UnsupportedHostError,
  herdrStateOf,
  isAgentName,
  buildAgentEnvironment,
  claudeArguments,
} from "../src/herdr/adapter.js";
import { HerdrError, type HerdrResult } from "../src/herdr/runner.js";
import {
  fixture,
  idleScreen,
  SHELL_READY,
  CLEAN_ENV,
  type Harness,
  harness,
  startedWorker,
} from "./herdr-adapter-harness.js";

const TOKEN = "tok_ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
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
      capture: () => h.adapter.capturePrompt(paneId),
      answer: () =>
        h.adapter.answerPrompt({
          paneId,
          promptSha: "a".repeat(64),
          answer: { kind: "esc" },
          beforeType: noop,
          log: noop,
        }),
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
    for (const name of [
      "send",
      "clear",
      "dialog",
      "capture",
      "answer",
    ] as const)
      await assert.rejects(attempts(worker)[name], PhaseError);
    assert.equal(before(), workerAt, "a fresh worker pane accepts no messages");

    const pm = await h.adapter.createWorkspace({
      cwd: h.fake.root,
      label: "pm",
      role: "PM",
    });
    const pmAt = before();
    for (const name of [
      "send",
      "clear",
      "dialog",
      "capture",
      "answer",
    ] as const)
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
    const passed = buildAgentEnvironment(
      { PATH: "/usr/bin", NEXORA_API_KEY: "k", OTHER: "o" },
      { CAPSTAN_TOKEN: "t" },
      ["NEXORA_API_KEY", "ABSENT"],
    );
    assert.equal(passed.NEXORA_API_KEY, "k");
    assert.equal(passed.OTHER, undefined);
    assert.equal(passed.ABSENT, undefined);
    for (const name of ["CAPSTAN_TOKEN", "bad-name", "lower", ""])
      assert.throws(
        () => buildAgentEnvironment({ [name]: "x" }, {}, [name]),
        InvalidArgumentError,
        name,
      );
    assert.throws(
      () =>
        buildAgentEnvironment({ SECRET_ONE: "a\u0007b" }, {}, ["SECRET_ONE"]),
      (error: Error) =>
        error instanceof InvalidArgumentError &&
        error.message.includes("SECRET_ONE") &&
        !error.message.includes("a\u0007b"),
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
test("a PATH, HOME or TERM with a tilde, a space or a quote is shell-quoted into the command, not refused", async () => {
  const h = harness();
  try {
    const { paneId } = await h.adapter.createWorktree({
      workspaceId: "w9",
      branch: "s4",
      label: "s4",
    });
    h.fake.onRun = (pane, command) => {
      const rc = /--rcfile '([^']+)'/.exec(command)!;
      rmSync(path.dirname(rc[1]!), { recursive: true, force: true });
      pane.screen = "❯ ";
    };
    await h.adapter.prepareShell({
      paneId,
      environment: {
        HOME: "/home/a b",
        PATH: "/w/git.sr.ht/~who/repo/.capstan/bin:/usr/bin",
        TERM: "it's",
      },
    });
    const command = h.fake.callsTo("pane", "run")[0]![3]!;
    assert.ok(command.includes("HOME='/home/a b'"));
    assert.ok(
      command.includes("PATH='/w/git.sr.ht/~who/repo/.capstan/bin:/usr/bin'"),
    );
    assert.ok(command.includes("TERM='it'\\''s'"));
  } finally {
    h.adapter.close();
    h.fake.cleanup();
  }
});
test("a HOME that tries to end its quote stays one inert word, and bidi or line separator characters are refused", async () => {
  const h = harness();
  try {
    const { paneId } = await h.adapter.createWorktree({
      workspaceId: "w9",
      branch: "s5",
      label: "s5",
    });
    h.fake.onRun = (pane, command) => {
      const rc = /--rcfile '([^']+)'/.exec(command)!;
      rmSync(path.dirname(rc[1]!), { recursive: true, force: true });
      pane.screen = "❯ ";
    };
    for (const environment of [
      { HOME: "/h‮", PATH: "/p", TERM: "t" },
      { HOME: "/h", PATH: "/p x", TERM: "t" },
    ])
      await assert.rejects(
        h.adapter.prepareShell({ paneId, environment }),
        InvalidArgumentError,
      );
    assert.equal(h.fake.callsTo("pane", "run").length, 0);
    await h.adapter.prepareShell({
      paneId,
      environment: { HOME: "/h'; rm -rf ~; '", PATH: "/p", TERM: "t" },
    });
    const command = h.fake.callsTo("pane", "run")[0]![3]!;
    assert.ok(command.includes(String.raw`HOME='/h'\''; rm -rf ~; '\'''`));
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
        kind: "gemini",
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
    h.fake.notification = {
      shown: false,
      reason: "no_foreground_client",
    };
    await assert.rejects(
      h.adapter.notify("Capstan: PM message waiting", "body"),
      (error: unknown) =>
        error instanceof HerdrError &&
        error.code === "notification_not_shown" &&
        /no_foreground_client/.test(error.message),
    );
    h.fake.notification = { shown: true };
    await h.adapter.notify("Capstan: PM message waiting", "body");
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
test("the input line of a Codex or OMP agent is read with that host's parser", async () => {
  for (const [kind, empty, typed] of [
    ["codex", "codex-idle-empty.ansi", "codex-idle-typed.ansi"],
    ["omp", "omp-idle-empty.ansi", "omp-idle-typed.ansi"],
  ] as const) {
    const h = harness();
    try {
      const worker = await startedWorker(h, fixture(empty), ["idle"], kind);
      assert.equal(h.adapter.paneEntry(worker.paneId)!.kind, kind);
      assert.equal(await h.adapter.readInput(worker.paneId), "", kind);
      worker.pane.screen = fixture(typed);
      assert.equal(
        await h.adapter.readInput(worker.paneId),
        "hello typed",
        kind,
      );
    } finally {
      h.adapter.close();
      h.fake.cleanup();
    }
  }
});
test("with a project slug Herdr sees <slug>-<id> at every agent call while callers keep the ledger id", async () => {
  const h = harness("acme");
  try {
    const { paneId } = await h.adapter.createWorktree({
      workspaceId: "w9",
      branch: "cap/task/dev-g1",
      label: "acme · dev-1",
    });
    await h.adapter.startAgent({
      name: "dev-1",
      kind: "claude",
      paneId,
      args: [],
      environment: CLEAN_ENV,
    });
    assert.deepEqual(h.fake.callsTo("agent", "start")[0]!.slice(0, 3), [
      "agent",
      "start",
      "acme-dev-1",
    ]);
    assert.ok(h.fake.agentStates.has("acme-dev-1"));
    h.fake.panes.get(paneId)!.screen = idleScreen();
    assert.equal(h.adapter.paneForAgent("dev-1"), paneId);
    assert.equal((await h.adapter.agentState("dev-1")).paneId, paneId);
    assert.deepEqual(h.fake.callsTo("agent", "get").at(-1), [
      "agent",
      "get",
      "acme-dev-1",
    ]);
    const sent = await h.adapter.guardedSend({
      paneId,
      text: "hello",
      beforeSend: () => {},
    });
    assert.equal(sent.sent, true);
    assert.deepEqual(h.fake.callsTo("agent", "prompt").at(-1), [
      "agent",
      "prompt",
      "acme-dev-1",
      "hello",
    ]);
  } finally {
    h.adapter.close();
    h.fake.cleanup();
  }
});
test("an agent id that does not fit Herdr's 32 characters behind the slug is refused at start, before Herdr is called", async () => {
  const h = harness("acme");
  try {
    const { paneId } = await h.adapter.createWorktree({
      workspaceId: "w9",
      branch: "cap/task/long-g1",
      label: "acme · long",
    });
    await assert.rejects(
      h.adapter.startAgent({
        name: "a-very-long-role-name-that-overflows-1",
        kind: "claude",
        paneId,
        args: [],
        environment: CLEAN_ENV,
      }),
      /does not fit Herdr's rule/,
    );
    assert.equal(h.fake.callsTo("agent", "start").length, 0);
  } finally {
    h.adapter.close();
    h.fake.cleanup();
  }
});
