import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
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
  NotIdle,
  PhaseError,
  SendAfterRecordError,
  UnknownPaneError,
  type KeyLogEntry,
} from "../src/herdr/adapter.js";
import { HerdrError, type HerdrResult } from "../src/herdr/runner.js";
import {
  TEXT_FIELD_WORDING,
  TRUST_NO,
  TRUST_YES,
  parseHostPrompt,
  stripAnsi,
} from "../src/herdr/screen.js";
import {
  promptHash,
  type CapturedPrompt,
  type PromptAnswer,
} from "../src/herdr/prompt-relay.js";
import {
  fixture,
  idleScreen,
  CLEAN_ENV,
  type FakePane,
  type Harness,
  harness,
  startedWorker,
  answer,
} from "./herdr-adapter-harness.js";

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
      blocker: "unknown",
    });
  } finally {
    h.adapter.close();
    h.fake.cleanup();
  }
});
test("a dialog that redraws a moment after the Down key is still answered, with Enter sent only after the selection shows", async () => {
  const h = harness();
  try {
    const worker = await blockedWorker(h);
    let readsAfterDown = -1;
    const original = h.fake.run;
    const slow = new HerdrAdapter({
      run: async (args) => {
        if (args[0] === "pane" && args[1] === "send-keys" && args[3] === "down")
          readsAfterDown = 0;
        if (args[0] === "pane" && args[1] === "read" && readsAfterDown >= 0) {
          readsAfterDown += 1;
          if (readsAfterDown === 4)
            worker.pane.screen = dialogScreen(worker.checkout, "yes");
        }
        return original(args);
      },
      tempRoot: h.fake.root,
      sleep: async () => {},
      now: (() => {
        let clock = 0;
        return () => (clock += 50);
      })(),
    });
    await slow.adoptPane({
      paneId: worker.paneId,
      role: "worker",
      agent: "dev",
      workspaceId: null,
      worktreePath: h.adapter.paneEntry(worker.paneId)!.worktreePath ?? null,
    });
    h.fake.onKey = (pane, key) => {
      if (key === "enter") {
        pane.status = "idle";
        pane.screen = idleScreen();
      }
    };
    const keys: string[] = [];
    const outcome = await slow.answerTrustDialog({
      paneId: worker.paneId,
      log: (entry) => {
        keys.push(entry.key);
      },
    });
    assert.deepEqual(outcome, { handled: true, keys: ["down", "enter"] });
    assert.deepEqual(keys, ["down", "enter"]);
    assert.ok(readsAfterDown >= 4, "the selection was polled until it showed");
  } finally {
    h.adapter.close();
    h.fake.cleanup();
  }
});
test("half-drawn reads during the redraw are waited out, but a different path stops the answer at once", async () => {
  const h = harness();
  try {
    const worker = await blockedWorker(h);
    let after = -1;
    const original = h.fake.run;
    const garbled = [
      () => "",
      () =>
        dialogScreen(worker.checkout, "no").replace(
          "Yes, I trust this folder",
          "Yes, I tr",
        ),
      () => `${dialogScreen(worker.checkout, "yes")}\nstray line`,
    ];
    const adapter = new HerdrAdapter({
      run: async (args) => {
        if (args[0] === "pane" && args[1] === "send-keys" && args[3] === "down")
          after = 0;
        if (args[0] === "pane" && args[1] === "read" && after >= 0) {
          const index = after;
          after += 1;
          if (index < garbled.length)
            return { code: 0, stdout: garbled[index]!(), stderr: "" };
          if (index === garbled.length)
            worker.pane.screen = dialogScreen(worker.checkout, "yes");
        }
        return original(args);
      },
      tempRoot: h.fake.root,
      sleep: async () => {},
      now: (() => {
        let clock = 0;
        return () => (clock += 20);
      })(),
    });
    await adapter.adoptPane({
      paneId: worker.paneId,
      role: "worker",
      agent: "dev",
      workspaceId: null,
      worktreePath: h.adapter.paneEntry(worker.paneId)!.worktreePath ?? null,
    });
    h.fake.onKey = (pane, key) => {
      if (key === "enter") {
        pane.status = "idle";
        pane.screen = idleScreen();
      }
    };
    const outcome = await adapter.answerTrustDialog({
      paneId: worker.paneId,
      log: () => {},
    });
    assert.deepEqual(outcome, { handled: true, keys: ["down", "enter"] });

    const g = harness();
    try {
      const other = await blockedWorker(g);
      const second = new HerdrAdapter({
        run: async (args) => {
          if (
            args[0] === "pane" &&
            args[1] === "read" &&
            args[2] === other.paneId &&
            g.fake.events.includes("key:down")
          )
            return {
              code: 0,
              stdout: dialogScreen(`${other.checkout}-elsewhere`, "yes"),
              stderr: "",
            };
          return g.fake.run(args);
        },
        tempRoot: g.fake.root,
        sleep: async () => {},
        now: () => 0,
      });
      await second.adoptPane({
        paneId: other.paneId,
        role: "worker",
        agent: "dev",
        workspaceId: null,
        worktreePath: g.adapter.paneEntry(other.paneId)!.worktreePath ?? null,
      });
      g.fake.onKey = () => {};
      const stopped = await second.answerTrustDialog({
        paneId: other.paneId,
        log: () => {},
      });
      assert.deepEqual(stopped, { handled: false, reason: "path_mismatch" });
      assert.ok(!g.fake.events.includes("key:enter"));
    } finally {
      g.adapter.close();
      g.fake.cleanup();
    }
  } finally {
    h.adapter.close();
    h.fake.cleanup();
  }
});
test("a Codex start that is blocked at startup keeps its kind and its trust dialog is answered with Enter only", async () => {
  const h = harness();
  try {
    const { paneId } = await h.adapter.createWorktree({
      workspaceId: "w9",
      branch: "cap/task/cx-g1",
      label: "cx",
    });
    h.fake.startError = {
      code: "agent_not_ready",
      message: "blocked during startup",
    };
    assert.deepEqual(
      await h.adapter.startAgent({
        name: "cx",
        kind: "codex",
        paneId,
        args: [],
        environment: CLEAN_ENV,
      }),
      { status: "blocked_at_startup" },
    );
    h.fake.startError = undefined;
    assert.equal(h.adapter.paneEntry(paneId)!.kind, "codex");
    const checkout = h.adapter.paneEntry(paneId)!.worktreePath!;
    const pane = h.fake.panes.get(paneId)!;
    pane.screen = stripAnsi(fixture("codex-trust-dialog.ansi")).replace(
      "/tmp/s7q",
      checkout,
    );
    pane.status = "blocked";
    h.fake.agentStates.set("cx", { paneId, statuses: ["blocked"] });
    h.fake.onKey = (target, key) => {
      if (target === pane && key === "enter") {
        pane.status = "idle";
        pane.screen = fixture("codex-idle-empty.ansi");
      }
    };
    const keys: string[] = [];
    const outcome = await h.adapter.answerTrustDialog({
      paneId,
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
test("an OMP agent has no trust dialog to answer, and a Codex dialog for another path is left alone", async () => {
  const h = harness();
  try {
    const worker = await startedWorker(
      h,
      fixture("omp-idle-empty.ansi"),
      ["idle"],
      "omp",
    );
    assert.deepEqual(
      await h.adapter.answerTrustDialog({
        paneId: worker.paneId,
        log: () => {},
      }),
      { handled: false, reason: "host_has_no_trust_dialog" },
    );
  } finally {
    h.adapter.close();
    h.fake.cleanup();
  }
  const g = harness();
  try {
    const worker = await startedWorker(
      g,
      fixture("codex-idle-empty.ansi"),
      ["idle"],
      "codex",
    );
    worker.pane.screen = stripAnsi(fixture("codex-trust-dialog.ansi"));
    worker.pane.status = "blocked";
    g.fake.agentStates.set("dev", {
      paneId: worker.paneId,
      statuses: ["blocked"],
    });
    assert.deepEqual(
      await g.adapter.answerTrustDialog({
        paneId: worker.paneId,
        log: () => {},
      }),
      { handled: false, reason: "path_mismatch" },
    );
  } finally {
    g.adapter.close();
    g.fake.cleanup();
  }
});
async function startedPm(h: Harness, screen = idleScreen()) {
  const pm = await h.adapter.createWorkspace({
    cwd: "/tmp",
    label: "proj",
    role: "PM",
  });
  await h.adapter.startAgent({
    name: "pm-1",
    kind: "claude",
    paneId: pm.paneId,
    args: [],
    environment: CLEAN_ENV,
  });
  const pane = h.fake.panes.get(pm.paneId)!;
  pane.screen = screen;
  h.fake.agentStates.set("pm-1", { paneId: pm.paneId, statuses: ["idle"] });
  h.fake.events.length = 0;
  return { paneId: pm.paneId, pane };
}
test("wakePm types the wake line into a started PM pane that is idle with an empty input line, and records before it types", async () => {
  const h = harness("acme");
  try {
    const { paneId } = await startedPm(h);
    const order: string[] = [];
    const outcome = await h.adapter.wakePm({
      paneId,
      text: "Run cstan inbox: a teammate has written to you.",
      beforeSend: () => {
        order.push(`record:${h.fake.events.length}`);
      },
    });
    assert.deepEqual(outcome, { sent: true });
    assert.deepEqual(order, ["record:0"], "the record comes before any typing");
    assert.deepEqual(h.fake.callsTo("agent", "prompt").at(-1), [
      "agent",
      "prompt",
      "acme-pm-1",
      "Run cstan inbox: a teammate has written to you.",
    ]);
  } finally {
    h.adapter.close();
    h.fake.cleanup();
  }
});
test("wakePm never types while the PM works or is blocked, with text on its input line, or when the line cannot be read", async () => {
  const h = harness();
  try {
    const { paneId, pane } = await startedPm(h);
    const wake = () =>
      h.adapter.wakePm({
        paneId,
        text: "Run cstan inbox",
        beforeSend: () => {},
      });
    for (const status of ["working", "blocked"]) {
      h.fake.agentStates.get("pm-1")!.statuses = [status];
      assert.deepEqual(
        await wake(),
        { sent: false, reason: "pm_not_idle" },
        status,
      );
    }
    h.fake.agentStates.get("pm-1")!.statuses = ["idle"];
    pane.screen = idleScreen("half a sentence");
    assert.deepEqual(await wake(), { sent: false, reason: "input_not_empty" });
    pane.screen = "nothing that looks like an input line";
    assert.deepEqual(await wake(), { sent: false, reason: "input_not_empty" });
    assert.equal(h.fake.callsTo("agent", "prompt").length, 0);
    h.fake.agentStates.get("pm-1")!.statuses = ["idle"];
    pane.screen = idleScreen();
    assert.deepEqual(await wake(), { sent: true });
  } finally {
    h.adapter.close();
    h.fake.cleanup();
  }
});
test("a state that changes between the two checks stops the wake before the keys, and a worker pane, an unregistered pane or a bad text is refused", async () => {
  const h = harness();
  try {
    const { paneId } = await startedPm(h);
    h.fake.agentStates.get("pm-1")!.statuses = ["idle", "working"];
    assert.deepEqual(
      await h.adapter.wakePm({
        paneId,
        text: "Run cstan inbox",
        beforeSend: () => {},
      }),
      { sent: false, reason: "pm_not_idle" },
    );
    assert.equal(h.fake.callsTo("agent", "prompt").length, 0);
    const worker = await startedWorker(h);
    await assert.rejects(
      h.adapter.wakePm({
        paneId: worker.paneId,
        text: "x y",
        beforeSend: () => {},
      }),
      PhaseError,
    );
    await assert.rejects(
      h.adapter.wakePm({ paneId: "w77:p1", text: "x y", beforeSend: () => {} }),
      UnknownPaneError,
    );
    h.fake.agentStates.get("pm-1")!.statuses = ["idle"];
    for (const text of ["/clear", "!ls", "a\nb", "x".repeat(20_000)])
      await assert.rejects(
        h.adapter.wakePm({ paneId, text, beforeSend: () => {} }),
        InvalidArgumentError,
      );
  } finally {
    h.adapter.close();
    h.fake.cleanup();
  }
});
// ---- permission prompt relay ------------------------------------------------

const promptFixture = (name: string): string =>
  readFileSync(path.resolve("test/fixtures/prompts", name), "utf8");
/** A Claude permission prompt that reacts to keys like the real one: arrows move, Tab opens the field of a text option, typed text lands in it, Enter and Esc end the prompt. */
class PromptScript {
  selected = 0;
  field: string | undefined;
  readonly base = parseHostPrompt(
    "claude",
    promptFixture("claude-bash-permission.ansi"),
  )!;
  readonly texts = this.base.options.map((option) => option.text);
  ended: "enter" | "esc" | undefined;
  /** Set to make Tab leave the screen as it was. */
  tabDoesNothing = false;
  /** Set to make a down key leave the selection where it was. */
  arrowsStuck = false;
  constructor(readonly pane: FakePane) {}

  render(): string {
    const rows = this.texts.map((text, index) => {
      const shown =
        index === this.selected && this.field !== undefined ? this.field : text;
      return ` ${index === this.selected ? "❯" : " "} ${index + 1}. ${shown}`;
    });
    return [
      "earlier output",
      "─".repeat(100),
      ...this.base.text.split("\n"),
      ...rows,
      "",
      this.field === undefined
        ? " Esc to cancel · Tab to amend"
        : " Esc to cancel",
    ].join("\r\n");
  }

  attach(h: Harness): void {
    this.pane.screen = this.render();
    h.fake.onKey = (pane, key) => {
      if (pane !== this.pane) return;
      if (key === "down" && !this.arrowsStuck)
        this.selected = Math.min(this.selected + 1, this.texts.length - 1);
      if (key === "up") this.selected = Math.max(this.selected - 1, 0);
      if (key === "tab" && !this.tabDoesNothing) {
        const wording = TEXT_FIELD_WORDING[this.texts[this.selected]!];
        if (wording !== undefined) this.field = wording;
      }
      if (key === "enter" || key === "esc") {
        this.ended = key;
        pane.status = "idle";
        pane.screen = idleScreen();
        return;
      }
      pane.screen = this.render();
    };
    h.fake.onText = (pane, text) => {
      if (pane !== this.pane || this.field === undefined) return;
      this.field = `${this.texts[this.selected]}, ${text}`;
      pane.screen = this.render();
    };
  }
}
async function promptWorker(
  h: Harness,
  kind: "claude" | "codex" | "omp" = "claude",
) {
  const worker = await startedWorker(h, idleScreen(), ["blocked"], kind);
  worker.pane.status = "blocked";
  const script = new PromptScript(worker.pane);
  script.attach(h);
  return { ...worker, script };
}
async function captured(h: Harness, paneId: string): Promise<CapturedPrompt> {
  const outcome = await h.adapter.capturePrompt(paneId);
  assert.ok(outcome.captured, JSON.stringify(outcome));
  return outcome.prompt;
}
const sentKeys = (h: Harness): string[] =>
  h.fake.events.filter((event) => /^(key|text):/.test(event));
test("capturePrompt reads the blocked Claude prompt and hashes it with the agent, pane and host", async () => {
  const h = harness();
  try {
    const w = await promptWorker(h);
    const prompt = await captured(h, w.paneId);
    assert.equal(prompt.agentId, "dev");
    assert.equal(prompt.paneId, w.paneId);
    assert.equal(prompt.hostKind, "claude");
    assert.deepEqual(
      prompt.options.map((option) => option.number),
      [1, 2, 3, 4],
    );
    assert.equal(
      prompt.promptSha,
      promptHash({
        agentId: "dev",
        paneId: w.paneId,
        hostKind: "claude",
        text: prompt.text,
        options: prompt.options,
      }),
    );
    assert.deepEqual(sentKeys(h), []);
  } finally {
    h.adapter.close();
    h.fake.cleanup();
  }
});
test("capturePrompt refuses an agent that is not blocked, one the name places elsewhere, another host and a screen that is not a proven dialog", async () => {
  for (const status of ["idle", "working", "done"]) {
    const h = harness();
    try {
      const w = await promptWorker(h);
      h.fake.agentStates.set("dev", { paneId: w.paneId, statuses: [status] });
      assert.deepEqual(await h.adapter.capturePrompt(w.paneId), {
        captured: false,
        reason: "not_blocked",
      });
    } finally {
      h.adapter.close();
      h.fake.cleanup();
    }
  }
  const elsewhere = harness();
  try {
    const w = await promptWorker(elsewhere);
    elsewhere.fake.agentStates.set("dev", {
      paneId: "w77:p1",
      statuses: ["blocked"],
    });
    assert.deepEqual(await elsewhere.adapter.capturePrompt(w.paneId), {
      captured: false,
      reason: "not_blocked",
    });
  } finally {
    elsewhere.adapter.close();
    elsewhere.fake.cleanup();
  }
  for (const kind of ["codex", "omp"] as const) {
    const h = harness();
    try {
      const w = await promptWorker(h, kind);
      assert.deepEqual(await h.adapter.capturePrompt(w.paneId), {
        captured: false,
        reason: "unsupported_host",
      });
    } finally {
      h.adapter.close();
      h.fake.cleanup();
    }
  }
  for (const name of [
    "synthetic-dialog-not-last.ansi",
    "synthetic-fake-above-input.ansi",
    "synthetic-two-selected.ansi",
  ]) {
    const h = harness();
    try {
      const w = await promptWorker(h);
      w.pane.screen = promptFixture(name);
      assert.deepEqual(await h.adapter.capturePrompt(w.paneId), {
        captured: false,
        reason: "prompt_unrecognized",
      });
    } finally {
      h.adapter.close();
      h.fake.cleanup();
    }
  }
});
test("an option answer sends only arrow keys, then Enter after a read shows the target selected; beforeType runs once before the first key", async () => {
  const h = harness();
  try {
    const w = await promptWorker(h);
    const prompt = await captured(h, w.paneId);
    h.fake.events.length = 0;
    const result = await answer(h, w.paneId, prompt.promptSha, {
      kind: "option",
      number: 4,
    });
    assert.deepEqual(result.outcome, {
      typed: true,
      keys: ["down", "down", "down", "enter"],
    });
    assert.equal(result.before, 1);
    assert.deepEqual(result.logged, [
      "before",
      "down",
      "down",
      "down",
      "enter",
    ]);
    assert.deepEqual(sentKeys(h), [
      "key:down",
      "key:down",
      "key:down",
      "key:enter",
    ]);
    assert.equal(w.script.ended, "enter");
  } finally {
    h.adapter.close();
    h.fake.cleanup();
  }
});
test("an option that widens permissions is typed like any other option, and an already selected option needs only Enter", async () => {
  const h = harness();
  try {
    const w = await promptWorker(h);
    const prompt = await captured(h, w.paneId);
    assert.equal(prompt.options[2]!.widensPermissions, true);
    h.fake.events.length = 0;
    const result = await answer(h, w.paneId, prompt.promptSha, {
      kind: "option",
      number: 3,
    });
    assert.deepEqual(result.outcome, {
      typed: true,
      keys: ["down", "down", "enter"],
    });
  } finally {
    h.adapter.close();
    h.fake.cleanup();
  }
  const again = harness();
  try {
    const w = await promptWorker(again);
    const prompt = await captured(again, w.paneId);
    again.fake.events.length = 0;
    const result = await answer(again, w.paneId, prompt.promptSha, {
      kind: "option",
      number: 1,
    });
    assert.deepEqual(result.outcome, { typed: true, keys: ["enter"] });
    assert.equal(result.before, 1);
  } finally {
    again.adapter.close();
    again.fake.cleanup();
  }
});
test("esc sends exactly one escape key", async () => {
  const h = harness();
  try {
    const w = await promptWorker(h);
    const prompt = await captured(h, w.paneId);
    h.fake.events.length = 0;
    const result = await answer(h, w.paneId, prompt.promptSha, {
      kind: "esc",
    });
    assert.deepEqual(result.outcome, { typed: true, keys: ["esc"] });
    assert.deepEqual(sentKeys(h), ["key:esc"]);
    assert.equal(result.before, 1);
  } finally {
    h.adapter.close();
    h.fake.cleanup();
  }
});
test("a stale hash returns prompt_changed with no key or text sent and beforeType not called", async () => {
  const h = harness();
  try {
    const w = await promptWorker(h);
    const prompt = await captured(h, w.paneId);
    h.fake.events.length = 0;
    for (const reply of [
      { kind: "option", number: 2 },
      { kind: "esc" },
      { kind: "text", number: 4, text: "no" },
    ] as const) {
      const result = await answer(h, w.paneId, "0".repeat(64), reply);
      assert.deepEqual(result.outcome, {
        typed: false,
        reason: "prompt_changed",
        keys: [],
      });
      assert.equal(result.before, 0);
    }
    assert.deepEqual(sentKeys(h), []);
    assert.equal(h.fake.callsTo("pane", "send-keys").length, 0);
    assert.equal(h.fake.callsTo("pane", "send-text").length, 0);
    void prompt;
  } finally {
    h.adapter.close();
    h.fake.cleanup();
  }
});
test("a screen that changes between the first read and the Enter returns prompt_changed and sends no Enter", async () => {
  const h = harness();
  try {
    const w = await promptWorker(h);
    const prompt = await captured(h, w.paneId);
    h.fake.events.length = 0;
    // The first read is the real prompt; the next read shows a different one.
    const changed = parseHostPrompt(
      "claude",
      promptFixture("claude-write-permission.ansi"),
    )!;
    const other = new PromptScript(w.pane);
    other.texts.splice(
      0,
      other.texts.length,
      ...changed.options.map((o) => o.text),
    );
    h.fake.readQueue.push(w.pane.screen, other.render());
    const result = await answer(h, w.paneId, prompt.promptSha, {
      kind: "option",
      number: 1,
    });
    assert.deepEqual(result.outcome, {
      typed: false,
      reason: "prompt_changed",
      keys: [],
    });
    assert.equal(result.before, 0);
    assert.deepEqual(sentKeys(h), []);
  } finally {
    h.adapter.close();
    h.fake.cleanup();
  }
});
test("a prompt that changes after the arrow keys stops before Enter and reports the keys already sent", async () => {
  const h = harness();
  try {
    const w = await promptWorker(h);
    const prompt = await captured(h, w.paneId);
    h.fake.events.length = 0;
    const original = h.fake.onKey!;
    h.fake.onKey = (pane, key) => {
      original(pane, key);
      w.script.texts[0] = "Yes, and delete everything";
      pane.screen = w.script.render();
    };
    const result = await answer(h, w.paneId, prompt.promptSha, {
      kind: "option",
      number: 2,
    });
    assert.deepEqual(result.outcome, {
      typed: false,
      reason: "prompt_changed",
      keys: ["down"],
    });
    assert.deepEqual(sentKeys(h), ["key:down"]);
  } finally {
    h.adapter.close();
    h.fake.cleanup();
  }
});
test("a selection that never reaches the target returns selection_not_reached and sends no Enter", async () => {
  const h = harness();
  try {
    const w = await promptWorker(h);
    const prompt = await captured(h, w.paneId);
    w.script.arrowsStuck = true;
    h.fake.events.length = 0;
    const result = await answer(h, w.paneId, prompt.promptSha, {
      kind: "option",
      number: 2,
    });
    assert.deepEqual(result.outcome, {
      typed: false,
      reason: "selection_not_reached",
      keys: ["down"],
    });
    assert.deepEqual(sentKeys(h), ["key:down"]);
  } finally {
    h.adapter.close();
    h.fake.cleanup();
  }
});
test("answerPrompt refuses with not_blocked and sends nothing when Herdr reports idle, working or done, or the name points at another pane", async () => {
  for (const status of ["idle", "working", "done"]) {
    const h = harness();
    try {
      const w = await promptWorker(h);
      const prompt = await captured(h, w.paneId);
      h.fake.agentStates.set("dev", { paneId: w.paneId, statuses: [status] });
      h.fake.events.length = 0;
      const result = await answer(h, w.paneId, prompt.promptSha, {
        kind: "option",
        number: 2,
      });
      assert.deepEqual(result.outcome, {
        typed: false,
        reason: "not_blocked",
        keys: [],
      });
      assert.equal(result.before, 0);
      assert.deepEqual(sentKeys(h), []);
    } finally {
      h.adapter.close();
      h.fake.cleanup();
    }
  }
  const h = harness();
  try {
    const w = await promptWorker(h);
    const prompt = await captured(h, w.paneId);
    h.fake.agentStates.set("dev", { paneId: "w77:p1", statuses: ["blocked"] });
    h.fake.events.length = 0;
    const result = await answer(h, w.paneId, prompt.promptSha, {
      kind: "esc",
    });
    assert.deepEqual(result.outcome, {
      typed: false,
      reason: "not_blocked",
      keys: [],
    });
    assert.deepEqual(sentKeys(h), []);
  } finally {
    h.adapter.close();
    h.fake.cleanup();
  }
});
test("an answer to an option that does not exist, a text answer to an option without a field, and a refused text are refused with nothing sent", async () => {
  const h = harness();
  try {
    const w = await promptWorker(h);
    const prompt = await captured(h, w.paneId);
    h.fake.events.length = 0;
    const cases: Array<[PromptAnswer, string]> = [
      [{ kind: "option", number: 5 }, "no_such_option"],
      [{ kind: "option", number: 0 }, "no_such_option"],
      [{ kind: "text", number: 9, text: "x" }, "no_such_option"],
      [{ kind: "text", number: 1, text: "fine" }, "no_text_option"],
      [{ kind: "text", number: 2, text: "fine" }, "no_text_option"],
      [{ kind: "text", number: 4, text: "two\nlines" }, "text_refused"],
      [{ kind: "text", number: 4, text: "bell\u0007" }, "text_refused"],
      [{ kind: "text", number: 4, text: "zero\u200bwidth" }, "text_refused"],
      [{ kind: "text", number: 4, text: "\ttab" }, "text_refused"],
      [{ kind: "text", number: 4, text: "/clear" }, "text_refused"],
      [{ kind: "text", number: 4, text: "!ls" }, "text_refused"],
      [{ kind: "text", number: 4, text: "#x" }, "text_refused"],
      [{ kind: "text", number: 4, text: "?x" }, "text_refused"],
      [{ kind: "text", number: 4, text: "@x" }, "text_refused"],
      [{ kind: "text", number: 4, text: "a".repeat(1001) }, "text_refused"],
      [{ kind: "text", number: 4, text: "lone\ud800" }, "text_refused"],
    ];
    for (const [reply, reason] of cases) {
      const result = await answer(h, w.paneId, prompt.promptSha, reply);
      assert.deepEqual(
        result.outcome,
        { typed: false, reason, keys: [] },
        JSON.stringify(reply),
      );
      assert.equal(result.before, 0);
    }
    assert.deepEqual(sentKeys(h), []);
  } finally {
    h.adapter.close();
    h.fake.cleanup();
  }
});
test("a text answer moves to the option, opens its field with Tab, types the literal text and presses Enter only after the field shows exactly that text", async () => {
  const h = harness();
  try {
    const w = await promptWorker(h);
    const prompt = await captured(h, w.paneId);
    h.fake.events.length = 0;
    const result = await answer(h, w.paneId, prompt.promptSha, {
      kind: "text",
      number: 4,
      text: "do not touch files",
    });
    assert.deepEqual(result.outcome, {
      typed: true,
      keys: ["down", "down", "down", "tab", "text", "enter"],
    });
    assert.equal(result.before, 1);
    assert.deepEqual(sentKeys(h), [
      "key:down",
      "key:down",
      "key:down",
      "key:tab",
      "text:do not touch files",
      "key:enter",
    ]);
    assert.equal(h.fake.callsTo("pane", "send-text").length, 1);
    assert.deepEqual(h.fake.callsTo("pane", "send-text")[0], [
      "pane",
      "send-text",
      w.paneId,
      "do not touch files",
    ]);
  } finally {
    h.adapter.close();
    h.fake.cleanup();
  }
});
test("a field that does not open stops after Tab without text, Enter or Esc", async () => {
  const h = harness();
  try {
    const w = await promptWorker(h);
    const prompt = await captured(h, w.paneId);
    w.script.tabDoesNothing = true;
    h.fake.events.length = 0;
    const result = await answer(h, w.paneId, prompt.promptSha, {
      kind: "text",
      number: 4,
      text: "hello",
    });
    assert.deepEqual(result.outcome, {
      typed: false,
      reason: "text_field_not_open",
      keys: ["down", "down", "down", "tab"],
    });
    assert.deepEqual(sentKeys(h), [
      "key:down",
      "key:down",
      "key:down",
      "key:tab",
    ]);
    assert.equal(w.script.ended, undefined);
  } finally {
    h.adapter.close();
    h.fake.cleanup();
  }
});
test("text that the field does not show exactly is not submitted: no Enter and no Esc follow", async () => {
  const h = harness();
  try {
    const w = await promptWorker(h);
    const prompt = await captured(h, w.paneId);
    h.fake.onText = (pane) => {
      w.script.field = "No, something else";
      pane.screen = w.script.render();
    };
    h.fake.events.length = 0;
    const result = await answer(h, w.paneId, prompt.promptSha, {
      kind: "text",
      number: 4,
      text: "hello",
    });
    assert.deepEqual(result.outcome, {
      typed: false,
      reason: "text_field_not_open",
      keys: ["down", "down", "down", "tab", "text"],
    });
    assert.equal(
      sentKeys(h).some((event) => /enter|esc/.test(event)),
      false,
    );
  } finally {
    h.adapter.close();
    h.fake.cleanup();
  }
});
test("the real open-field screens satisfy the structural check: same text, other options unchanged, only the target option and the footer differ", () => {
  for (const [closed, open] of [
    ["claude-bash-permission.ansi", "claude-bash-permission-no-textfield.ansi"],
    [
      "claude-write-permission.ansi",
      "claude-write-permission-yes-textfield.ansi",
    ],
  ] as const) {
    const before = parseHostPrompt("claude", promptFixture(closed))!;
    const after = parseHostPrompt("claude", promptFixture(open))!;
    const target = after.selectedIndex;
    assert.equal(after.text, before.text);
    assert.equal(
      after.options[target]!.text,
      TEXT_FIELD_WORDING[before.options[target]!.text],
    );
    assert.deepEqual(
      after.options.filter((_, index) => index !== target),
      before.options.filter((_, index) => index !== target),
    );
  }
  for (const [typed, expected] of [
    ["claude-bash-permission-no-typed.ansi", "No, do not touch files"],
    ["claude-write-permission-yes-typed.ansi", "Yes, go ahead"],
  ] as const) {
    const parsed = parseHostPrompt("claude", promptFixture(typed))!;
    assert.equal(parsed.options[parsed.selectedIndex]!.text, expected);
  }
});
const DIALOG_SCREEN = promptFixture("synthetic-dialog-teach-auto-mode.ansi");
async function dialogWorker(h: Harness, status = "idle") {
  const worker = await startedWorker(h, DIALOG_SCREEN, [status]);
  return worker;
}
test("capturePrompt captures the incident dialog as an Esc-only relay when Herdr is not working, and refuses it while working", async () => {
  for (const status of ["idle", "done", "blocked"]) {
    const h = harness();
    try {
      const w = await dialogWorker(h, status);
      const outcome = await h.adapter.capturePrompt(w.paneId);
      assert.ok(outcome.captured, status);
      assert.deepEqual(outcome.prompt.options, []);
      assert.equal(outcome.prompt.dialog, true);
      assert.match(outcome.prompt.promptSha, /^[0-9a-f]{64}$/);
      assert.match(
        outcome.prompt.text,
        /Teach auto mode about your environment\?/,
      );
      assert.equal(
        outcome.prompt.promptSha,
        promptHash({
          agentId: "dev",
          paneId: w.paneId,
          hostKind: "claude",
          text: outcome.prompt.text,
          options: [],
          dialog: true,
        }),
      );
      assert.deepEqual(sentKeys(h), []);
    } finally {
      h.adapter.close();
      h.fake.cleanup();
    }
  }
  const h = harness();
  try {
    const w = await dialogWorker(h, "working");
    assert.deepEqual(await h.adapter.capturePrompt(w.paneId), {
      captured: false,
      reason: "not_blocked",
    });
  } finally {
    h.adapter.close();
    h.fake.cleanup();
  }
});
test("a permission prompt still needs Herdr blocked and keeps its pinned hash", async () => {
  const h = harness();
  try {
    const w = await startedWorker(
      h,
      promptFixture("claude-bash-permission.ansi"),
      ["blocked"],
    );
    const prompt = await captured(h, w.paneId);
    assert.equal(prompt.dialog, undefined);
    const pinned = promptHash({
      agentId: "dev",
      paneId: "w1:p1",
      hostKind: "claude",
      text: prompt.text,
      options: prompt.options,
    });
    assert.equal(
      pinned,
      "65b106511d978b69335c9c35afb293d1bbd6f9253bbe94d247b9d8646119cc0b",
    );
    assert.equal(prompt.promptSha, pinned);
    h.fake.agentStates.set("dev", { paneId: w.paneId, statuses: ["idle"] });
    assert.deepEqual(await h.adapter.capturePrompt(w.paneId), {
      captured: false,
      reason: "not_blocked",
    });
  } finally {
    h.adapter.close();
    h.fake.cleanup();
  }
});
test("esc on a dialog relay sends exactly one esc key and reports whether the input box reads again", async () => {
  for (const reappears of [true, false]) {
    const h = harness();
    try {
      const w = await dialogWorker(h);
      const prompt = await captured(h, w.paneId);
      h.fake.onKey = (pane, key) => {
        if (key === "esc" && reappears) pane.screen = idleScreen();
      };
      h.fake.events.length = 0;
      const result = await answer(h, w.paneId, prompt.promptSha, {
        kind: "esc",
      });
      assert.deepEqual(result.outcome, {
        typed: true,
        keys: ["esc"],
        inputReadable: reappears,
      });
      assert.equal(result.before, 1);
      assert.deepEqual(result.logged, ["before", "esc"]);
      assert.deepEqual(sentKeys(h), ["key:esc"]);
    } finally {
      h.adapter.close();
      h.fake.cleanup();
    }
  }
});
test("a dialog relay refuses option and text answers, a changed screen and a screen that is no longer a dialog with no key sent", async () => {
  const h = harness();
  try {
    const w = await dialogWorker(h);
    const prompt = await captured(h, w.paneId);
    h.fake.events.length = 0;
    for (const reply of [
      { kind: "option", number: 1 },
      { kind: "text", number: 1, text: "hello" },
    ] as const)
      assert.deepEqual(
        (await answer(h, w.paneId, prompt.promptSha, reply)).outcome,
        {
          typed: false,
          reason: "no_such_option",
          keys: [],
        },
      );
    w.pane.screen = DIALOG_SCREEN.replace("Continue", "Proceed");
    const changed = await answer(h, w.paneId, prompt.promptSha, {
      kind: "esc",
    });
    assert.deepEqual(changed.outcome, {
      typed: false,
      reason: "prompt_changed",
      keys: [],
    });
    w.pane.screen = idleScreen();
    assert.deepEqual(
      (await answer(h, w.paneId, prompt.promptSha, { kind: "esc" })).outcome,
      { typed: false, reason: "prompt_unrecognized", keys: [] },
    );
    w.pane.screen = DIALOG_SCREEN;
    h.fake.agentStates.set("dev", { paneId: w.paneId, statuses: ["working"] });
    assert.deepEqual(
      (await answer(h, w.paneId, prompt.promptSha, { kind: "esc" })).outcome,
      { typed: false, reason: "not_blocked", keys: [] },
    );
    assert.deepEqual(sentKeys(h), []);
    assert.equal(changed.before, 0);
  } finally {
    h.adapter.close();
    h.fake.cleanup();
  }
});
test("guardedSend and clearAfterDeferral name what blocks an unreadable input line", async () => {
  const cases: Array<[string, string]> = [
    [DIALOG_SCREEN, "dialog"],
    [promptFixture("claude-bash-permission.ansi"), "permission_prompt"],
    ["nothing recognisable", "unknown"],
  ];
  for (const [screen, blocker] of cases) {
    const h = harness();
    try {
      const w = await startedWorker(h, screen);
      assert.deepEqual(
        await h.adapter.guardedSend({
          paneId: w.paneId,
          text: "hi",
          beforeSend: () => {},
        }),
        {
          sent: false,
          reason: "input_not_empty",
          detail: INPUT_UNREADABLE_DETAIL,
          blocker,
        },
        blocker,
      );
      await assert.rejects(
        h.adapter.clearAfterDeferral({
          paneId: w.paneId,
          deferredForMs: 100,
          maxDeferralMs: 100,
          discard: () => {},
          log: () => {},
        }),
        (error: unknown) =>
          error instanceof InputUnreadable && error.blocker === blocker,
        blocker,
      );
    } finally {
      h.adapter.close();
      h.fake.cleanup();
    }
  }
});
