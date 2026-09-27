import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { spawnSync } from "node:child_process";
import { test } from "node:test";
import herdrBridge from "./m1-herdr-bridge.mjs";
import { ControllerCore } from "../dist/src/controller/core.js";
import { M1BridgeAdapter } from "../dist/src/controller/m1-bridge.js";

const environmentKeys = [
  "CAPSTAN_BRIDGE_ROLE",
  "CAPSTAN_BRIDGE_SOCKET",
  "CAPSTAN_BRIDGE_JOURNAL",
  "CAPSTAN_BRIDGE_RECEIPT_SOCKET",
  "CAPSTAN_BRIDGE_PEER_HELPER",
  "CAPSTAN_BRIDGE_CONTROLLER_PEER_PID",
];

function context(core, credential, tag) {
  const identity = `${tag}-${crypto.randomUUID()}`;
  return {
    credential,
    requestId: `req-${identity}`,
    idempotencyKey: `idem-${identity}`,
    expectedVersion: core.stateVersion,
    inputRevision: core.inputRevision,
  };
}

async function waitFor(predicate) {
  for (let attempt = 0; attempt < 200; attempt += 1) {
    if (predicate()) return;
    await new Promise((resolve) => setTimeout(resolve, 5));
  }
  assert.fail("timed out waiting for controller/bridge state");
}

test("controller dispatches to the real M1 bridge only after durable ack and persists its report", async () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "capstan-m1-adapter-"));
  const stateDirectory = path.join(root, "state");
  fs.mkdirSync(stateDirectory, { mode: 0o700 });
  const bridgeSocket = path.join(root, "bridge.sock");
  const receiptSocket = path.join(root, "receipt.sock");
  const journal = path.join(root, "bridge.jsonl");
  const peerHelper = path.join(root, "receipt-peer");
  const projectIdentity = crypto.randomUUID().replaceAll("-", "");
  const initialProject = {
    projectId: `p${projectIdentity}`,
    name: "M1 adapter integration",
    ownerCredential: `owner-${projectIdentity}`,
    initialInputs: [
      { kind: "project_config", content: { name: "M1 adapter integration" } },
      {
        kind: "task_brief",
        content: { objective: "Exercise the durable adapter" },
      },
      { kind: "acceptance_criteria", content: ["criterion-one"] },
      { kind: "policy", content: {} },
      { kind: "plan", content: {} },
    ],
  };
  const compiledPeer = spawnSync(
    "cc",
    [
      "-std=c11",
      "-O2",
      "-Wall",
      "-Wextra",
      "-Werror",
      path.resolve(import.meta.dirname, "m1-receipt-peer.c"),
      "-o",
      peerHelper,
    ],
    { encoding: "utf8" },
  );
  assert.equal(compiledPeer.status, 0, compiledPeer.stderr);
  fs.closeSync(fs.openSync(journal, "wx", 0o600));

  const priorEnvironment = Object.fromEntries(
    environmentKeys.map((key) => [key, process.env[key]]),
  );
  const handlers = new Map();
  const sentPrompts = [];
  const pi = {
    on(event, handler) {
      handlers.set(event, handler);
    },
    sendUserMessage(prompt) {
      sentPrompts.push(prompt);
      setImmediate(() => {
        void handlers.get("agent_start")?.();
      });
    },
  };
  let core;
  let adapter;
  try {
    core = await ControllerCore.open({
      stateDirectory,
      project: initialProject,
    });
    const owner = initialProject.ownerCredential;
    const pmSeat = "integration-pm-seat";
    const devSeat = "integration-dev-seat";
    const pmCredential = `pm-${crypto.randomUUID()}`;
    const developerCredential = `developer-${crypto.randomUUID()}`;
    core.createSeat(context(core, owner, "pm-seat"), {
      seatId: pmSeat,
      name: "PM",
      role: "PM",
    });
    core.createActor(context(core, owner, "pm-actor"), {
      displayName: "PM",
      role: "PM",
      credential: pmCredential,
      seatId: pmSeat,
    });
    core.createSeat(context(core, owner, "dev-seat"), {
      seatId: devSeat,
      name: "Developer",
      role: "Developer",
    });
    core.createActor(context(core, owner, "dev-actor"), {
      displayName: "Developer",
      role: "Developer",
      credential: developerCredential,
      seatId: devSeat,
    });
    core.createWorkItem(context(core, pmCredential, "work"), {
      workItemId: "integration-work",
      title: "Integration work",
      description: "Send one prompt through M1",
      requiredRole: "Developer",
    });
    core.markReady(context(core, owner, "ready"), "integration-work");
    const assignment = core.assignWorkItem(
      context(core, owner, "assign"),
      "integration-work",
      devSeat,
    );

    Object.assign(process.env, {
      CAPSTAN_BRIDGE_ROLE: "Developer",
      CAPSTAN_BRIDGE_SOCKET: bridgeSocket,
      CAPSTAN_BRIDGE_JOURNAL: journal,
      CAPSTAN_BRIDGE_RECEIPT_SOCKET: receiptSocket,
      CAPSTAN_BRIDGE_PEER_HELPER: peerHelper,
      CAPSTAN_BRIDGE_CONTROLLER_PEER_PID: String(process.pid),
    });
    adapter = new M1BridgeAdapter(core, receiptSocket, bridgeSocket);
    await adapter.listen();
    herdrBridge(pi);
    await handlers.get("session_start")({}, { isIdle: () => true, abort() {} });

    const dispatched = await adapter.dispatchAndStart(
      context(core, owner, "deliver"),
      assignment.commandId,
    );
    assert.equal(dispatched.commandId, assignment.commandId);
    assert.equal(
      sentPrompts.length,
      1,
      "one durable dispatch sends exactly one worker prompt",
    );
    const capsule = JSON.parse(sentPrompts[0]);
    assert.equal(capsule.workItem.workItemId, "integration-work");
    assert.equal(capsule.seat.role, "Developer");
    await handlers.get("turn_end")({
      message: {
        role: "assistant",
        content: [{ type: "text", text: "implementation submitted" }],
      },
    });
    await handlers.get("agent_end")({ willContinue: false });
    await waitFor(
      () => core.commandState(assignment.commandId) === "completed",
    );
    assert.equal(core.readiness("integration-work").ready, false);
    core.confirmContainment(
      context(core, owner, "contain"),
      assignment.assignmentId,
      "integration-proof:contained",
    );
    const candidate = core.submitCandidate(
      context(core, developerCredential, "candidate"),
      {
        candidateId: "integration-candidate",
        assignmentId: assignment.assignmentId,
        commitSha: "a".repeat(40),
        baseSha: "b".repeat(40),
        changedScope: ["src/controller"],
        limitations: [],
      },
    );
    assert.equal(candidate.candidateId, "integration-candidate");
  } finally {
    if (handlers.has("session_shutdown"))
      await handlers.get("session_shutdown")();
    await adapter?.close();
    core?.close();
    for (const key of environmentKeys) {
      if (priorEnvironment[key] === undefined) delete process.env[key];
      else process.env[key] = priorEnvironment[key];
    }
    fs.rmSync(root, { recursive: true, force: true });
  }
});
