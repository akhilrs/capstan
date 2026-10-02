import assert from "node:assert/strict";
import { test } from "node:test";
import {
  availableDecisions,
  confirmText,
  duplicateWarning,
  observeAction,
  toWireCall,
} from "../src/dash/actions.js";
import {
  MESSAGE_STATES,
  resolutionTarget,
  RESOLUTION_DECISIONS,
} from "../src/controller/messaging.js";
import { buildDashModel, type MessageRow } from "../src/dash/model.js";
import { NOW, healthy, message } from "./dash-fixtures.js";

function row(state: string, active = true): MessageRow {
  const model = buildDashModel(
    healthy({ messages: [message("0192abcd-0000", state)] }),
    NOW,
    3,
  );
  return { ...model.queue.messages[0]!, recipientActive: active };
}

test("the offered decisions equal the core's resolution table for every message state", () => {
  for (const state of MESSAGE_STATES) {
    const offered = availableDecisions(row(state));
    const expected = RESOLUTION_DECISIONS.filter(
      (d) => resolutionTarget(d, state) !== undefined,
    );
    assert.deepEqual(offered, expected, state);
  }
});

test("retry is not offered when the recipient has ended", () => {
  assert.deepEqual(availableDecisions(row("failed", false)), [
    "skip",
    "cancel",
  ]);
  assert.ok(availableDecisions(row("failed", true)).includes("retry"));
});

test("the confirm text names the action, the id, the recipient and the state", () => {
  const message = row("deferred");
  const text = confirmText({ kind: "resolve", decision: "skip", message });
  assert.match(
    text,
    /^Skip message 0192abcd-0000 to developer-agent \(state deferred\)\?/,
  );
  assert.match(text, /Press y again to confirm/);
  assert.ok(!/already have received/.test(text));
});

test("retrying a sent or unacked message warns about a duplicate delivery", () => {
  for (const state of ["sent", "unacked"]) {
    const message = row(state);
    assert.equal(duplicateWarning(message), true);
    assert.match(
      confirmText({ kind: "resolve", decision: "retry", message }),
      /may already have received it/,
    );
  }
  assert.equal(duplicateWarning(row("failed")), false);
});

test("actions map to the daemon routes that cstan resolve and cstan cancel use", () => {
  const message = row("failed");
  assert.deepEqual(
    toWireCall({ kind: "resolve", decision: "retry", message }),
    {
      command: "resolve",
      args: ["0192abcd-0000", "retry"],
    },
  );
  assert.deepEqual(toWireCall({ kind: "resolve", decision: "skip", message }), {
    command: "resolve",
    args: ["0192abcd-0000", "skip"],
  });
  assert.deepEqual(
    toWireCall({ kind: "resolve", decision: "cancel", message }),
    {
      command: "cancel",
      args: ["0192abcd-0000"],
    },
  );
  assert.deepEqual(
    toWireCall({ kind: "observe", agentId: "developer-agent" }),
    {
      command: "peek",
      args: ["developer-agent", "40"],
    },
  );
});

test("observe is offered for active agents only", () => {
  const [pm] = buildDashModel(healthy(), NOW, 3).agents;
  assert.deepEqual(observeAction(pm!), {
    kind: "observe",
    agentId: "pm-agent",
  });
  assert.equal(observeAction({ ...pm!, state: "ended" }), undefined);
});
