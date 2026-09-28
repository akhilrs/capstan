import assert from "node:assert/strict";
import {
  mkdtempSync,
  readdirSync,
  readFileSync,
  rmSync,
  statSync,
} from "node:fs";
import net from "node:net";
import { tmpdir } from "node:os";
import path from "node:path";
import { spawnSync } from "node:child_process";
import { test } from "node:test";
import { createHash } from "node:crypto";
import Database from "better-sqlite3";
import {
  AuthenticationError,
  AuthorizationError,
  credentialHash,
} from "../src/controller/auth.js";
import {
  CandidateBindingError,
  ControllerCore,
  IdempotencyConflictError,
  InputRevisionConflictError,
  MutationConflictError,
  ReadinessError,
  StateVersionConflictError,
  TransitionAuthorizationError,
} from "../src/controller/core.js";
import { WorkflowScheduler } from "../src/controller/scheduler.js";
import { validateWorkflowPlan } from "../src/controller/workflow.js";
import { M1BridgeAdapter } from "../src/controller/m1-bridge.js";
import {
  M1_MAX_PROMPT_BYTES,
  M1ReceiptFrameParser,
  M1ResponseFrameParser,
  parseM1Frame,
} from "../src/controller/m1-protocol.js";
import type {
  AssignmentResult,
  BridgeReceipt,
  InitialProject,
  MutationContext,
  Role,
} from "../src/controller/types.js";
import { canonicalJson, digestJson } from "../src/controller/canonical.js";
test("canonical JSON rejects accessor-backed values without invoking them", () => {
  let reads = 0;
  const value = Object.defineProperty({}, "content", {
    enumerable: true,
    get: () => {
      reads += 1;
      return reads === 1 ? "first" : "later";
    },
  });

  assert.throws(() => canonicalJson(value), /accessor properties/);
  assert.equal(reads, 0);
});
test("canonical JSON rejects ill-formed UTF-16", () => {
  assert.throws(
    () => canonicalJson("lone-surrogate:\ud800"),
    /ill-formed UTF-16/,
  );
});
test("canonical JSON rejects ill-formed UTF-16 object keys", () => {
  const invalidKey = { ["key:\ud800"]: "value" };
  assert.throws(() => canonicalJson(invalidKey), /ill-formed UTF-16/);
});
test("canonical JSON rejects non-enumerable and extra array properties", () => {
  const hidden = Object.defineProperty({ title: "visible" }, "description", {
    value: "hidden",
    enumerable: false,
  });
  const extra = Object.assign(["criterion"], { hidden: "not serialized" });
  assert.throws(() => canonicalJson(hidden), /non-enumerable properties/);
  const invalidIndex = Object.assign([], { "4294967295": "not-an-index" });
  assert.throws(() => canonicalJson(extra), /extra properties/);
  assert.throws(() => canonicalJson(invalidIndex), /extra properties/);
});

const inputKinds = [
  "project_config",
  "task_brief",
  "acceptance_criteria",
  "policy",
  "plan",
] as const;

function project(): InitialProject {
  const identity = crypto.randomUUID().replaceAll("-", "");
  return {
    projectId: `p${identity}`,
    name: "Controller test project",
    ownerCredential: `owner-${identity}`,
    initialInputs: inputKinds.map((kind) => ({
      kind,
      content:
        kind === "acceptance_criteria"
          ? ["criterion-one"]
          : { kind, revision: 1 },
    })),
  };
}

interface Fixture {
  readonly core: ControllerCore;
  readonly stateDirectory: string;
  readonly project: InitialProject;
}

async function fixture(runtimeWorkspacePath?: string): Promise<Fixture> {
  const stateDirectory = mkdtempSync(
    path.join(tmpdir(), "capstan-controller-test-"),
  );
  const info = project();
  const core = await ControllerCore.open({
    stateDirectory,
    project: info,
    ...(runtimeWorkspacePath === undefined ? {} : { runtimeWorkspacePath }),
  });
  return { core, stateDirectory, project: info };
}

function context(
  core: ControllerCore,
  credential: string,
  prefix = "test",
): MutationContext {
  const id = `${prefix}-${crypto.randomUUID()}`;
  return {
    credential,
    requestId: `req-${id}`,
    idempotencyKey: `idem-${id}`,
    expectedVersion: core.stateVersion,
    inputRevision: core.inputRevision,
  };
}

function cleanup(value: Fixture): void {
  value.core.close();
  rmSync(value.stateDirectory, { recursive: true, force: true });
}
test("read-only reopen observes uncertain foreground state without mutation", async () => {
  const value = await fixture();
  let core = value.core;
  try {
    const developer = await addSeatAndActor(
      core,
      value.project.ownerCredential,
      "Developer",
      "offline-status",
    );
    core.createWorkItem(context(core, value.project.ownerCredential), {
      workItemId: "offline-status-work",
      title: "Offline status",
      description: "Preserve uncertain state during inspection",
      requiredRole: "Developer",
    });
    core.markReady(
      context(core, value.project.ownerCredential),
      "offline-status-work",
    );
    const assignment = core.assignWorkItem(
      context(core, value.project.ownerCredential),
      "offline-status-work",
      developer.seatId,
    );
    core.beginCommandDelivery(
      context(core, value.project.ownerCredential),
      assignment.commandId,
    );
    const expectedVersion = core.stateVersion;
    const expectedRunState = core.statusSnapshot().run.state;
    const expectedCommandState = core.commandState(assignment.commandId);
    const expectedInspect = core.inspect("offline-status-work");
    core.close();

    const snapshotFiles = () =>
      new Map(
        readdirSync(value.stateDirectory)
          .filter(
            (name) =>
              !name.endsWith("-shm") &&
              (!name.endsWith("-wal") ||
                statSync(path.join(value.stateDirectory, name)).size > 0),
          )
          .map((name) => [
            name,
            createHash("sha256")
              .update(readFileSync(path.join(value.stateDirectory, name)))
              .digest("hex"),
          ]),
      );
    const before = snapshotFiles();
    const readOnly = await ControllerCore.openReadOnly({
      stateDirectory: value.stateDirectory,
      project: value.project,
    });
    core = readOnly;
    const status = readOnly.statusSnapshot();
    assert.equal(status.run.state, expectedRunState);
    assert.equal(
      readOnly.commandState(assignment.commandId),
      expectedCommandState,
    );
    assert.deepEqual(readOnly.inspect("offline-status-work"), expectedInspect);
    assert.equal(readOnly.stateVersion, expectedVersion);
    assert.throws(
      () =>
        readOnly.markReady(
          context(readOnly, value.project.ownerCredential),
          "offline-status-work",
        ),
      /read-only/,
    );
    readOnly.close();
    assert.deepEqual(snapshotFiles(), before);
  } finally {
    cleanup({ ...value, core });
  }
});
test("project initialization rejects unusable acceptance criteria", async () => {
  const stateDirectory = mkdtempSync(
    path.join(tmpdir(), "capstan-invalid-criteria-"),
  );
  const info = project();
  const invalidProject = {
    ...info,
    initialInputs: info.initialInputs.map((input) =>
      input.kind === "acceptance_criteria"
        ? { ...input, content: ["  "] }
        : input,
    ),
  };
  await assert.rejects(
    ControllerCore.open({
      stateDirectory,
      project: invalidProject,
    }),
    /acceptance criteria must be a non-empty list/,
  );
  rmSync(stateDirectory, { recursive: true, force: true });
});
test("project initialization rejects sparse acceptance criteria", async () => {
  const stateDirectory = mkdtempSync(
    path.join(tmpdir(), "capstan-sparse-criteria-"),
  );
  const info = project();
  const sparseCriteria: string[] = [];
  sparseCriteria.length = 2;
  sparseCriteria[1] = "criterion";
  Object.setPrototypeOf(
    sparseCriteria,
    Object.assign(Object.create(Array.prototype), { 0: "inherited criterion" }),
  );
  const invalidProject = {
    ...info,
    initialInputs: info.initialInputs.map((input) =>
      input.kind === "acceptance_criteria"
        ? { ...input, content: sparseCriteria }
        : input,
    ),
  };
  await assert.rejects(
    ControllerCore.open({
      stateDirectory,
      project: invalidProject,
    }),
    /acceptance criteria must be a non-empty list/,
  );
  rmSync(stateDirectory, { recursive: true, force: true });
});
test("project initialization persists the validated criteria snapshot", async () => {
  const stateDirectory = mkdtempSync(
    path.join(tmpdir(), "capstan-changing-criteria-"),
  );
  const info = project();
  let reads = 0;
  let kindReads = 0;
  const changingProject = {
    ...info,
    initialInputs: info.initialInputs.map((input) =>
      input.kind === "acceptance_criteria"
        ? {
            get kind() {
              kindReads += 1;
              return kindReads === 1 ? input.kind : "policy";
            },
            get content() {
              reads += 1;
              return reads === 1 ? ["criterion"] : [];
            },
          }
        : input,
    ),
  };
  const core = await ControllerCore.open({
    stateDirectory,
    project: changingProject,
  });
  try {
    assert.equal(reads, 1);
    assert.equal(kindReads, 1);
    const db = new Database(path.join(stateDirectory, "controller.sqlite"), {
      readonly: true,
    });
    try {
      const row = db
        .prepare(
          "SELECT content_json FROM project_revisions WHERE project_id = ? AND kind = 'acceptance_criteria'",
        )
        .get(info.projectId) as { content_json: string };
      assert.deepEqual(JSON.parse(row.content_json), ["criterion"]);
    } finally {
      db.close();
    }
  } finally {
    core.close();
    rmSync(stateDirectory, { recursive: true, force: true });
  }
});
test("project initialization rejects ill-formed names", async () => {
  const stateDirectory = mkdtempSync(
    path.join(tmpdir(), "capstan-invalid-name-"),
  );
  const info = project();
  await assert.rejects(
    ControllerCore.open({
      stateDirectory,
      project: { ...info, name: "invalid:\ud800" },
    }),
    /ill-formed UTF-16/,
  );
  rmSync(stateDirectory, { recursive: true, force: true });
});

function receipt(
  identity: {
    commandId: string;
    assignmentId: string;
    attempt: number;
    generation: number;
  },
  sequence: number,
  type: BridgeReceipt["type"],
  role = "Developer",
): BridgeReceipt {
  return {
    ...identity,
    sequence,
    type,
    role,
    timestamp: new Date(Date.UTC(2026, 0, 1, 0, 0, sequence)).toISOString(),
    ...(type === "completed"
      ? {
          reply: "work finished",
          evidenceRef: { journal: "/tmp/journal", sequence },
        }
      : {}),
  };
}

async function addSeatAndActor(
  core: ControllerCore,
  ownerCredential: string,
  role: Exclude<Role, "operator" | "controller">,
  suffix: string,
) {
  const seatId = `${suffix}-seat`;
  core.createSeat(context(core, ownerCredential), {
    seatId,
    name: `${role} ${suffix}`,
    role,
  });
  const actor = core.createActor(context(core, ownerCredential), {
    displayName: `${role} ${suffix}`,
    role,
    seatId,
  });
  return { seatId, credential: actor.credential, actorId: actor.actorId };
}

test("credential hashing rejects ill-formed UTF-16", () => {
  const prefix = "c".repeat(31);
  assert.throws(() => credentialHash(`${prefix}\ud800`), AuthenticationError);
  assert.throws(() => credentialHash(`${prefix}\udc00`), AuthenticationError);
  assert.equal(credentialHash(`${prefix}\ud83d\ude00`).length, 64);
});

test("actor credentials are issued by the controller and replay exactly", async () => {
  const value = await fixture();
  try {
    const { core, project: info } = value;
    core.createSeat(context(core, info.ownerCredential), {
      seatId: "issued-seat",
      name: "Issued PM",
      role: "PM",
    });
    const request = context(core, info.ownerCredential);
    const input = {
      displayName: "Issued PM",
      role: "PM" as const,
      seatId: "issued-seat",
    };
    const actor = core.createActor(request, input);
    assert.equal(actor.credential.length >= 32, true);
    assert.deepEqual(core.createActor(request, input), actor);
    const db = new Database(
      path.join(value.stateDirectory, "controller.sqlite"),
      { readonly: true },
    );
    try {
      const stored = db
        .prepare(
          "SELECT result_json FROM mutation_requests WHERE project_id = ? AND idempotency_key = ?",
        )
        .get(info.projectId, request.idempotencyKey) as {
        result_json: string;
      };
      assert.equal(stored.result_json.includes(actor.credential), false);
    } finally {
      db.close();
    }
    assert.throws(
      () =>
        core.createActor(context(core, info.ownerCredential), {
          ...input,
          displayName: "Another PM",
        }),
      /seat already has an active actor/,
    );
    core.createSeat(context(core, info.ownerCredential), {
      seatId: "snapshot-seat",
      name: "Snapshot PM",
      role: "PM",
    });
    let displayReads = 0;
    const snapshotRequest = context(core, info.ownerCredential);
    const snapshotActor = core.createActor(snapshotRequest, {
      get displayName() {
        displayReads += 1;
        return displayReads === 1 ? "Snapshot PM" : "Divergent PM";
      },
      role: "PM",
      seatId: "snapshot-seat",
    });
    assert.equal(displayReads, 1);
    assert.deepEqual(
      core.createActor(snapshotRequest, {
        displayName: "Snapshot PM",
        role: "PM",
        seatId: "snapshot-seat",
      }),
      snapshotActor,
    );
    core.createWorkItem(context(core, actor.credential), {
      workItemId: "issued-actor-work",
      title: "Authenticated by issued token",
      description: "The actor uses its returned credential",
      requiredRole: "Developer",
    });
    core.close();
    const reopened = await ControllerCore.open({
      stateDirectory: value.stateDirectory,
      project: info,
    });
    try {
      assert.deepEqual(reopened.createActor(request, input), actor);
    } finally {
      reopened.close();
    }
  } finally {
    cleanup(value);
  }
});

test("migration ledger gaps reject startup even when later migration checksums match", async () => {
  const value = await fixture();
  try {
    value.core.close();
    const db = new Database(
      path.join(value.stateDirectory, "controller.sqlite"),
    );
    try {
      db.prepare("DELETE FROM schema_migrations WHERE version = 2").run();
    } finally {
      db.close();
    }
    await assert.rejects(
      ControllerCore.open({
        stateDirectory: value.stateDirectory,
        project: value.project,
      }),
      /migration ledger has a gap before version 3/,
    );
  } finally {
    cleanup(value);
  }
});

test("M1 response parser rejects a fragmented late duplicate", () => {
  const parser = new M1ResponseFrameParser();
  const response = {
    type: "ack",
    commandId: "command-1",
    durable: true,
    state: "acknowledged",
  };
  const responseLine = `${JSON.stringify(response)}\n`;
  assert.deepEqual(parser.push(Buffer.from(responseLine)), [response]);
  const duplicate = `${responseLine}`;
  const split = Math.floor(duplicate.length / 2);
  assert.deepEqual(parser.push(Buffer.from(duplicate.slice(0, split))), []);
  assert.throws(
    () => parser.push(Buffer.from(duplicate.slice(split))),
    /unexpected frame after bridge response/,
  );
  const incompleteTrailer = new M1ResponseFrameParser();
  assert.deepEqual(
    incompleteTrailer.push(Buffer.from(`${responseLine}{"type":`)),
    [],
  );
  assert.throws(() => incompleteTrailer.finish(), /incomplete frame/);
});
test("receipt framing accepts one newline frame independent of TCP chunk boundaries", () => {
  const frame = Buffer.from('{"type":"accepted"}\n');
  const suffix = Buffer.alloc(1_048_577, 0x78);
  const combined = new M1ReceiptFrameParser();
  assert.deepEqual(
    parseM1Frame(combined.push(Buffer.concat([frame, suffix]))!),
    { type: "accepted" },
  );
  assert.equal(combined.push(suffix), undefined);

  const fragmented = new M1ReceiptFrameParser();
  assert.deepEqual(parseM1Frame(fragmented.push(frame)!), {
    type: "accepted",
  });
  assert.equal(fragmented.push(suffix), undefined);
});
test("oversized UTF-8 M1 prompt stays ready without committing an assignment", async () => {
  const value = await fixture();
  try {
    const { core, project: info } = value;
    const developer = await addSeatAndActor(
      core,
      info.ownerCredential,
      "Developer",
      "oversized",
    );
    core.createWorkItem(context(core, info.ownerCredential), {
      workItemId: "oversized-work",
      title: "Oversized dispatch",
      description: "😀".repeat(M1_MAX_PROMPT_BYTES / 4),
      requiredRole: "Developer",
    });
    core.markReady(context(core, info.ownerCredential), "oversized-work");
    const version = core.stateVersion;
    assert.throws(
      () =>
        core.assignWorkItem(
          context(core, info.ownerCredential),
          "oversized-work",
          developer.seatId,
        ),
      /M1 dispatch prompt exceeds its byte limit/,
    );
    assert.equal(core.stateVersion, version);
    assert.equal(core.readiness("oversized-work").ready, true);
  } finally {
    cleanup(value);
  }
});
test("M1 receipt acknowledgement does not require sender EOF", async () => {
  const value = await fixture();
  const bridgeSocket = path.join(value.stateDirectory, "unused-bridge.sock");
  const receiptSocket = path.join(value.stateDirectory, "receipt-no-eof.sock");
  let adapter: M1BridgeAdapter | undefined;
  try {
    const { core, project: info } = value;
    const developer = await addSeatAndActor(
      core,
      info.ownerCredential,
      "Developer",
      "receipt-no-eof",
    );
    core.createWorkItem(context(core, info.ownerCredential), {
      workItemId: "receipt-no-eof-work",
      title: "Acknowledge a complete frame",
      description: "Bridge sender waits for the reply before closing",
      requiredRole: "Developer",
    });
    core.markReady(context(core, info.ownerCredential), "receipt-no-eof-work");
    const assignment = core.assignWorkItem(
      context(core, info.ownerCredential),
      "receipt-no-eof-work",
      developer.seatId,
    );
    core.beginCommandDelivery(
      context(core, info.ownerCredential),
      assignment.commandId,
    );
    adapter = new M1BridgeAdapter(core, receiptSocket, bridgeSocket, {
      allowUnauthenticatedLocalPeers: true,
    });
    await adapter.listen();
    const response = await new Promise<string>((resolve, reject) => {
      const socket = net.createConnection(receiptSocket);
      socket.once("error", reject);
      socket.once("connect", () =>
        socket.write(
          `${JSON.stringify(
            receipt(
              {
                commandId: assignment.commandId,
                assignmentId: assignment.assignmentId,
                attempt: assignment.attempt,
                generation: assignment.generation,
              },
              1,
              "accepted",
            ),
          )}\n`,
        ),
      );
      socket.once("data", (data) => {
        resolve(data.toString("utf8"));
        socket.end();
      });
    });
    assert.deepEqual(JSON.parse(response), {
      ok: true,
      sequence: 1,
      duplicate: false,
    });
    assert.equal(core.commandState(assignment.commandId), "acknowledged");
  } finally {
    await adapter?.close();
    cleanup(value);
  }
});

test("M1 adapter rejects duplicate responses after acknowledgement", async () => {
  const value = await fixture();
  const bridgeSocket = path.join(value.stateDirectory, "bridge.sock");
  const receiptSocket = path.join(value.stateDirectory, "receipt.sock");
  const requests: string[] = [];
  const server = net.createServer((socket) => {
    socket.once("data", (chunk) => {
      const request = JSON.parse(chunk.toString("utf8")) as {
        type: string;
        commandId: string;
        assignmentId?: string;
        attempt?: number;
        generation?: number;
        singleResponse?: boolean;
      };
      requests.push(request.type);
      if (request.type === "dispatch") {
        assert.equal(request.singleResponse, true);
        value.core.recordBridgeReceipt(
          receipt(
            {
              commandId: request.commandId,
              assignmentId: request.assignmentId!,
              attempt: request.attempt!,
              generation: request.generation!,
            },
            1,
            "accepted",
          ),
        );
      }
      const ack = JSON.stringify({
        type: "ack",
        commandId: request.commandId,
        durable: true,
        state: "acknowledged",
      });
      socket.end(`${ack}\n${ack}\n`);
    });
  });
  await new Promise<void>((resolve, reject) => {
    server.once("error", reject);
    server.listen(bridgeSocket, resolve);
  });
  let adapter: M1BridgeAdapter | undefined;
  try {
    const { core, project: info } = value;
    const developer = await addSeatAndActor(
      core,
      info.ownerCredential,
      "Developer",
      "double-frame",
    );
    core.createWorkItem(context(core, info.ownerCredential), {
      workItemId: "double-frame-work",
      title: "Double response",
      description: "Reject an extra bridge response frame",
      requiredRole: "Developer",
    });
    core.markReady(context(core, info.ownerCredential), "double-frame-work");
    const assignment = core.assignWorkItem(
      context(core, info.ownerCredential),
      "double-frame-work",
      developer.seatId,
    );
    adapter = new M1BridgeAdapter(core, receiptSocket, bridgeSocket, {
      allowUnauthenticatedLocalPeers: true,
    });
    await adapter.listen();
    await assert.rejects(
      adapter.dispatchAndStart(
        context(core, info.ownerCredential),
        assignment.commandId,
      ),
      /unexpected frame after bridge response/,
    );
    assert.equal(core.commandState(assignment.commandId), "acknowledged");
    await assert.rejects(
      adapter.dispatchAndStart(
        context(core, info.ownerCredential),
        assignment.commandId,
      ),
      /unexpected frame after bridge response/,
    );
    assert.deepEqual(requests, ["dispatch", "get"]);
    assert.equal(core.commandState(assignment.commandId), "acknowledged");
  } finally {
    await adapter?.close();
    await new Promise<void>((resolve, reject) =>
      server.close((error) => (error ? reject(error) : resolve())),
    );
    cleanup(value);
  }
});
test("M1 adapter inspects uncertain command without dispatch or authority restoration", async () => {
  const value = await fixture();
  const bridgeSocket = path.join(value.stateDirectory, "inspect-bridge.sock");
  const receiptSocket = path.join(value.stateDirectory, "inspect-receipt.sock");
  const requests: unknown[] = [];
  const server = net.createServer((socket) => {
    socket.once("data", (chunk) => {
      const request = JSON.parse(chunk.toString("utf8")) as {
        type: string;
        commandId: string;
      };
      requests.push(request);
      const response =
        requests.length === 1
          ? {
              type: "ack",
              commandId: request.commandId,
              durable: true,
              state: "acknowledged",
            }
          : {
              type: "completed",
              commandId: request.commandId,
              durable: true,
              reply: "completed without a durable controller report",
            };
      socket.end(`${JSON.stringify(response)}\n`);
    });
  });
  await new Promise<void>((resolve, reject) => {
    server.once("error", reject);
    server.listen(bridgeSocket, resolve);
  });
  let adapter: M1BridgeAdapter | undefined;
  let core = value.core;
  try {
    const info = value.project;
    const developer = await addSeatAndActor(
      core,
      info.ownerCredential,
      "Developer",
      "inspect-unknown",
    );
    core.createWorkItem(context(core, info.ownerCredential), {
      workItemId: "inspect-unknown-work",
      title: "Inspect unknown",
      description: "An M1 status query must not issue another prompt",
      requiredRole: "Developer",
    });
    core.markReady(context(core, info.ownerCredential), "inspect-unknown-work");
    const assignment = core.assignWorkItem(
      context(core, info.ownerCredential),
      "inspect-unknown-work",
      developer.seatId,
    );
    core.beginCommandDelivery(
      context(core, info.ownerCredential),
      assignment.commandId,
    );
    adapter = new M1BridgeAdapter(core, receiptSocket, bridgeSocket, {
      allowUnauthenticatedLocalPeers: true,
    });
    await adapter.listen();
    const version = core.stateVersion;
    const inspected = await adapter.inspectUncertainCommand(
      assignment.commandId,
    );
    assert.deepEqual(inspected, {
      commandId: assignment.commandId,
      bridgeState: "acknowledged",
      durable: true,
    });
    assert.deepEqual(requests, [
      { type: "get", commandId: assignment.commandId },
    ]);
    assert.equal(core.stateVersion, version);
    assert.equal(core.commandState(assignment.commandId), "attempting");
    assert.throws(
      () =>
        core.confirmContainment(
          context(core, info.ownerCredential),
          assignment.assignmentId,
          "proof:premature-containment",
          inspected,
        ),
      /reconcile the same command/,
    );
    assert.equal(core.readiness("inspect-unknown-work").ready, false);
    core.close();
    core = await ControllerCore.open({
      stateDirectory: value.stateDirectory,
      project: info,
    });
    assert.equal(core.commandState(assignment.commandId), "unknown");
    assert.deepEqual(
      core.confirmContainment(
        context(core, info.ownerCredential),
        assignment.assignmentId,
        "proof:bridge-queried-and-worker-quiescent",
        inspected,
      ),
      { contained: true },
    );
    assert.deepEqual(requests, [
      { type: "get", commandId: assignment.commandId },
    ]);
    assert.equal(core.readiness("inspect-unknown-work").ready, true);
    const recovery = core.recordRecovery(context(core, info.ownerCredential), {
      recoveryId: "inspect-unknown-recovery",
      workItemId: "inspect-unknown-work",
      assignmentId: assignment.assignmentId,
      recoveryType: "worker_replacement",
      reason: "Bridge state inspected and worker quiescent",
    });
    assert.equal(recovery.outcome, "pending");
    core.markReady(context(core, info.ownerCredential), "inspect-unknown-work");
    const replacement = core.assignWorkItem(
      context(core, info.ownerCredential),
      "inspect-unknown-work",
      developer.seatId,
      undefined,
      recovery.recoveryId,
    );
    assert.notEqual(replacement.assignmentId, assignment.assignmentId);
  } finally {
    core.close();
    await adapter?.close();
    await new Promise<void>((resolve, reject) =>
      server.close((error) => (error ? reject(error) : resolve())),
    );
    cleanup(value);
  }
});

test("private project ownership survives restart and ambiguous delivery is reconciled fail-closed", async () => {
  const value = await fixture();
  const { core, stateDirectory, project: info } = value;
  try {
    assert.equal(statSync(stateDirectory).mode & 0o077, 0);
    const developer = await addSeatAndActor(
      core,
      info.ownerCredential,
      "Developer",
      "dev",
    );
    core.createWorkItem(context(core, info.ownerCredential), {
      workItemId: "restart-work",
      title: "Restart work",
      description: "Bounded task",
      requiredRole: "Developer",
    });
    core.markReady(context(core, info.ownerCredential), "restart-work");
    const assignment = core.assignWorkItem(
      context(core, info.ownerCredential),
      "restart-work",
      developer.seatId,
    );
    core.beginCommandDelivery(
      context(core, info.ownerCredential),
      assignment.commandId,
    );
    assert.equal(core.commandState(assignment.commandId), "attempting");
    await assert.rejects(
      ControllerCore.open({ stateDirectory, project: info }),
      /lock|ownership|already/i,
    );
    const coreModule = new URL("../src/controller/core.js", import.meta.url)
      .href;
    const childSource = `
      import { ControllerCore } from ${JSON.stringify(coreModule)};
      const options = ${JSON.stringify({ stateDirectory, project: info })};
      try {
        const second = await ControllerCore.open(options);
        second.close();
        process.exitCode = 4;
      } catch (error) {
        if (/lock|ownership|held|owns/i.test(String(error?.message))) process.exitCode = 0;
        else { console.error(error); process.exitCode = 5; }
      }
    `;
    const child = spawnSync(
      process.execPath,
      ["--input-type=module", "-e", childSource],
      {
        encoding: "utf8",
      },
    );
    assert.equal(child.status, 0, child.stderr);
    core.close();
    const reopened = await ControllerCore.open({
      stateDirectory,
      project: info,
    });
    try {
      assert.equal(reopened.commandState(assignment.commandId), "unknown");
      const versionBeforePreStartContainment = reopened.stateVersion;
      assert.throws(
        () =>
          reopened.confirmContainment(
            context(reopened, info.ownerCredential),
            assignment.assignmentId,
            "proof:never-started",
          ),
        /start was not durably requested; reconcile the same command/,
      );
      assert.equal(reopened.stateVersion, versionBeforePreStartContainment);
      assert.equal(reopened.readiness("restart-work").ready, false);
      assert.match(
        reopened.readiness("restart-work").reasons.join(";"),
        /blocked|assignment/i,
      );
    } finally {
      reopened.close();
    }
  } finally {
    rmSync(stateDirectory, { recursive: true, force: true });
  }
});
test("restart containment preserves a durable PM report for acceptance", async () => {
  const value = await fixture();
  const { stateDirectory, project: info } = value;
  let core = value.core;
  try {
    const pm = await addSeatAndActor(
      core,
      info.ownerCredential,
      "PM",
      "restart-pm-report",
    );
    core.createWorkItem(context(core, info.ownerCredential), {
      workItemId: "restart-pm-report-work",
      title: "Persisted PM report",
      description: "Accept a durable report after restart containment",
      requiredRole: "PM",
    });
    core.markReady(
      context(core, info.ownerCredential),
      "restart-pm-report-work",
    );
    const assignment = core.assignWorkItem(
      context(core, info.ownerCredential),
      "restart-pm-report-work",
      pm.seatId,
    );
    core.beginCommandDelivery(
      context(core, info.ownerCredential),
      assignment.commandId,
    );
    const identity = {
      commandId: assignment.commandId,
      assignmentId: assignment.assignmentId,
      attempt: assignment.attempt,
      generation: assignment.generation,
    };
    core.recordBridgeReceipt(receipt(identity, 1, "accepted", "PM"));
    core.beginCommandStart(
      context(core, info.ownerCredential),
      assignment.commandId,
    );
    core.recordBridgeReceipt(receipt(identity, 2, "submitted", "PM"));
    core.recordBridgeReceipt(receipt(identity, 3, "working", "PM"));
    core.recordBridgeReceipt(receipt(identity, 4, "completed", "PM"));
    core.close();
    core = await ControllerCore.open({ stateDirectory, project: info });
    assert.equal(core.commandState(assignment.commandId), "completed");
    const versionBeforeUncontainedAccept = core.stateVersion;
    assert.throws(
      () =>
        core.acceptNonCandidateReport(
          context(core, info.ownerCredential),
          "restart-pm-report-work",
          assignment.assignmentId,
        ),
      /not contained/,
    );
    assert.equal(core.stateVersion, versionBeforeUncontainedAccept);
    core.confirmContainment(
      context(core, info.ownerCredential),
      assignment.assignmentId,
      "restart-containment:pm-report",
    );
    assert.deepEqual(
      core.acceptNonCandidateReport(
        context(core, info.ownerCredential),
        "restart-pm-report-work",
        assignment.assignmentId,
      ),
      { acceptedWorkItemId: "restart-pm-report-work" },
    );
    const versionBeforeLateRevision = core.stateVersion;
    assert.throws(
      () =>
        core.recordInputRevision(context(core, info.ownerCredential), {
          kind: "acceptance_criteria",
          content: ["new criterion"],
        }),
      /project inputs cannot change after work has been accepted/,
    );
    assert.equal(core.stateVersion, versionBeforeLateRevision);
    assert.deepEqual(
      core.transitionRun(context(core, info.ownerCredential), "completed"),
      { state: "completed" },
    );
  } finally {
    core.close();
    cleanup(value);
  }
});
test("readiness requires an active actor bound to the required-role seat", async () => {
  const value = await fixture();
  try {
    const { core, project: info } = value;
    core.createSeat(context(core, info.ownerCredential), {
      seatId: "actor-required-seat",
      name: "Actor required",
      role: "Developer",
    });
    core.createSeat(context(core, info.ownerCredential), {
      seatId: "actor-required-empty-seat",
      name: "Unstaffed alternate",
      role: "Developer",
    });
    core.createWorkItem(context(core, info.ownerCredential), {
      workItemId: "actor-required-work",
      title: "Actor-required work",
      description: "Do not dispatch without a receipt principal",
      requiredRole: "Developer",
    });
    assert.throws(
      () =>
        core.markReady(
          context(core, info.ownerCredential),
          "actor-required-work",
        ),
      /no active Developer seat with an active actor/,
    );
    core.createActor(context(core, info.ownerCredential), {
      displayName: "Developer",
      role: "Developer",
      seatId: "actor-required-seat",
    });
    assert.deepEqual(
      core.markReady(
        context(core, info.ownerCredential),
        "actor-required-work",
      ),
      { state: "ready" },
    );
    assert.deepEqual(
      core
        .statusSnapshot()
        .work.find((work) => work.workItemId === "actor-required-work")
        ?.nextLegalActions,
      ["assign"],
    );
    assert.throws(
      () =>
        core.assignWorkItem(
          context(core, info.ownerCredential),
          "actor-required-work",
          "actor-required-empty-seat",
        ),
      /assignment seat must be active, match the work item role, and have an active actor/,
    );
    core.transitionRun(context(core, info.ownerCredential), "canceling");
    assert.deepEqual(
      core
        .statusSnapshot()
        .work.find((work) => work.workItemId === "actor-required-work")
        ?.nextLegalActions,
      ["wait"],
    );
  } finally {
    cleanup(value);
  }
});

test("queued assignment can be contained after restart before any delivery", async () => {
  const value = await fixture();
  const { project: info } = value;
  let core = value.core;
  try {
    const developer = await addSeatAndActor(
      core,
      info.ownerCredential,
      "Developer",
      "queued-containment",
    );
    core.createWorkItem(context(core, info.ownerCredential), {
      workItemId: "queued-containment-work",
      title: "Queued work",
      description: "Abandon work known never delivered",
      requiredRole: "Developer",
    });
    core.markReady(
      context(core, info.ownerCredential),
      "queued-containment-work",
    );
    const assignment = core.assignWorkItem(
      context(core, info.ownerCredential),
      "queued-containment-work",
      developer.seatId,
    );
    core.close();
    core = await ControllerCore.open({
      stateDirectory: value.stateDirectory,
      project: info,
    });
    assert.deepEqual(
      core.confirmContainment(
        context(core, info.ownerCredential),
        assignment.assignmentId,
        "proof:command-never-delivered",
      ),
      { contained: true },
    );
    assert.throws(
      () =>
        core.beginCommandDelivery(
          context(core, info.ownerCredential),
          assignment.commandId,
        ),
      /contained|delivery/,
    );
  } finally {
    core.close();
    cleanup(value);
  }
});

test("unstarted command authority cannot be marked contained", async () => {
  const value = await fixture();
  try {
    const { core, project: info } = value;
    const developer = await addSeatAndActor(
      core,
      info.ownerCredential,
      "Developer",
      "prestart-containment",
    );
    core.createWorkItem(context(core, info.ownerCredential), {
      workItemId: "prestart-containment-work",
      title: "Unstarted work",
      description: "Containment cannot predate start",
      requiredRole: "Developer",
    });
    core.markReady(
      context(core, info.ownerCredential),
      "prestart-containment-work",
    );
    const assignment = core.assignWorkItem(
      context(core, info.ownerCredential),
      "prestart-containment-work",
      developer.seatId,
    );
    core.beginCommandDelivery(
      context(core, info.ownerCredential),
      assignment.commandId,
    );
    assert.throws(
      () =>
        core.confirmContainment(
          context(core, info.ownerCredential),
          assignment.assignmentId,
          "pre-start-containment",
        ),
      /start was not durably requested/,
    );
    const identity = {
      commandId: assignment.commandId,
      assignmentId: assignment.assignmentId,
      attempt: assignment.attempt,
      generation: assignment.generation,
    };
    core.recordBridgeReceipt(receipt(identity, 1, "accepted"));
    const versionBeforeAcknowledgedContainment = core.stateVersion;
    assert.throws(
      () =>
        core.confirmContainment(
          context(core, info.ownerCredential),
          assignment.assignmentId,
          "proof:acknowledged-but-never-started",
        ),
      /assignment has no running or uncertain authority to contain/,
    );
    assert.equal(core.stateVersion, versionBeforeAcknowledgedContainment);
    core.close();
    const reopened = await ControllerCore.open({
      stateDirectory: value.stateDirectory,
      project: info,
    });
    try {
      assert.throws(
        () =>
          reopened.confirmContainment(
            context(reopened, info.ownerCredential),
            assignment.assignmentId,
            "proof:accepted-but-never-started",
          ),
        /start was not durably requested; reconcile the same command/,
      );
    } finally {
      reopened.close();
    }
  } finally {
    cleanup(value);
  }
});

test("contained worker receipts are audited without blocking replacement receipts", async () => {
  const value = await fixture();
  try {
    const { core, project: info } = value;
    const developer = await addSeatAndActor(
      core,
      info.ownerCredential,
      "Developer",
      "late-receipt",
    );
    core.createWorkItem(context(core, info.ownerCredential), {
      workItemId: "late-receipt-work",
      title: "Recover stopped work",
      description: "Containment must fence the old generation",
      requiredRole: "Developer",
    });
    core.markReady(context(core, info.ownerCredential), "late-receipt-work");
    const assignment = core.assignWorkItem(
      context(core, info.ownerCredential),
      "late-receipt-work",
      developer.seatId,
    );
    core.beginCommandDelivery(
      context(core, info.ownerCredential),
      assignment.commandId,
    );
    const identity = {
      commandId: assignment.commandId,
      assignmentId: assignment.assignmentId,
      attempt: assignment.attempt,
      generation: assignment.generation,
    };
    core.recordBridgeReceipt(receipt(identity, 1, "accepted"));
    core.beginCommandStart(
      context(core, info.ownerCredential),
      assignment.commandId,
    );
    core.recordBridgeReceipt(receipt(identity, 2, "working"));
    core.confirmContainment(
      context(core, info.ownerCredential),
      assignment.assignmentId,
      "proof:stopped-old-worker",
    );
    assert.equal(core.readiness("late-receipt-work").ready, true);
    core.markReady(context(core, info.ownerCredential), "late-receipt-work");
    assert.deepEqual(
      core.recordBridgeReceipt(receipt(identity, 3, "completed")),
      {
        duplicate: false,
        fenced: true,
      },
    );
    assert.equal(core.commandState(assignment.commandId), "started");
    assert.equal(core.readiness("late-receipt-work").ready, true);
    const recovery = core.recordRecovery(context(core, info.ownerCredential), {
      recoveryId: "late-receipt-recovery",
      workItemId: "late-receipt-work",
      assignmentId: assignment.assignmentId,
      recoveryType: "worker_replacement",
      reason: "old worker stopped",
    });
    assert.equal(recovery.outcome, "pending");
    const replacement = core.assignWorkItem(
      context(core, info.ownerCredential),
      "late-receipt-work",
      developer.seatId,
      undefined,
      recovery.recoveryId,
    );
    assert.notEqual(replacement.assignmentId, assignment.assignmentId);
    core.beginCommandDelivery(
      context(core, info.ownerCredential),
      replacement.commandId,
    );
    core.recordBridgeReceipt(
      receipt(
        {
          commandId: replacement.commandId,
          assignmentId: replacement.assignmentId,
          attempt: replacement.attempt,
          generation: replacement.generation,
        },
        4,
        "accepted",
      ),
    );
    assert.equal(core.commandState(replacement.commandId), "acknowledged");
  } finally {
    cleanup(value);
  }
});

test("an ambiguous pre-start receipt cannot be contained without M1 start intent", async () => {
  const value = await fixture();
  try {
    const { core, project: info } = value;
    const developer = await addSeatAndActor(
      core,
      info.ownerCredential,
      "Developer",
      "prestart-error",
    );
    core.createWorkItem(context(core, info.ownerCredential), {
      workItemId: "prestart-error-work",
      title: "Ambiguous dispatch",
      description: "M1 start was never requested",
      requiredRole: "Developer",
    });
    core.markReady(context(core, info.ownerCredential), "prestart-error-work");
    const assignment = core.assignWorkItem(
      context(core, info.ownerCredential),
      "prestart-error-work",
      developer.seatId,
    );
    core.beginCommandDelivery(
      context(core, info.ownerCredential),
      assignment.commandId,
    );
    core.recordBridgeReceipt(
      receipt(
        {
          commandId: assignment.commandId,
          assignmentId: assignment.assignmentId,
          attempt: assignment.attempt,
          generation: assignment.generation,
        },
        1,
        "dispatch_error",
      ),
    );
    assert.throws(
      () =>
        core.confirmContainment(
          context(core, info.ownerCredential),
          assignment.assignmentId,
          "proof:dispatch-error-before-start",
        ),
      /start was not durably requested/,
    );
    for (const snapshot of [
      {
        commandId: "other-command",
        bridgeState: "acknowledged",
        durable: true,
      },
      {
        commandId: assignment.commandId,
        bridgeState: "running",
        durable: true,
      },
    ]) {
      assert.throws(
        () =>
          core.confirmContainment(
            context(core, info.ownerCredential),
            assignment.assignmentId,
            "proof:invalid-bridge-snapshot",
            snapshot,
          ),
        /reconcile the same command/,
      );
    }
    assert.equal(core.readiness("prestart-error-work").ready, false);
  } finally {
    cleanup(value);
  }
});

test("the last active worker actor cannot be revoked during assigned authority", async () => {
  const value = await fixture();
  try {
    const { core, project: info } = value;
    const developer = await addSeatAndActor(
      core,
      info.ownerCredential,
      "Developer",
      "revocation-active-work",
    );
    core.createWorkItem(context(core, info.ownerCredential), {
      workItemId: "revocation-active-work",
      title: "Worker receipt after revocation attempt",
      description: "The active seat principal must remain able to report",
      requiredRole: "Developer",
    });
    core.markReady(
      context(core, info.ownerCredential),
      "revocation-active-work",
    );
    const assignment = core.assignWorkItem(
      context(core, info.ownerCredential),
      "revocation-active-work",
      developer.seatId,
    );
    core.beginCommandDelivery(
      context(core, info.ownerCredential),
      assignment.commandId,
    );
    const identity = {
      commandId: assignment.commandId,
      assignmentId: assignment.assignmentId,
      attempt: assignment.attempt,
      generation: assignment.generation,
    };
    core.recordBridgeReceipt(receipt(identity, 1, "accepted"));
    core.beginCommandStart(
      context(core, info.ownerCredential),
      assignment.commandId,
    );
    const versionBeforeRevocation = core.stateVersion;
    assert.throws(
      () =>
        core.revokeActor(
          context(core, info.ownerCredential),
          developer.actorId,
        ),
      /last active actor.*active or uncertain assignments/,
    );
    assert.equal(core.stateVersion, versionBeforeRevocation);
    core.recordBridgeReceipt(receipt(identity, 2, "submitted"));
    core.recordBridgeReceipt(receipt(identity, 3, "working"));
    core.recordBridgeReceipt(receipt(identity, 4, "completed"));
    assert.equal(core.commandState(assignment.commandId), "completed");
  } finally {
    cleanup(value);
  }
});
test("working receipts require durable controller start intent", async () => {
  const value = await fixture();
  try {
    const { core, project: info } = value;
    const developer = await addSeatAndActor(
      core,
      info.ownerCredential,
      "Developer",
      "missing-start-intent",
    );
    core.createWorkItem(context(core, info.ownerCredential), {
      workItemId: "missing-start-intent-work",
      title: "Unrequested start",
      description: "Reject bridge progress without a stored start request",
      requiredRole: "Developer",
    });
    core.markReady(
      context(core, info.ownerCredential),
      "missing-start-intent-work",
    );
    const assignment = core.assignWorkItem(
      context(core, info.ownerCredential),
      "missing-start-intent-work",
      developer.seatId,
    );
    core.beginCommandDelivery(
      context(core, info.ownerCredential),
      assignment.commandId,
    );
    const identity = {
      commandId: assignment.commandId,
      assignmentId: assignment.assignmentId,
      attempt: assignment.attempt,
      generation: assignment.generation,
    };
    core.recordBridgeReceipt(receipt(identity, 1, "accepted"));
    const versionBeforeWorking = core.stateVersion;
    assert.throws(
      () => core.recordBridgeReceipt(receipt(identity, 2, "submitted")),
      /start intent/,
    );
    assert.throws(
      () => core.recordBridgeReceipt(receipt(identity, 2, "working")),
      /without durable start intent/,
    );
    assert.equal(core.commandState(assignment.commandId), "acknowledged");
    assert.equal(core.stateVersion, versionBeforeWorking);
  } finally {
    cleanup(value);
  }
});

test("a seat cannot receive a second assignment before prior authority is contained", async () => {
  const value = await fixture();
  try {
    const { core, project: info } = value;
    const developer = await addSeatAndActor(
      core,
      info.ownerCredential,
      "Developer",
      "single-active-assignment",
    );
    for (const workItemId of ["seat-work-a", "seat-work-b"]) {
      core.createWorkItem(context(core, info.ownerCredential), {
        workItemId,
        title: workItemId,
        description: "Keep one active assignment per runtime seat",
        requiredRole: "Developer",
      });
      core.markReady(context(core, info.ownerCredential), workItemId);
    }
    core.assignWorkItem(
      context(core, info.ownerCredential),
      "seat-work-a",
      developer.seatId,
    );
    const versionBeforeRejectedAssignment = core.stateVersion;
    assert.throws(
      () =>
        core.assignWorkItem(
          context(core, info.ownerCredential),
          "seat-work-b",
          developer.seatId,
        ),
      MutationConflictError,
    );
    assert.equal(core.stateVersion, versionBeforeRejectedAssignment);
    assert.equal(core.readiness("seat-work-b").ready, true);
  } finally {
    cleanup(value);
  }
});

test("runs reject completion while work is open", async () => {
  const value = await fixture();
  try {
    const { core, project: info } = value;
    core.createWorkItem(context(core, info.ownerCredential), {
      workItemId: "open-run-work",
      title: "Open work",
      description: "This work has not been accepted or canceled",
      requiredRole: "Developer",
    });
    const versionBefore = core.stateVersion;
    assert.throws(
      () =>
        core.transitionRun(context(core, info.ownerCredential), "completed"),
      MutationConflictError,
    );
    assert.equal(core.stateVersion, versionBefore);
  } finally {
    cleanup(value);
  }
});

test("unresolved Supervisor finding blocks completion after its report is accepted", async () => {
  const value = await fixture();
  try {
    const { core, project: info } = value;
    const supervisor = await addSeatAndActor(
      core,
      info.ownerCredential,
      "Supervisor",
      "blocking-supervisor",
    );
    core.createWorkItem(context(core, info.ownerCredential), {
      workItemId: "blocking-observation",
      title: "Inspect accepted delivery",
      description: "Report any unresolved issue",
      requiredRole: "Supervisor",
    });
    core.markReady(context(core, info.ownerCredential), "blocking-observation");
    const assignment = core.assignWorkItem(
      context(core, info.ownerCredential),
      "blocking-observation",
      supervisor.seatId,
    );
    core.createFinding(context(core, supervisor.credential), {
      findingId: "supervisor-blocker",
      workItemId: "blocking-observation",
      assignmentId: assignment.assignmentId,
      generation: assignment.generation,
      fingerprint: "unresolved-acceptance",
      severity: "high",
      evidence: { observation: "integration fails" },
      requestedCorrection: "Fix integration",
      resolutionCondition: "Rerun final acceptance",
    });
    const identity = {
      commandId: assignment.commandId,
      assignmentId: assignment.assignmentId,
      attempt: assignment.attempt,
      generation: assignment.generation,
    };
    core.beginCommandDelivery(
      context(core, info.ownerCredential),
      identity.commandId,
    );
    core.recordBridgeReceipt(receipt(identity, 1, "accepted", "Supervisor"));
    core.beginCommandStart(
      context(core, info.ownerCredential),
      identity.commandId,
    );
    core.recordBridgeReceipt(receipt(identity, 2, "submitted", "Supervisor"));
    core.recordBridgeReceipt(receipt(identity, 3, "working", "Supervisor"));
    core.recordBridgeReceipt(receipt(identity, 4, "completed", "Supervisor"));
    core.confirmContainment(
      context(core, info.ownerCredential),
      assignment.assignmentId,
      "containment:blocking-supervisor",
    );
    core.acceptNonCandidateReport(
      context(core, info.ownerCredential),
      "blocking-observation",
      assignment.assignmentId,
    );
    assert.equal(
      core
        .statusSnapshot()
        .work.find((work) => work.workItemId === "blocking-observation")?.state,
      "accepted",
    );
    assert.throws(
      () =>
        core.transitionRun(context(core, info.ownerCredential), "completed"),
      /unresolved Supervisor findings/,
    );
    assert.equal(core.statusSnapshot().run.state, "active");
  } finally {
    cleanup(value);
  }
});

test("operator requests can invoke controller-only terminal run transitions", async () => {
  const value = await fixture();
  try {
    const { core, project: info } = value;
    assert.deepEqual(
      core.transitionRun(context(core, info.ownerCredential), "completed"),
      { state: "completed" },
    );
    assert.throws(
      () =>
        core.transitionRun(context(core, info.ownerCredential), "completed"),
      TransitionAuthorizationError,
    );
    const versionBeforeCreation = core.stateVersion;
    assert.throws(
      () =>
        core.createWorkItem(context(core, info.ownerCredential), {
          workItemId: "post-terminal-work",
          title: "After run completion",
          description: "Terminal runs cannot accept new work",
          requiredRole: "Developer",
        }),
      ReadinessError,
    );
    assert.equal(core.stateVersion, versionBeforeCreation);
    const inputRevisionBefore = core.inputRevision;
    assert.throws(
      () =>
        core.recordInputRevision(context(core, info.ownerCredential), {
          kind: "policy",
          content: { afterCompletion: true },
        }),
      /run is completed/,
    );
    assert.equal(core.inputRevision, inputRevisionBefore);
    assert.equal(core.stateVersion, versionBeforeCreation);
    assert.throws(
      () => core.readiness("post-terminal-work"),
      /work item does not exist/,
    );
  } finally {
    cleanup(value);
  }
});
test("PM and Supervisor reports complete through durable role-authorized receipts", async () => {
  const value = await fixture();
  try {
    const { core, project: info } = value;
    const pm = await addSeatAndActor(
      core,
      info.ownerCredential,
      "PM",
      "pm-report",
    );
    const supervisor = await addSeatAndActor(
      core,
      info.ownerCredential,
      "Supervisor",
      "supervisor-report",
    );
    const reportDependent = await addSeatAndActor(
      core,
      info.ownerCredential,
      "Developer",
      "report-dependent",
    );
    const reports = [
      { role: "PM" as const, suffix: "pm", actor: pm },
      {
        role: "Supervisor" as const,
        suffix: "supervisor",
        actor: supervisor,
      },
    ];
    for (const { role, suffix, actor: worker } of reports) {
      const workItemId = `report-${suffix}`;
      core.createWorkItem(context(core, info.ownerCredential), {
        workItemId,
        title: `${role} report`,
        description: "Complete and accept a non-candidate role report",
        requiredRole: role,
      });
      core.markReady(context(core, info.ownerCredential), workItemId);
      const assignment = core.assignWorkItem(
        context(core, info.ownerCredential),
        workItemId,
        worker.seatId,
      );
      const identity = {
        commandId: assignment.commandId,
        assignmentId: assignment.assignmentId,
        attempt: assignment.attempt,
        generation: assignment.generation,
      };
      core.beginCommandDelivery(
        context(core, info.ownerCredential),
        identity.commandId,
      );
      core.recordBridgeReceipt(receipt(identity, 1, "accepted", role));
      core.beginCommandStart(
        context(core, info.ownerCredential),
        identity.commandId,
      );
      core.recordBridgeReceipt(receipt(identity, 2, "submitted", role));
      core.recordBridgeReceipt(receipt(identity, 3, "working", role));
      const emptyReport = suffix === "supervisor";
      core.recordBridgeReceipt(
        emptyReport
          ? {
              ...receipt(identity, 4, "completed", role),
              reply: "   ",
            }
          : receipt(identity, 4, "completed", role),
      );
      assert.equal(core.commandState(identity.commandId), "completed");
      core.confirmContainment(
        context(core, info.ownerCredential),
        assignment.assignmentId,
        `containment:${suffix}`,
      );
      if (emptyReport) {
        const database = new Database(
          path.join(value.stateDirectory, "controller.sqlite"),
        );
        try {
          const completedReceipt = database
            .prepare(
              "SELECT sequence FROM command_receipts WHERE project_id = ? AND command_id = ? AND receipt_type = 'completed'",
            )
            .get(info.projectId, identity.commandId) as
            { sequence: number } | undefined;
          assert.equal(completedReceipt?.sequence, 4);
        } finally {
          database.close();
        }
        assert.throws(
          () =>
            core.acceptNonCandidateReport(
              context(core, info.ownerCredential),
              workItemId,
              assignment.assignmentId,
            ),
          MutationConflictError,
        );
        assert.deepEqual(
          core.recordRecovery(context(core, info.ownerCredential), {
            recoveryId: "empty-supervisor-report-remediation",
            workItemId,
            assignmentId: assignment.assignmentId,
            recoveryType: "implementation_remediation",
            reason: "Replace a completed report with no acceptance evidence",
          }).outcome,
          "pending",
        );
      } else {
        const recovery = core.recordRecovery(
          context(core, info.ownerCredential),
          {
            recoveryId: "replace-pm-report",
            workItemId,
            assignmentId: assignment.assignmentId,
            recoveryType: "implementation_remediation",
            reason: "Replace the original PM report",
          },
        );
        assert.equal(recovery.outcome, "pending");
        core.markReady(context(core, info.ownerCredential), workItemId);
        const replacement = core.assignWorkItem(
          context(core, info.ownerCredential),
          workItemId,
          worker.seatId,
          undefined,
          recovery.recoveryId,
        );
        const replacementIdentity = {
          commandId: replacement.commandId,
          assignmentId: replacement.assignmentId,
          attempt: replacement.attempt,
          generation: replacement.generation,
        };
        core.beginCommandDelivery(
          context(core, info.ownerCredential),
          replacement.commandId,
        );
        core.recordBridgeReceipt(
          receipt(replacementIdentity, 5, "accepted", role),
        );
        core.beginCommandStart(
          context(core, info.ownerCredential),
          replacement.commandId,
        );
        core.recordBridgeReceipt(
          receipt(replacementIdentity, 6, "submitted", role),
        );
        core.recordBridgeReceipt(
          receipt(replacementIdentity, 7, "working", role),
        );
        core.recordBridgeReceipt(
          receipt(replacementIdentity, 8, "completed", role),
        );
        core.confirmContainment(
          context(core, info.ownerCredential),
          replacement.assignmentId,
          "containment:replacement-pm",
        );
        assert.throws(
          () =>
            core.acceptNonCandidateReport(
              context(core, info.ownerCredential),
              workItemId,
              assignment.assignmentId,
            ),
          MutationConflictError,
        );
        assert.deepEqual(
          core.acceptNonCandidateReport(
            context(core, info.ownerCredential),
            workItemId,
            replacement.assignmentId,
          ),
          { acceptedWorkItemId: workItemId },
        );
      }
    }
    core.createWorkItem(context(core, info.ownerCredential), {
      workItemId: "uses-pm-report",
      title: "Use accepted PM report",
      description: "A non-candidate report can satisfy an unpinned dependency",
      requiredRole: "Developer",
    });
    core.addDependency(
      context(core, info.ownerCredential),
      "uses-pm-report",
      "report-pm",
    );
    assert.equal(core.readiness("uses-pm-report").ready, true);
    core.markReady(context(core, info.ownerCredential), "uses-pm-report");
    const dependentAssignment = core.assignWorkItem(
      context(core, info.ownerCredential),
      "uses-pm-report",
      reportDependent.seatId,
    );
    const dependentIdentity = {
      commandId: dependentAssignment.commandId,
      assignmentId: dependentAssignment.assignmentId,
      attempt: dependentAssignment.attempt,
      generation: dependentAssignment.generation,
    };
    core.beginCommandDelivery(
      context(core, info.ownerCredential),
      dependentIdentity.commandId,
    );
    core.recordBridgeReceipt(
      receipt(dependentIdentity, 1, "accepted", "Developer"),
    );
    core.beginCommandStart(
      context(core, info.ownerCredential),
      dependentIdentity.commandId,
    );
    core.recordBridgeReceipt(
      receipt(dependentIdentity, 2, "dispatch_error", "Developer"),
    );
    const versionWithUncontainedAssignment = core.stateVersion;
    assert.throws(
      () =>
        core.removeDependency(
          context(core, info.ownerCredential),
          "uses-pm-report",
          "report-pm",
        ),
      MutationConflictError,
    );
    assert.equal(core.stateVersion, versionWithUncontainedAssignment);
    core.confirmContainment(
      context(core, info.ownerCredential),
      dependentAssignment.assignmentId,
      "containment:stale-report-dependent",
    );
    const recoveryId = "stale-report-dependent-recovery";
    core.recordRecovery(context(core, info.ownerCredential), {
      recoveryId,
      workItemId: "uses-pm-report",
      assignmentId: dependentAssignment.assignmentId,
      recoveryType: "worker_replacement",
      reason: "Replan after stale accepted prerequisite",
    });
    const versionBeforeLatePolicy = core.stateVersion;
    assert.throws(
      () =>
        core.recordInputRevision(context(core, info.ownerCredential), {
          kind: "policy",
          content: { reportContextChanged: true },
        }),
      /project inputs cannot change after work has been accepted/,
    );
    assert.equal(core.stateVersion, versionBeforeLatePolicy);
    assert.deepEqual(
      core.removeDependency(
        context(core, info.ownerCredential),
        "uses-pm-report",
        "report-pm",
      ),
      { removed: true },
    );
    assert.equal(core.readiness("uses-pm-report").ready, true);
    assert.deepEqual(
      core.markReady(context(core, info.ownerCredential), "uses-pm-report"),
      { state: "ready" },
    );
    const replacementAssignment = core.assignWorkItem(
      context(core, info.ownerCredential),
      "uses-pm-report",
      reportDependent.seatId,
      undefined,
      recoveryId,
    );
    assert.equal(replacementAssignment.inputRevision, core.inputRevision);
    assert.notEqual(
      replacementAssignment.assignmentId,
      dependentAssignment.assignmentId,
    );
  } finally {
    cleanup(value);
  }
});
test("terminal runs reject acceptance of completed PM reports", async () => {
  const value = await fixture();
  try {
    const { core, project: info } = value;
    const pm = await addSeatAndActor(
      core,
      info.ownerCredential,
      "PM",
      "terminal-report",
    );
    core.createWorkItem(context(core, info.ownerCredential), {
      workItemId: "terminal-report-work",
      title: "Terminal report",
      description: "Do not accept worker reports after the run stops",
      requiredRole: "PM",
    });
    core.markReady(context(core, info.ownerCredential), "terminal-report-work");
    const assignment = core.assignWorkItem(
      context(core, info.ownerCredential),
      "terminal-report-work",
      pm.seatId,
    );
    const identity = {
      commandId: assignment.commandId,
      assignmentId: assignment.assignmentId,
      attempt: assignment.attempt,
      generation: assignment.generation,
    };
    core.beginCommandDelivery(
      context(core, info.ownerCredential),
      identity.commandId,
    );
    core.recordBridgeReceipt(receipt(identity, 1, "accepted", "PM"));
    core.beginCommandStart(
      context(core, info.ownerCredential),
      identity.commandId,
    );
    core.recordBridgeReceipt(receipt(identity, 2, "submitted", "PM"));
    core.recordBridgeReceipt(receipt(identity, 3, "working", "PM"));
    core.recordBridgeReceipt(receipt(identity, 4, "completed", "PM"));
    core.confirmContainment(
      context(core, info.ownerCredential),
      assignment.assignmentId,
      "terminal-report-contained",
    );
    core.transitionRun(context(core, info.ownerCredential), "canceling");
    core.transitionRun(context(core, info.ownerCredential), "canceled");
    const versionBeforeRejectedAcceptance = core.stateVersion;
    assert.throws(
      () =>
        core.acceptNonCandidateReport(
          context(core, info.ownerCredential),
          "terminal-report-work",
          assignment.assignmentId,
        ),
      /not contained, current, and eligible for acceptance/,
    );
    assert.equal(core.stateVersion, versionBeforeRejectedAcceptance);
  } finally {
    cleanup(value);
  }
});
test("developer completions cannot bypass candidate verification acceptance", async () => {
  const value = await fixture();
  try {
    const { core, project: info } = value;
    const developer = await addSeatAndActor(
      core,
      info.ownerCredential,
      "Developer",
      "report-role-guard",
    );
    core.createWorkItem(context(core, info.ownerCredential), {
      workItemId: "developer-report-work",
      title: "Developer completion",
      description: "Developer output needs independent candidate verification",
      requiredRole: "Developer",
    });
    core.markReady(
      context(core, info.ownerCredential),
      "developer-report-work",
    );
    const assignment = core.assignWorkItem(
      context(core, info.ownerCredential),
      "developer-report-work",
      developer.seatId,
    );
    const identity = {
      commandId: assignment.commandId,
      assignmentId: assignment.assignmentId,
      attempt: assignment.attempt,
      generation: assignment.generation,
    };
    core.beginCommandDelivery(
      context(core, info.ownerCredential),
      identity.commandId,
    );
    core.recordBridgeReceipt(receipt(identity, 1, "accepted"));
    core.beginCommandStart(
      context(core, info.ownerCredential),
      identity.commandId,
    );
    core.recordBridgeReceipt(receipt(identity, 2, "submitted"));
    core.recordBridgeReceipt(receipt(identity, 3, "working"));
    core.recordBridgeReceipt(receipt(identity, 4, "completed"));
    core.confirmContainment(
      context(core, info.ownerCredential),
      assignment.assignmentId,
      "developer-report-contained",
    );
    const versionBeforeRejectedAcceptance = core.stateVersion;
    assert.throws(
      () =>
        core.acceptNonCandidateReport(
          context(core, info.ownerCredential),
          "developer-report-work",
          assignment.assignmentId,
        ),
      /not contained, current, and eligible for acceptance/,
    );
    assert.equal(core.stateVersion, versionBeforeRejectedAcceptance);
  } finally {
    cleanup(value);
  }
});

test("finding responses persist their reports and require explicit resolution evidence", async () => {
  const value = await fixture();
  try {
    const { core, project: info } = value;
    const supervisor = await addSeatAndActor(
      core,
      info.ownerCredential,
      "Supervisor",
      "finding-supervisor",
    );
    const pm = await addSeatAndActor(
      core,
      info.ownerCredential,
      "PM",
      "finding-pm",
    );
    core.createWorkItem(context(core, info.ownerCredential), {
      workItemId: "finding-supervisor-work",
      title: "Supervisor report",
      description: "Create a finding",
      requiredRole: "Supervisor",
    });
    core.markReady(
      context(core, info.ownerCredential),
      "finding-supervisor-work",
    );
    const supervisorAssignment = core.assignWorkItem(
      context(core, info.ownerCredential),
      "finding-supervisor-work",
      supervisor.seatId,
    );
    core.createFinding(context(core, supervisor.credential), {
      findingId: "finding-1",
      workItemId: "finding-supervisor-work",
      assignmentId: supervisorAssignment.assignmentId,
      generation: supervisorAssignment.generation,
      fingerprint: "finding-fingerprint",
      severity: "medium",
      evidence: { source: "test" },
      requestedCorrection: "Address the issue",
      resolutionCondition: "Supervisor confirms correction",
    });
    assert.deepEqual(
      core.transitionFinding(
        context(core, supervisor.credential),
        "finding-1",
        "reported",
        { report: "Needs correction" },
      ),
      { state: "reported" },
    );
    const db = new Database(
      path.join(value.stateDirectory, "controller.sqlite"),
    );
    try {
      const report = db
        .prepare(
          "SELECT assignment_id, response_type, content_json FROM finding_responses WHERE project_id = ? AND finding_id = ?",
        )
        .get(info.projectId, "finding-1") as
        | {
            assignment_id: string;
            response_type: string;
            content_json: string;
          }
        | undefined;
      assert.deepEqual(report, {
        assignment_id: supervisorAssignment.assignmentId,
        response_type: "report",
        content_json: '{"report":"Needs correction"}',
      });
      assert.throws(
        () =>
          db
            .prepare(
              "UPDATE finding_responses SET content_json = ? WHERE project_id = ? AND finding_id = ?",
            )
            .run("{}", info.projectId, "finding-1"),
        /finding responses are immutable/,
      );
      assert.throws(
        () =>
          db
            .prepare(
              "DELETE FROM finding_responses WHERE project_id = ? AND finding_id = ?",
            )
            .run(info.projectId, "finding-1"),
        /finding responses are immutable/,
      );
    } finally {
      db.close();
    }
    const versionBeforeUnassignedAck = core.stateVersion;
    assert.throws(
      () =>
        core.transitionFinding(
          context(core, pm.credential),
          "finding-1",
          "acknowledged",
          { acknowledgment: "I will review" },
        ),
      /active responding child assignment/,
    );
    assert.equal(core.stateVersion, versionBeforeUnassignedAck);
    core.createWorkItem(context(core, info.ownerCredential), {
      workItemId: "finding-pm-work",
      parentWorkItemId: "finding-supervisor-work",
      title: "PM report",
      description: "Acknowledge the finding",
      requiredRole: "PM",
    });
    core.markReady(context(core, info.ownerCredential), "finding-pm-work");
    core.assignWorkItem(
      context(core, info.ownerCredential),
      "finding-pm-work",
      pm.seatId,
    );
    assert.deepEqual(
      core.transitionFinding(
        context(core, pm.credential),
        "finding-1",
        "acknowledged",
        { acknowledgment: "I will review" },
      ),
      { state: "acknowledged" },
    );
    core.transitionFinding(
      context(core, info.ownerCredential),
      "finding-1",
      "correcting",
      { correction: "Change is ready for review" },
    );
    const responseDb = new Database(
      path.join(value.stateDirectory, "controller.sqlite"),
    );
    try {
      assert.deepEqual(
        responseDb
          .prepare(
            "SELECT response_type FROM finding_responses WHERE project_id = ? AND finding_id = ? ORDER BY response_type",
          )
          .all(info.projectId, "finding-1"),
        [{ response_type: "acknowledged" }, { response_type: "report" }],
      );
    } finally {
      responseDb.close();
    }
    const versionBeforeInvalidResolution = core.stateVersion;
    assert.throws(
      () =>
        core.transitionFinding(
          context(core, info.ownerCredential),
          "finding-1",
          "resolved",
          null,
        ),
      /repeat its condition and include non-empty evidence/,
    );
    assert.throws(
      () =>
        core.transitionFinding(
          context(core, info.ownerCredential),
          "finding-1",
          "resolved",
          { condition: "different condition", evidence: "evidence://proof" },
        ),
      /repeat its condition and include non-empty evidence/,
    );
    assert.equal(core.stateVersion, versionBeforeInvalidResolution);
    assert.deepEqual(
      core.transitionFinding(
        context(core, info.ownerCredential),
        "finding-1",
        "resolved",
        {
          condition: "Supervisor confirms correction",
          evidence: "evidence://correction-check",
        },
      ),
      { state: "resolved" },
    );
  } finally {
    cleanup(value);
  }
});
test("runtime identity observations keep distinct durable identifiers", async () => {
  const value = await fixture();
  try {
    const { core, project: info } = value;
    const supervisor = await addSeatAndActor(
      core,
      info.ownerCredential,
      "Supervisor",
      "runtime-identity",
    );
    core.createRuntimeSession(context(core, info.ownerCredential), {
      sessionId: "runtime-identity-session",
      seatId: supervisor.seatId,
      provider: "herdr",
      profile: "runtime-identity-profile",
      workspace: value.stateDirectory,
    });
    const first = core.recordRuntimeIdentity(
      context(core, info.ownerCredential),
      "runtime-identity-session",
      { processStartId: "process-start-1" },
    );
    const second = core.recordRuntimeIdentity(
      context(core, info.ownerCredential),
      "runtime-identity-session",
      { processStartId: "process-start-1" },
    );
    assert.notEqual(first.observationId, second.observationId);
    core.createWorkItem(context(core, info.ownerCredential), {
      workItemId: "runtime-usage-work",
      title: "Usage binding",
      description: "Usage must follow actor authority",
      requiredRole: "Supervisor",
    });
    core.markReady(context(core, info.ownerCredential), "runtime-usage-work");
    const usageAssignment = core.assignWorkItem(
      context(core, info.ownerCredential),
      "runtime-usage-work",
      supervisor.seatId,
    );
    assert.throws(
      () =>
        core.recordUsage(context(core, supervisor.credential), {
          observationId: "unbound-worker-usage",
          provider: "herdr",
          metric: "tokens",
          availability: "observed",
          detail: {},
        }),
      /worker usage requires a session or assignment binding/,
    );
    assert.throws(
      () =>
        core.recordUsage(context(core, supervisor.credential), {
          observationId: "unassigned-session-usage",
          sessionId: "runtime-identity-session",
          provider: "herdr",
          metric: "tokens",
          availability: "observed",
          detail: {},
        }),
      /worker usage requires a session bound to its assignment/,
    );
    assert.throws(
      () =>
        core.recordUsage(context(core, supervisor.credential), {
          observationId: "mismatched-worker-usage",
          sessionId: "runtime-identity-session",
          assignmentId: usageAssignment.assignmentId,
          provider: "herdr",
          metric: "tokens",
          availability: "observed",
          detail: {},
        }),
      /usage session and assignment do not belong together/,
    );
    const otherSeat = core.createSeat(context(core, info.ownerCredential), {
      seatId: "other-usage-seat",
      name: "Other Supervisor",
      role: "Supervisor",
    });
    const otherSupervisor = core.createActor(
      context(core, info.ownerCredential),
      {
        displayName: "Other Supervisor",
        role: "Supervisor",
        seatId: otherSeat.seatId,
      },
    );
    assert.throws(
      () =>
        core.recordUsage(context(core, otherSupervisor.credential), {
          observationId: "cross-seat-usage",
          sessionId: "runtime-identity-session",
          provider: "herdr",
          metric: "tokens",
          availability: "observed",
          detail: {},
        }),
      /worker usage requires a session bound to its assignment/,
    );
    assert.throws(
      () =>
        core.recordUsage(context(core, otherSupervisor.credential), {
          observationId: "foreign-assignment-usage",
          assignmentId: usageAssignment.assignmentId,
          provider: "herdr",
          metric: "tokens",
          availability: "observed",
          detail: {},
        }),
      /worker usage must belong to the actor's assignment/,
    );
    const db = new Database(
      path.join(value.stateDirectory, "controller.sqlite"),
      { readonly: true },
    );
    try {
      const result = db
        .prepare(
          "SELECT COUNT(*) AS count FROM runtime_identities WHERE project_id = ? AND session_id = ?",
        )
        .get(info.projectId, "runtime-identity-session") as { count: number };
      assert.equal(result.count, 2);
    } finally {
      db.close();
    }
  } finally {
    cleanup(value);
  }
});

test("assignment capsule resolves duplicate-title plan slices by stable ID", async () => {
  const value = await fixture();
  try {
    const { core, project: info } = value;
    const plan = {
      schemaVersion: 1,
      taskId: "duplicate-title-plan",
      objective: "Keep slice scopes unambiguous",
      acceptanceCriteria: ["criterion-one"],
      limits: { maxSlices: 2, maxRunMs: 1_000, maxDispatches: 7 },
      slices: [
        {
          id: "slice-a",
          title: "Shared title",
          description: "First scope",
          role: "Developer",
          dependsOn: [],
          writeScope: ["src/a.mjs"],
          acceptanceCriteria: ["criterion-one"],
        },
        {
          id: "slice-b",
          title: "Shared title",
          description: "Second scope",
          role: "Developer",
          dependsOn: ["slice-a"],
          writeScope: ["src/b.mjs"],
          acceptanceCriteria: ["criterion-one"],
        },
      ],
    };
    core.recordInputRevision(context(core, info.ownerCredential), {
      kind: "plan",
      content: plan,
    });
    const developer = await addSeatAndActor(
      core,
      info.ownerCredential,
      "Developer",
      "duplicate-title",
    );
    const workItemId = `wf-${createHash("sha256")
      .update(`${plan.taskId}:slice-b`)
      .digest("hex")
      .slice(0, 24)}`;
    core.createWorkItem(context(core, info.ownerCredential), {
      workItemId,
      title: "Shared title",
      description: "Implement the second slice",
      requiredRole: "Developer",
    });
    core.markReady(context(core, info.ownerCredential), workItemId);
    const assignment = core.assignWorkItem(
      context(core, info.ownerCredential),
      workItemId,
      developer.seatId,
    );
    const dispatch = core.beginCommandDelivery(
      context(core, info.ownerCredential),
      assignment.commandId,
    );
    const payload = dispatch.payload as { prompt: string };
    const capsule = JSON.parse(payload.prompt) as {
      scope: { writePaths: string[] };
    };
    assert.deepEqual(capsule.scope.writePaths, ["src/b.mjs"]);
  } finally {
    cleanup(value);
  }
});

test("assignment capsule includes accepted non-candidate report evidence", async () => {
  const value = await fixture("/workspace");
  try {
    const { core, project: info } = value;
    const pm = await addSeatAndActor(
      core,
      info.ownerCredential,
      "PM",
      "capsule-report-pm",
    );
    const supervisor = await addSeatAndActor(
      core,
      info.ownerCredential,
      "Supervisor",
      "capsule-report-supervisor",
    );
    const developer = await addSeatAndActor(
      core,
      info.ownerCredential,
      "Developer",
      "capsule-report-dev",
    );
    core.createWorkItem(context(core, info.ownerCredential), {
      workItemId: "capsule-pm-report",
      title: "PM report",
      description: "Return a durable report",
      requiredRole: "PM",
    });
    core.markReady(context(core, info.ownerCredential), "capsule-pm-report");
    const reportAssignment = core.assignWorkItem(
      context(core, info.ownerCredential),
      "capsule-pm-report",
      pm.seatId,
    );
    const reportIdentity = {
      commandId: reportAssignment.commandId,
      assignmentId: reportAssignment.assignmentId,
      attempt: reportAssignment.attempt,
      generation: reportAssignment.generation,
    };
    core.beginCommandDelivery(
      context(core, info.ownerCredential),
      reportIdentity.commandId,
    );
    core.recordBridgeReceipt(receipt(reportIdentity, 1, "accepted", "PM"));
    core.beginCommandStart(
      context(core, info.ownerCredential),
      reportIdentity.commandId,
    );
    core.recordBridgeReceipt(receipt(reportIdentity, 2, "submitted", "PM"));
    core.recordBridgeReceipt(receipt(reportIdentity, 3, "working", "PM"));
    const completedReport = receipt(reportIdentity, 4, "completed", "PM");
    core.recordBridgeReceipt(completedReport);
    core.confirmContainment(
      context(core, info.ownerCredential),
      reportAssignment.assignmentId,
      "containment:capsule-report",
    );
    core.acceptNonCandidateReport(
      context(core, info.ownerCredential),
      "capsule-pm-report",
      reportAssignment.assignmentId,
    );
    core.createWorkItem(context(core, info.ownerCredential), {
      workItemId: "capsule-report-dependent",
      title: "Use the accepted report",
      description: "The worker needs the accepted report contents",
      requiredRole: "Developer",
    });
    core.addDependency(
      context(core, info.ownerCredential),
      "capsule-report-dependent",
      "capsule-pm-report",
    );
    core.markReady(
      context(core, info.ownerCredential),
      "capsule-report-dependent",
    );
    const unstartedAssignment = core.assignWorkItem(
      context(core, info.ownerCredential),
      "capsule-report-dependent",
      developer.seatId,
    );
    core.confirmContainment(
      context(core, info.ownerCredential),
      unstartedAssignment.assignmentId,
      "proof:capsule-unstarted-worker",
    );
    const recovery = core.recordRecovery(context(core, info.ownerCredential), {
      recoveryId: "capsule-report-recovery",
      workItemId: "capsule-report-dependent",
      assignmentId: unstartedAssignment.assignmentId,
      recoveryType: "worker_replacement",
      reason: "Replace the contained pre-start assignment",
    });
    core.markReady(
      context(core, info.ownerCredential),
      "capsule-report-dependent",
    );
    const dependentAssignment = core.assignWorkItem(
      context(core, info.ownerCredential),
      "capsule-report-dependent",
      developer.seatId,
      undefined,
      recovery.recoveryId,
    );
    const dispatch = core.beginCommandDelivery(
      context(core, info.ownerCredential),
      dependentAssignment.commandId,
    );
    const payload = dispatch.payload as { prompt: string };
    const capsule = JSON.parse(payload.prompt) as {
      projectId: string;
      assignment: {
        commandId: string;
        assignmentId: string;
        attempt: number;
        generation: number;
      };
      workItem: {
        workItemId: string;
        title: string;
        description: string;
        requiredRole: Role;
        inputRevision: number;
      };
      seat: { seatId: string; role: Role };
      inputs: Record<string, unknown>;
      acceptedFacts: Array<{
        kind: string;
        revision: number;
        contentHash: string;
      }>;
      policy: unknown;
      scope: { workspace: string; writePaths: string[]; readOnly: boolean };
      openFindings: unknown[];
      recoveryHistory: unknown[];
      resultSchema: { type: string; required: string[] };
      nextLegalActions: string[];
      artifacts: unknown[];
      dependencies: Array<Record<string, unknown>>;
    };
    assert.deepEqual(capsule.assignment, {
      commandId: dependentAssignment.commandId,
      assignmentId: dependentAssignment.assignmentId,
      attempt: dependentAssignment.attempt,
      generation: dependentAssignment.generation,
    });
    assert.equal(capsule.projectId, core.projectId);
    assert.deepEqual(capsule.workItem, {
      workItemId: "capsule-report-dependent",
      title: "Use the accepted report",
      description: "The worker needs the accepted report contents",
      requiredRole: "Developer",
      inputRevision: core.inputRevision,
    });
    assert.deepEqual(capsule.seat, {
      seatId: developer.seatId,
      role: "Developer",
    });
    assert.deepEqual(
      capsule.inputs,
      Object.fromEntries(
        info.initialInputs.map((input) => [input.kind, input.content]),
      ),
    );
    assert.deepEqual(
      capsule.acceptedFacts,
      info.initialInputs
        .map((input) => ({
          kind: input.kind,
          revision: 1,
          contentHash: digestJson(input.content),
        }))
        .sort((left, right) => left.kind.localeCompare(right.kind)),
    );
    assert.deepEqual(capsule.policy, { kind: "policy", revision: 1 });
    assert.deepEqual(capsule.scope, {
      workspace: "/workspace",
      writePaths: [],
      readOnly: false,
    });
    assert.deepEqual(capsule.openFindings, []);
    const [recoveryEntry] = capsule.recoveryHistory as Array<
      Record<string, unknown>
    >;
    assert.deepEqual(
      {
        recovery_id: recoveryEntry?.recovery_id,
        recovery_type: recoveryEntry?.recovery_type,
        generation: recoveryEntry?.generation,
        reason: recoveryEntry?.reason,
        containment_state: recoveryEntry?.containment_state,
        outcome: recoveryEntry?.outcome,
      },
      {
        recovery_id: "capsule-report-recovery",
        recovery_type: "worker_replacement",
        generation: 1,
        reason: "Replace the contained pre-start assignment",
        containment_state: "contained",
        outcome: "pending",
      },
    );
    assert.deepEqual(capsule.resultSchema, {
      type: "object",
      required: [
        "candidateId",
        "commitSha",
        "baseSha",
        "changedScope",
        "limitations",
        "evidence",
      ],
    });
    assert.deepEqual(capsule.nextLegalActions, [
      "implement_assigned_scope",
      "report_blocker",
      "submit_candidate",
    ]);
    assert.deepEqual(capsule.artifacts, []);
    assert.deepEqual(capsule.dependencies, [
      {
        workItemId: "capsule-pm-report",
        candidateId: null,
        inputRevision: core.inputRevision,
        report: completedReport,
        reportHash: digestJson(completedReport),
      },
    ]);
    core.createWorkItem(context(core, info.ownerCredential), {
      workItemId: "capsule-supervisor-finding",
      title: "Report a finding",
      description: "An active finding must reach its Supervisor",
      requiredRole: "Supervisor",
    });
    core.markReady(
      context(core, info.ownerCredential),
      "capsule-supervisor-finding",
    );
    const initialSupervisorAssignment = core.assignWorkItem(
      context(core, info.ownerCredential),
      "capsule-supervisor-finding",
      supervisor.seatId,
    );
    core.createFinding(context(core, supervisor.credential), {
      findingId: "capsule-open-finding",
      workItemId: "capsule-supervisor-finding",
      assignmentId: initialSupervisorAssignment.assignmentId,
      generation: initialSupervisorAssignment.generation,
      fingerprint: "capsule-finding-fingerprint",
      severity: "high",
      evidence: { artifactRef: "artifact://finding/evidence" },
      requestedCorrection: "Correct the identified failure",
      resolutionCondition: "The correction passes the focused scenario",
    });
    core.confirmContainment(
      context(core, info.ownerCredential),
      initialSupervisorAssignment.assignmentId,
      "proof:capsule-finding-report",
    );
    const supervisorRecovery = core.recordRecovery(
      context(core, info.ownerCredential),
      {
        recoveryId: "capsule-finding-recovery",
        workItemId: "capsule-supervisor-finding",
        assignmentId: initialSupervisorAssignment.assignmentId,
        recoveryType: "worker_replacement",
        reason: "Reassign after detecting the open finding",
      },
    );
    core.markReady(
      context(core, info.ownerCredential),
      "capsule-supervisor-finding",
    );
    const supervisorAssignment = core.assignWorkItem(
      context(core, info.ownerCredential),
      "capsule-supervisor-finding",
      supervisor.seatId,
      undefined,
      supervisorRecovery.recoveryId,
    );
    const supervisorDispatch = core.beginCommandDelivery(
      context(core, info.ownerCredential),
      supervisorAssignment.commandId,
    );
    const supervisorCapsule = JSON.parse(
      (supervisorDispatch.payload as { prompt: string }).prompt,
    ) as {
      openFindings: Array<Record<string, unknown>>;
      scope: { readOnly: boolean };
    };
    assert.deepEqual(supervisorCapsule.openFindings, [
      {
        findingId: "capsule-open-finding",
        severity: "high",
        evidence: { artifactRef: "artifact://finding/evidence" },
        requestedCorrection: "Correct the identified failure",
        resolutionCondition: "The correction passes the focused scenario",
        state: "detected",
      },
    ]);
    assert.equal(supervisorCapsule.scope.readOnly, true);
  } finally {
    cleanup(value);
  }
});

test("mutation replay is exact and conflicting idempotency-key reuse has no side effect", async () => {
  const value = await fixture();
  try {
    const { core, project: info } = value;
    const request = context(core, info.ownerCredential, "work-create");
    const input = {
      workItemId: "replay-work",
      title: "Original",
      description: "unchanged",
      requiredRole: "Developer" as const,
    };
    const first = core.createWorkItem(request, input);
    const versionAfterFirst = core.stateVersion;
    assert.deepEqual(core.createWorkItem(request, input), first);
    assert.equal(core.stateVersion, versionAfterFirst);
    assert.throws(
      () =>
        core.createWorkItem(request, { ...input, title: "Conflicting reuse" }),
      IdempotencyConflictError,
    );
    const staleVersionId = crypto.randomUUID();
    assert.throws(
      () =>
        core.createWorkItem(
          {
            ...request,
            requestId: `req-${staleVersionId}`,
            idempotencyKey: `idem-${staleVersionId}`,
          },
          { ...input, workItemId: "stale-version-work" },
        ),
      StateVersionConflictError,
    );
    assert.equal(core.stateVersion, versionAfterFirst);
  } finally {
    cleanup(value);
  }
});

test("readiness, bridge receipt sequence, containment, candidate binding, and acceptance are enforced", async () => {
  const value = await fixture();
  try {
    const { core, project: info } = value;
    const pm = await addSeatAndActor(core, info.ownerCredential, "PM", "pm");
    const developer = await addSeatAndActor(
      core,
      info.ownerCredential,
      "Developer",
      "dev",
    );
    core.recordInputRevision(context(core, info.ownerCredential), {
      kind: "acceptance_criteria",
      content: [" criterion-one "],
    });
    core.createWorkItem(context(core, pm.credential), {
      workItemId: "feature",
      title: "Feature",
      description: "Implement safely",
      requiredRole: "Developer",
    });
    assert.equal(core.readiness("feature").ready, true);
    core.markReady(context(core, info.ownerCredential), "feature");
    const versionBeforeEmptyCandidate = core.stateVersion;
    assert.throws(
      () =>
        core.assignWorkItem(
          context(core, info.ownerCredential),
          "feature",
          developer.seatId,
          "",
        ),
      /only a Verifier assignment may bind a candidate/,
    );
    assert.equal(core.stateVersion, versionBeforeEmptyCandidate);
    const devAssignment = core.assignWorkItem(
      context(core, info.ownerCredential),
      "feature",
      developer.seatId,
    );
    core.createWorkItem(context(core, pm.credential), {
      workItemId: "late-prerequisite-running",
      title: "Late prerequisite",
      description: "A dependency must not change after assignment",
      requiredRole: "Developer",
    });
    const runningVersion = core.stateVersion;
    assert.throws(
      () =>
        core.addDependency(
          context(core, pm.credential),
          "feature",
          "late-prerequisite-running",
        ),
      MutationConflictError,
    );
    assert.equal(core.stateVersion, runningVersion);
    assert.throws(
      () =>
        core.recordInputRevision(context(core, info.ownerCredential), {
          kind: "policy",
          content: { mustNotRaceWithActiveWorker: true },
        }),
      MutationConflictError,
    );
    assert.equal(core.inputRevision, devAssignment.inputRevision);
    const identity = {
      commandId: devAssignment.commandId,
      assignmentId: devAssignment.assignmentId,
      attempt: devAssignment.attempt,
      generation: devAssignment.generation,
    };
    const firstDelivery = core.beginCommandDelivery(
      context(core, info.ownerCredential),
      identity.commandId,
    );
    const retriedDelivery = core.beginCommandDelivery(
      context(core, info.ownerCredential),
      identity.commandId,
    );
    assert.equal(firstDelivery.ordinal, 1);
    assert.equal(retriedDelivery.ordinal, 2);
    assert.deepEqual(retriedDelivery.payload, firstDelivery.payload);
    assert.equal(
      core.recordBridgeReceipt(receipt(identity, 1, "accepted")).duplicate,
      false,
    );
    core.beginCommandStart(
      context(core, info.ownerCredential),
      identity.commandId,
    );
    const submitted = receipt(identity, 2, "submitted");
    assert.equal(core.recordBridgeReceipt(submitted).duplicate, false);
    assert.equal(
      core.commandState(identity.commandId),
      "acknowledged",
      "submitted marks send, not worker completion",
    );
    assert.equal(
      core.recordBridgeReceipt(receipt(identity, 3, "working")).duplicate,
      false,
    );
    assert.equal(
      core.recordBridgeReceipt(receipt(identity, 4, "completed")).duplicate,
      false,
    );
    assert.equal(core.commandState(identity.commandId), "completed");
    assert.equal(core.readiness("feature").ready, false);
    assert.equal(
      core.recordBridgeReceipt(receipt(identity, 4, "completed")).duplicate,
      true,
    );
    assert.throws(
      () => core.recordBridgeReceipt({ ...submitted, sequence: 4 }),
      MutationConflictError,
    );

    const verifier = await addSeatAndActor(
      core,
      info.ownerCredential,
      "Verifier",
      "verify",
    );
    const verifierTask = core.createWorkItem(context(core, pm.credential), {
      workItemId: "verify-feature",
      title: "Verify feature",
      description: "Test submitted candidate",
      requiredRole: "Verifier",
      parentWorkItemId: "feature",
    });
    assert.equal(verifierTask.workItemId, "verify-feature");
    core.confirmContainment(
      context(core, info.ownerCredential),
      devAssignment.assignmentId,
      "supervisor-confirmed:dev",
    );
    for (const candidateId of ["", " \t "]) {
      assert.throws(
        () =>
          core.submitCandidate(context(core, developer.credential), {
            candidateId,
            assignmentId: devAssignment.assignmentId,
            commitSha: "c".repeat(40),
            baseSha: "b".repeat(40),
            changedScope: ["src"],
            limitations: [],
            evidence: ["implemented and verified"],
          }),
        CandidateBindingError,
      );
    }
    for (const invalidEvidence of [
      [],
      [" \t "],
      ["x".repeat(4097)],
      Array.from({ length: 65 }, () => "evidence"),
    ]) {
      assert.throws(
        () =>
          core.submitCandidate(context(core, developer.credential), {
            candidateId: "invalid-evidence",
            assignmentId: devAssignment.assignmentId,
            commitSha: "c".repeat(40),
            baseSha: "b".repeat(40),
            changedScope: ["src"],
            limitations: [],
            evidence: invalidEvidence,
          }),
        CandidateBindingError,
      );
    }
    const candidate = core.submitCandidate(
      context(core, developer.credential),
      {
        candidateId: "candidate-1",
        assignmentId: devAssignment.assignmentId,
        commitSha: `${"A".repeat(20)}${"a".repeat(20)}`,
        baseSha: `${"B".repeat(20)}${"b".repeat(20)}`,
        changedScope: ["src"],
        limitations: [],
        evidence: ["implemented and verified"],
      },
    );
    assert.throws(
      () =>
        core.submitCandidate(context(core, developer.credential), {
          candidateId: "candidate-duplicate-assignment",
          assignmentId: devAssignment.assignmentId,
          commitSha: "c".repeat(40),
          baseSha: "b".repeat(40),
          changedScope: ["src"],
          limitations: [],
          evidence: ["implemented and verified"],
        }),
      CandidateBindingError,
    );
    const candidateDb = new Database(
      path.join(value.stateDirectory, "controller.sqlite"),
    );
    try {
      const stored = candidateDb
        .prepare(
          "SELECT commit_sha, base_sha, evidence_json, report_hash FROM candidates WHERE project_id = ? AND candidate_id = ?",
        )
        .get(info.projectId, candidate.candidateId) as
        | {
            commit_sha: string;
            base_sha: string;
            evidence_json: string;
            report_hash: string;
          }
        | undefined;
      assert.deepEqual(stored, {
        commit_sha: "a".repeat(40),
        base_sha: "b".repeat(40),
        evidence_json: '["implemented and verified"]',
        report_hash: digestJson({
          candidateId: candidate.candidateId,
          assignmentId: devAssignment.assignmentId,
          attempt: devAssignment.attempt,
          generation: devAssignment.generation,
          inputRevision: core.inputRevision,
          commitSha: "a".repeat(40),
          baseSha: "b".repeat(40),
          changedScope: ["src"],
          limitations: [],
          evidence: ["implemented and verified"],
        }),
      });
    } finally {
      candidateDb.close();
    }
    core.markReady(context(core, info.ownerCredential), "verify-feature");
    const verifierAssignment = core.assignWorkItem(
      context(core, info.ownerCredential),
      "verify-feature",
      verifier.seatId,
      candidate.candidateId,
    );
    const verifierIdentity = {
      commandId: verifierAssignment.commandId,
      assignmentId: verifierAssignment.assignmentId,
      attempt: verifierAssignment.attempt,
      generation: verifierAssignment.generation,
    };
    core.beginCommandDelivery(
      context(core, info.ownerCredential),
      verifierIdentity.commandId,
    );
    core.recordBridgeReceipt(
      receipt(verifierIdentity, 1, "accepted", "Verifier"),
    );
    core.beginCommandStart(
      context(core, info.ownerCredential),
      verifierIdentity.commandId,
    );
    core.recordBridgeReceipt(
      receipt(verifierIdentity, 2, "submitted", "Verifier"),
    );
    core.recordBridgeReceipt(
      receipt(verifierIdentity, 3, "working", "Verifier"),
    );
    core.recordBridgeReceipt(
      receipt(verifierIdentity, 4, "completed", "Verifier"),
    );
    core.confirmContainment(
      context(core, info.ownerCredential),
      verifierAssignment.assignmentId,
      "supervisor-confirmed:verifier",
    );
    assert.throws(
      () =>
        core.recordEvidence(
          context(core, verifier.credential),
          verifierAssignment.assignmentId,
          {
            evidenceId: "wrong-candidate-evidence",
            candidateId: "different-candidate",
            criterion: "criterion-one",
            passed: true,
            artifactRef: "artifact://test/wrong-candidate",
            observation: "checked wrong candidate",
            exitStatus: 0,
          },
        ),
      CandidateBindingError,
    );
    const versionBeforeInvalidEvidence = core.stateVersion;
    assert.throws(
      () =>
        core.recordEvidence(
          context(core, verifier.credential),
          verifierAssignment.assignmentId,
          {
            evidenceId: "string-pass-evidence",
            candidateId: candidate.candidateId,
            criterion: " criterion-one ",
            passed: "false" as unknown as boolean,
            artifactRef: "artifact://test/string-pass",
            observation: "checked candidate",
            exitStatus: 0,
          },
        ),
      CandidateBindingError,
    );
    assert.equal(core.stateVersion, versionBeforeInvalidEvidence);
    assert.throws(
      () =>
        core.recordEvidence(
          context(core, verifier.credential),
          verifierAssignment.assignmentId,
          {
            evidenceId: "passing-nonzero-exit",
            candidateId: candidate.candidateId,
            criterion: " criterion-one ",
            passed: true,
            artifactRef: "artifact://test/nonzero-exit",
            observation: "checks reported failure",
            exitStatus: 7,
          },
        ),
      /passing evidence must have exit status 0/,
    );
    assert.equal(core.stateVersion, versionBeforeInvalidEvidence);
    assert.throws(
      () =>
        core.recordEvidence(
          context(core, verifier.credential),
          verifierAssignment.assignmentId,
          {
            evidenceId: "empty-artifact-evidence",
            candidateId: candidate.candidateId,
            criterion: " criterion-one ",
            passed: true,
            artifactRef: " \n ",
            observation: "checked candidate",
            exitStatus: 0,
          },
        ),
      CandidateBindingError,
    );
    assert.equal(core.stateVersion, versionBeforeInvalidEvidence);
    core.createWorkItem(context(core, pm.credential), {
      workItemId: "verify-feature-second",
      title: "Second verifier",
      description: "Every planned Verifier child must bind the candidate",
      requiredRole: "Verifier",
      parentWorkItemId: "feature",
    });
    assert.throws(
      () =>
        core.recordEvidence(
          context(core, verifier.credential),
          verifierAssignment.assignmentId,
          {
            evidenceId: "failed-before-all-verifiers-are-assigned",
            candidateId: candidate.candidateId,
            criterion: " criterion-one ",
            passed: false,
            artifactRef: "artifact://test/failure",
            observation: "check failed",
            exitStatus: 1,
          },
        ),
      /all Verifier children must be assigned to the failing candidate/,
    );
    let artifactReads = 0;
    const evidenceContext = context(core, verifier.credential);
    const recordedEvidence = core.recordEvidence(
      evidenceContext,
      verifierAssignment.assignmentId,
      {
        evidenceId: "evidence-1",
        candidateId: candidate.candidateId,
        criterion: " criterion-one ",
        passed: true,
        get artifactRef() {
          artifactReads += 1;
          return artifactReads === 1
            ? "artifact://test/evidence-1"
            : "artifact://test/divergent";
        },
        observation: "all checks passed",
        exitStatus: 0,
      },
    );
    assert.equal(artifactReads, 1);
    assert.equal(
      recordedEvidence.evidenceHash,
      digestJson({
        evidenceId: "evidence-1",
        candidateId: candidate.candidateId,
        verifierAssignmentId: verifierAssignment.assignmentId,
        inputRevision: evidenceContext.inputRevision,
        criterion: " criterion-one ",
        passed: true,
        artifactRef: "artifact://test/evidence-1",
        observation: "all checks passed",
        exitStatus: 0,
      }),
    );
    const versionBeforeUnassignedVerifier = core.stateVersion;
    assert.throws(
      () =>
        core.acceptCandidate(
          context(core, info.ownerCredential),
          "feature",
          candidate.candidateId,
        ),
      /every open Verifier child must be bound to the current candidate/,
    );
    assert.equal(core.stateVersion, versionBeforeUnassignedVerifier);
    const verifierRecovery = core.recordRecovery(
      context(core, info.ownerCredential),
      {
        recoveryId: "replace-first-verifier",
        workItemId: "verify-feature",
        assignmentId: verifierAssignment.assignmentId,
        recoveryType: "implementation_remediation",
        reason: "Repeat independent verification with a replacement assignment",
      },
    );
    assert.equal(verifierRecovery.outcome, "pending");
    core.markReady(context(core, info.ownerCredential), "verify-feature");
    const replacementVerifier = core.assignWorkItem(
      context(core, info.ownerCredential),
      "verify-feature",
      verifier.seatId,
      candidate.candidateId,
      verifierRecovery.recoveryId,
    );
    const replacementIdentity = {
      commandId: replacementVerifier.commandId,
      assignmentId: replacementVerifier.assignmentId,
      attempt: replacementVerifier.attempt,
      generation: replacementVerifier.generation,
    };
    core.beginCommandDelivery(
      context(core, info.ownerCredential),
      replacementVerifier.commandId,
    );
    core.recordBridgeReceipt(
      receipt(replacementIdentity, 5, "accepted", "Verifier"),
    );
    core.beginCommandStart(
      context(core, info.ownerCredential),
      replacementVerifier.commandId,
    );
    core.recordBridgeReceipt(
      receipt(replacementIdentity, 6, "submitted", "Verifier"),
    );
    core.recordBridgeReceipt(
      receipt(replacementIdentity, 7, "working", "Verifier"),
    );
    core.recordBridgeReceipt(
      receipt(replacementIdentity, 8, "completed", "Verifier"),
    );
    core.confirmContainment(
      context(core, info.ownerCredential),
      replacementVerifier.assignmentId,
      "supervisor-confirmed:replacement-verifier",
    );
    core.markReady(
      context(core, info.ownerCredential),
      "verify-feature-second",
    );
    const secondVerifier = core.assignWorkItem(
      context(core, info.ownerCredential),
      "verify-feature-second",
      verifier.seatId,
      candidate.candidateId,
    );
    const secondIdentity = {
      commandId: secondVerifier.commandId,
      assignmentId: secondVerifier.assignmentId,
      attempt: secondVerifier.attempt,
      generation: secondVerifier.generation,
    };
    core.beginCommandDelivery(
      context(core, info.ownerCredential),
      secondVerifier.commandId,
    );
    core.recordBridgeReceipt(
      receipt(secondIdentity, 9, "accepted", "Verifier"),
    );
    core.beginCommandStart(
      context(core, info.ownerCredential),
      secondVerifier.commandId,
    );
    core.recordBridgeReceipt(
      receipt(secondIdentity, 10, "submitted", "Verifier"),
    );
    core.recordBridgeReceipt(
      receipt(secondIdentity, 11, "working", "Verifier"),
    );
    core.recordBridgeReceipt(
      receipt(secondIdentity, 12, "completed", "Verifier"),
    );
    core.confirmContainment(
      context(core, info.ownerCredential),
      secondVerifier.assignmentId,
      "supervisor-confirmed:second-verifier",
    );
    const secondEvidence = core.recordEvidence(
      context(core, verifier.credential),
      secondVerifier.assignmentId,
      {
        evidenceId: "evidence-2",
        candidateId: candidate.candidateId,
        criterion: " criterion-one ",
        passed: true,
        artifactRef: "artifact://test/evidence-2",
        observation: "checks passed",
        exitStatus: 0,
      },
    );
    const versionBeforeCurrentVerifierEvidence = core.stateVersion;
    assert.throws(
      () =>
        core.acceptCandidate(
          context(core, info.ownerCredential),
          "feature",
          candidate.candidateId,
        ),
      /every bound Verifier assignment must be contained and pass every current criterion/,
    );
    assert.equal(core.stateVersion, versionBeforeCurrentVerifierEvidence);
    const replacementEvidence = core.recordEvidence(
      context(core, verifier.credential),
      replacementVerifier.assignmentId,
      {
        evidenceId: "replacement-evidence",
        candidateId: candidate.candidateId,
        criterion: " criterion-one ",
        passed: true,
        artifactRef: "artifact://test/replacement-evidence",
        observation: "all checks passed",
        exitStatus: 0,
      },
    );
    assert.equal(
      core.acceptCandidate(
        context(core, info.ownerCredential),
        "feature",
        candidate.candidateId,
      ).acceptedCandidateId,
      candidate.candidateId,
    );
    const versionBeforeLateVerifier = core.stateVersion;
    assert.throws(
      () =>
        core.createWorkItem(context(core, pm.credential), {
          workItemId: "late-verifier",
          title: "Late Verifier",
          description: "Cannot verify an already accepted parent",
          requiredRole: "Verifier",
          parentWorkItemId: "feature",
        }),
      /Verifier work cannot be added to a terminal Developer parent/,
    );
    assert.equal(core.stateVersion, versionBeforeLateVerifier);
    core.createWorkItem(context(core, pm.credential), {
      workItemId: "late-prerequisite-accepted",
      title: "Late accepted prerequisite",
      description: "Acceptance must not gain a new unmet dependency",
      requiredRole: "Developer",
    });
    const acceptedVersion = core.stateVersion;
    assert.throws(
      () =>
        core.addDependency(
          context(core, pm.credential),
          "feature",
          "late-prerequisite-accepted",
        ),
      MutationConflictError,
    );
    assert.equal(core.stateVersion, acceptedVersion);
    core.createWorkItem(context(core, pm.credential), {
      workItemId: "uses-verified-work",
      title: "Use verified prerequisite",
      description: "Verifier completion retains the accepted candidate",
      requiredRole: "Developer",
    });
    core.addDependency(
      context(core, pm.credential),
      "uses-verified-work",
      "verify-feature",
    );
    assert.equal(core.readiness("uses-verified-work").ready, true);
    assert.equal(core.readiness("feature").ready, false);
    core.createWorkItem(context(core, pm.credential), {
      workItemId: "candidate-pinned-downstream",
      title: "Candidate-pinned downstream",
      description: "Use the exact candidate selected by the planner",
      requiredRole: "Developer",
    });
    const versionBeforeEmptyPin = core.stateVersion;
    assert.throws(
      () =>
        core.addDependency(
          context(core, pm.credential),
          "candidate-pinned-downstream",
          "feature",
          "",
        ),
      /required candidate ID must not be empty/,
    );
    assert.equal(core.stateVersion, versionBeforeEmptyPin);
    core.addDependency(
      context(core, pm.credential),
      "candidate-pinned-downstream",
      "feature",
      candidate.candidateId,
    );
    assert.equal(core.readiness("candidate-pinned-downstream").ready, true);
    core.markReady(
      context(core, info.ownerCredential),
      "candidate-pinned-downstream",
    );
    const candidateDownstream = core.assignWorkItem(
      context(core, info.ownerCredential),
      "candidate-pinned-downstream",
      developer.seatId,
    );
    const candidateDispatch = core.beginCommandDelivery(
      context(core, info.ownerCredential),
      candidateDownstream.commandId,
    );
    const candidateCapsule = JSON.parse(
      (candidateDispatch.payload as { prompt: string }).prompt,
    ) as { artifacts: Array<Record<string, unknown>> };
    assert.deepEqual(candidateCapsule.artifacts, [
      {
        workItemId: "feature",
        candidateId: candidate.candidateId,
        evidenceId: "evidence-2",
        verifierAssignmentId: secondVerifier.assignmentId,
        inputRevision: core.inputRevision,
        criterion: " criterion-one ",
        passed: true,
        artifactRef: "artifact://test/evidence-2",
        observation: "checks passed",
        exitStatus: 0,
        evidenceHash: secondEvidence.evidenceHash,
      },
      {
        workItemId: "feature",
        candidateId: candidate.candidateId,
        evidenceId: "replacement-evidence",
        verifierAssignmentId: replacementVerifier.assignmentId,
        inputRevision: core.inputRevision,
        criterion: " criterion-one ",
        passed: true,
        artifactRef: "artifact://test/replacement-evidence",
        observation: "all checks passed",
        exitStatus: 0,
        evidenceHash: replacementEvidence.evidenceHash,
      },
    ]);
  } finally {
    cleanup(value);
  }
});

test("stale inputs, unauthorized controller actions, and candidate evidence are rejected", async () => {
  const value = await fixture();
  try {
    const { core, project: info } = value;
    const pm = await addSeatAndActor(core, info.ownerCredential, "PM", "pm");
    await addSeatAndActor(core, info.ownerCredential, "Developer", "ready-dev");
    core.createWorkItem(context(core, pm.credential), {
      workItemId: "prerequisite",
      title: "Prerequisite",
      description: "Must be accepted first",
      requiredRole: "Developer",
    });
    core.createWorkItem(context(core, pm.credential), {
      workItemId: "dependent",
      title: "Dependent",
      description: "Waits for prerequisite",
      requiredRole: "Developer",
    });
    core.addDependency(
      context(core, pm.credential),
      "dependent",
      "prerequisite",
    );
    assert.equal(core.readiness("dependent").ready, false);
    assert.match(
      core.readiness("dependent").reasons.join(";"),
      /dependency prerequisite is not accepted/,
    );
    const versionBeforeOrphan = core.stateVersion;
    assert.throws(
      () =>
        core.createWorkItem(context(core, pm.credential), {
          workItemId: "orphan-verifier",
          title: "Orphan verifier",
          description: "Cannot bind to a candidate",
          requiredRole: "Verifier",
        }),
      /Verifier work requires an existing Developer parent/,
    );
    assert.equal(core.stateVersion, versionBeforeOrphan);
    core.createWorkItem(context(core, pm.credential), {
      workItemId: "needs-verifier",
      title: "Needs verifier seat",
      description: "Cannot start without a verifier",
      requiredRole: "Verifier",
      parentWorkItemId: "prerequisite",
    });
    assert.match(
      core.readiness("needs-verifier").reasons.join(";"),
      /no active Verifier seat/,
    );
    core.createWorkItem(context(core, pm.credential), {
      workItemId: "stale-work",
      title: "Stale",
      description: "Needs new binding",
      requiredRole: "Developer",
    });
    const revision = core.inputRevision;
    const beforeRevision = context(core, info.ownerCredential, "old-input");
    core.recordInputRevision(context(core, info.ownerCredential), {
      kind: "policy",
      content: { revision: 2 },
    });
    const staleInputId = crypto.randomUUID();
    assert.throws(
      () =>
        core.createWorkItem(
          {
            ...beforeRevision,
            requestId: `req-${staleInputId}`,
            idempotencyKey: `idem-${staleInputId}`,
            expectedVersion: core.stateVersion,
          },
          {
            workItemId: "stale-input-work",
            title: "Stale input",
            description: "Must not be created against an old revision",
            requiredRole: "Developer",
          },
        ),
      InputRevisionConflictError,
    );
    assert.throws(
      () => core.markReady(context(core, info.ownerCredential), "stale-work"),
      ReadinessError,
    );
    assert.equal(
      core.rebindWorkItem(context(core, info.ownerCredential), "stale-work")
        .inputRevision,
      revision + 1,
    );

    const actor = await addSeatAndActor(
      core,
      info.ownerCredential,
      "Developer",
      "unauthorized",
    );
    assert.throws(
      () => core.markReady(context(core, actor.credential), "stale-work"),
      AuthorizationError,
    );
    assert.throws(
      () =>
        core.recordEvidence(
          context(core, actor.credential),
          "missing-assignment",
          {
            evidenceId: "no-evidence",
            candidateId: "missing-candidate",
            criterion: "criterion-one",
            passed: true,
            artifactRef: "artifact://none",
            observation: "no verifier report",
            exitStatus: 0,
          },
        ),
      AuthorizationError,
    );
    assert.throws(
      () =>
        core.acceptCandidate(
          context(core, info.ownerCredential),
          "stale-work",
          "missing-candidate",
        ),
      CandidateBindingError,
    );
  } finally {
    cleanup(value);
  }
});

test("worker replacement limits count consumed recovery records", async () => {
  const value = await fixture();
  try {
    const { core, project: info } = value;
    const developer = await addSeatAndActor(
      core,
      info.ownerCredential,
      "Developer",
      "dev",
    );
    core.createWorkItem(context(core, info.ownerCredential), {
      workItemId: "recover-work",
      title: "Recover",
      description: "One replacement only",
      requiredRole: "Developer",
    });
    core.markReady(context(core, info.ownerCredential), "recover-work");
    const original = core.assignWorkItem(
      context(core, info.ownerCredential),
      "recover-work",
      developer.seatId,
    );
    core.beginCommandDelivery(
      context(core, info.ownerCredential),
      original.commandId,
    );
    const originalIdentity = {
      commandId: original.commandId,
      assignmentId: original.assignmentId,
      attempt: original.attempt,
      generation: original.generation,
    };
    core.recordBridgeReceipt(receipt(originalIdentity, 1, "accepted"));
    core.beginCommandStart(
      context(core, info.ownerCredential),
      original.commandId,
    );
    core.recordBridgeReceipt(receipt(originalIdentity, 2, "aborted"));
    core.confirmContainment(
      context(core, info.ownerCredential),
      original.assignmentId,
      "operator-proof:original",
    );
    const first = core.recordRecovery(context(core, info.ownerCredential), {
      recoveryId: "recovery-1",
      workItemId: "recover-work",
      assignmentId: original.assignmentId,
      recoveryType: "worker_replacement",
      reason: "Replace contained worker",
    });
    assert.equal(first.outcome, "pending");
    core.markReady(context(core, info.ownerCredential), "recover-work");
    const replacement = core.assignWorkItem(
      context(core, info.ownerCredential),
      "recover-work",
      developer.seatId,
      undefined,
      first.recoveryId,
    );
    core.beginCommandDelivery(
      context(core, info.ownerCredential),
      replacement.commandId,
    );
    const replacementIdentity = {
      commandId: replacement.commandId,
      assignmentId: replacement.assignmentId,
      attempt: replacement.attempt,
      generation: replacement.generation,
    };
    core.recordBridgeReceipt(receipt(replacementIdentity, 3, "accepted"));
    core.beginCommandStart(
      context(core, info.ownerCredential),
      replacement.commandId,
    );
    core.recordBridgeReceipt(receipt(replacementIdentity, 4, "aborted"));
    core.confirmContainment(
      context(core, info.ownerCredential),
      replacement.assignmentId,
      "operator-proof:replacement",
    );
    core.markReady(context(core, info.ownerCredential), "recover-work");
    assert.throws(
      () =>
        core.assignWorkItem(
          context(core, info.ownerCredential),
          "recover-work",
          developer.seatId,
          undefined,
          first.recoveryId,
        ),
      MutationConflictError,
    );
    const recoveryDb = new Database(
      path.join(value.stateDirectory, "controller.sqlite"),
    );
    try {
      const consumed = recoveryDb
        .prepare(
          "SELECT outcome FROM recovery_attempts WHERE project_id = ? AND recovery_id = ?",
        )
        .get(info.projectId, first.recoveryId) as
        { outcome: string } | undefined;
      assert.equal(consumed?.outcome, "replacement_created");
    } finally {
      recoveryDb.close();
    }
    const second = core.recordRecovery(context(core, info.ownerCredential), {
      recoveryId: "recovery-2",
      workItemId: "recover-work",
      assignmentId: replacement.assignmentId,
      recoveryType: "worker_replacement",
      reason: "Limit must prevent another replacement",
    });
    assert.equal(second.outcome, "blocked");
    assert.equal(second.limit, 1);
  } finally {
    cleanup(value);
  }
});

test("scheduler dispatches Slice B only after Slice A candidate acceptance", async () => {
  const value = await fixture();
  try {
    const { core, project: info } = value;
    const plan = validateWorkflowPlan({
      schemaVersion: 1,
      taskId: "serial-slices",
      objective: "Require independent acceptance between serial slices",
      acceptanceCriteria: ["criterion-one"],
      limits: { maxSlices: 2, maxRunMs: 60_000, maxDispatches: 7 },
      slices: [
        {
          id: "first",
          title: "Slice A",
          description: "Implement the first accepted change",
          role: "Developer",
          dependsOn: [],
          writeScope: ["src/first.ts"],
          acceptanceCriteria: ["criterion-one"],
        },
        {
          id: "second",
          title: "Slice B",
          description: "Build on the accepted first change",
          role: "Developer",
          dependsOn: ["first"],
          writeScope: ["src/second.ts"],
          acceptanceCriteria: ["criterion-one"],
        },
      ],
    });
    const mutationContexts = new Map<string, MutationContext>();
    const dispatches: string[] = [];
    let planAccepted = false;
    let stopping = false;
    let cancelDuringPlanCheck = false;
    const scheduler = new WorkflowScheduler({
      plan,
      core,
      operatorCredential: info.ownerCredential,
      startedAtMs: Date.now(),
      mutationContexts: {
        getOrCreate(key, create) {
          const prior = mutationContexts.get(key);
          if (prior) return prior;
          const next = create();
          mutationContexts.set(key, next);
          return next;
        },
      },
      seats: {
        PM: { seatId: "scheduler-pm", name: "PM", displayName: "PM" },
        Developer: {
          seatId: "scheduler-developer",
          name: "Developer",
          displayName: "Developer",
        },
        Verifier: {
          seatId: "scheduler-verifier",
          name: "Verifier",
          displayName: "Verifier",
        },
        Supervisor: {
          seatId: "scheduler-supervisor",
          name: "Supervisor",
          displayName: "Supervisor",
        },
      },
      isPlanAccepted: async () => {
        if (cancelDuringPlanCheck) {
          cancelDuringPlanCheck = false;
          stopping = true;
        }
        return planAccepted;
      },
      isStopping: () => stopping,
      dispatch: async ({ slice, getMutationContext }) => {
        dispatches.push(slice.id);
        if (slice.id === "first") {
          core.createWorkItem(context(core, info.ownerCredential), {
            workItemId: "dispatch-version-race",
            title: "Concurrent runtime event",
            description:
              "Advance project state while runtime provisioning finishes",
            requiredRole: "Developer",
          });
          const deliveryContext = getMutationContext();
          assert.equal(deliveryContext.expectedVersion, core.stateVersion);
          assert.deepEqual(getMutationContext(), deliveryContext);
        }
      },
    });
    const identities = scheduler.initialize();
    assert.deepEqual(await scheduler.step(), {
      state: "waiting_for_plan_acceptance",
    });

    planAccepted = true;
    const first = await scheduler.step();
    assert.equal(first.state, "dispatched");
    if (first.state !== "dispatched") return;
    assert.equal(first.sliceId, "first");
    assert.deepEqual(dispatches, ["first"]);
    const blockedSecond = scheduler.status.work.find(
      (work) => work.title === "Slice B",
    );
    assert.ok(blockedSecond);
    assert.equal(blockedSecond?.state, "pending");
    assert.equal(core.readiness(blockedSecond.workItemId).ready, false);
    assert.ok(
      core
        .readiness(blockedSecond.workItemId)
        .reasons.includes(
          `dependency ${first.assignment.workItemId} is not accepted`,
        ),
    );

    const whileFirstUnaccepted = await scheduler.step();
    assert.equal(whileFirstUnaccepted.state, "waiting");
    if (whileFirstUnaccepted.state !== "waiting") return;
    assert.equal(whileFirstUnaccepted.workItemId, first.assignment.workItemId);
    assert.deepEqual(dispatches, ["first"]);

    const firstIdentity = {
      commandId: first.assignment.commandId,
      assignmentId: first.assignment.assignmentId,
      attempt: first.assignment.attempt,
      generation: first.assignment.generation,
    };
    core.beginCommandDelivery(
      context(core, info.ownerCredential),
      firstIdentity.commandId,
    );
    core.recordBridgeReceipt(receipt(firstIdentity, 1, "accepted"));
    core.beginCommandStart(
      context(core, info.ownerCredential),
      firstIdentity.commandId,
    );
    core.recordBridgeReceipt(receipt(firstIdentity, 2, "submitted"));
    core.recordBridgeReceipt(receipt(firstIdentity, 3, "working"));
    core.recordBridgeReceipt(receipt(firstIdentity, 4, "completed"));
    core.confirmContainment(
      context(core, info.ownerCredential),
      firstIdentity.assignmentId,
      "scheduler-test:developer-contained",
    );
    const candidate = core.submitCandidate(
      context(core, identities.Developer.credential),
      {
        candidateId: "serial-first-candidate",
        assignmentId: first.assignment.assignmentId,
        commitSha: "a".repeat(40),
        baseSha: "b".repeat(40),
        changedScope: ["src/first.ts"],
        limitations: [],
        evidence: ["implemented and verified"],
      },
    );
    core.createWorkItem(context(core, info.ownerCredential), {
      workItemId: "serial-first-verification",
      title: "Verify Slice A",
      description: "Independently verify the first candidate",
      requiredRole: "Verifier",
      parentWorkItemId: first.assignment.workItemId,
    });
    core.markReady(
      context(core, info.ownerCredential),
      "serial-first-verification",
    );
    const verifier = core.assignWorkItem(
      context(core, info.ownerCredential),
      "serial-first-verification",
      identities.Verifier.seatId,
      candidate.candidateId,
    );
    const verifierIdentity = {
      commandId: verifier.commandId,
      assignmentId: verifier.assignmentId,
      attempt: verifier.attempt,
      generation: verifier.generation,
    };
    core.beginCommandDelivery(
      context(core, info.ownerCredential),
      verifierIdentity.commandId,
    );
    core.recordBridgeReceipt(
      receipt(verifierIdentity, 1, "accepted", "Verifier"),
    );
    core.beginCommandStart(
      context(core, info.ownerCredential),
      verifierIdentity.commandId,
    );
    core.recordBridgeReceipt(
      receipt(verifierIdentity, 2, "submitted", "Verifier"),
    );
    core.recordBridgeReceipt(
      receipt(verifierIdentity, 3, "working", "Verifier"),
    );
    core.recordBridgeReceipt(
      receipt(verifierIdentity, 4, "completed", "Verifier"),
    );
    core.confirmContainment(
      context(core, info.ownerCredential),
      verifier.assignmentId,
      "scheduler-test:verifier-contained",
    );
    core.recordEvidence(
      context(core, identities.Verifier.credential),
      verifier.assignmentId,
      {
        evidenceId: "serial-first-evidence",
        candidateId: candidate.candidateId,
        criterion: "criterion-one",
        passed: true,
        artifactRef: "artifact://serial-first/evidence",
        observation: "checks passed",
        exitStatus: 0,
      },
    );
    core.acceptCandidate(
      context(core, info.ownerCredential),
      first.assignment.workItemId,
      candidate.candidateId,
    );

    cancelDuringPlanCheck = true;
    assert.deepEqual(await scheduler.step(), {
      state: "stopped",
      reason: "run canceled by signal",
    });
    assert.equal(
      scheduler.status.work.find((work) => work.title === "Slice B")?.state,
      "pending",
    );
    assert.deepEqual(dispatches, ["first"]);
    stopping = false;

    const second = await scheduler.step();
    assert.equal(second.state, "dispatched");
    if (second.state !== "dispatched") return;
    assert.equal(second.sliceId, "second");
    assert.deepEqual(dispatches, ["first", "second"]);
    const secondIdentity = {
      commandId: second.assignment.commandId,
      assignmentId: second.assignment.assignmentId,
      attempt: second.assignment.attempt,
      generation: second.assignment.generation,
    };
    core.beginCommandDelivery(
      context(core, info.ownerCredential),
      secondIdentity.commandId,
    );
    core.recordBridgeReceipt(receipt(secondIdentity, 5, "accepted"));
    core.beginCommandStart(
      context(core, info.ownerCredential),
      secondIdentity.commandId,
    );
    core.recordBridgeReceipt(receipt(secondIdentity, 6, "submitted"));
    core.recordBridgeReceipt(receipt(secondIdentity, 7, "working"));
    core.recordBridgeReceipt(receipt(secondIdentity, 8, "completed"));
    core.confirmContainment(
      context(core, info.ownerCredential),
      secondIdentity.assignmentId,
      "scheduler-test:second-developer-contained",
    );
    const secondCandidate = core.submitCandidate(
      context(core, identities.Developer.credential),
      {
        candidateId: "serial-second-candidate",
        assignmentId: second.assignment.assignmentId,
        commitSha: "c".repeat(40),
        baseSha: "a".repeat(40),
        changedScope: ["src/second.ts"],
        limitations: [],
        evidence: ["implemented and verified"],
      },
    );
    core.createWorkItem(context(core, info.ownerCredential), {
      workItemId: "serial-second-verification",
      title: "Verify Slice B",
      description: "Independently verify the second candidate",
      requiredRole: "Verifier",
      parentWorkItemId: second.assignment.workItemId,
    });
    core.markReady(
      context(core, info.ownerCredential),
      "serial-second-verification",
    );
    const secondVerifier = core.assignWorkItem(
      context(core, info.ownerCredential),
      "serial-second-verification",
      identities.Verifier.seatId,
      secondCandidate.candidateId,
    );
    const secondVerifierIdentity = {
      commandId: secondVerifier.commandId,
      assignmentId: secondVerifier.assignmentId,
      attempt: secondVerifier.attempt,
      generation: secondVerifier.generation,
    };
    core.beginCommandDelivery(
      context(core, info.ownerCredential),
      secondVerifierIdentity.commandId,
    );
    core.recordBridgeReceipt(
      receipt(secondVerifierIdentity, 5, "accepted", "Verifier"),
    );
    core.beginCommandStart(
      context(core, info.ownerCredential),
      secondVerifierIdentity.commandId,
    );
    core.recordBridgeReceipt(
      receipt(secondVerifierIdentity, 6, "submitted", "Verifier"),
    );
    core.recordBridgeReceipt(
      receipt(secondVerifierIdentity, 7, "working", "Verifier"),
    );
    core.recordBridgeReceipt(
      receipt(secondVerifierIdentity, 8, "completed", "Verifier"),
    );
    core.confirmContainment(
      context(core, info.ownerCredential),
      secondVerifier.assignmentId,
      "scheduler-test:second-verifier-contained",
    );
    core.recordEvidence(
      context(core, identities.Verifier.credential),
      secondVerifier.assignmentId,
      {
        evidenceId: "serial-second-evidence",
        candidateId: secondCandidate.candidateId,
        criterion: "criterion-one",
        passed: true,
        artifactRef: "artifact://serial-second/evidence",
        observation: "checks passed",
        exitStatus: 0,
      },
    );
    core.acceptCandidate(
      context(core, info.ownerCredential),
      second.assignment.workItemId,
      secondCandidate.candidateId,
    );
    assert.deepEqual(await scheduler.step(), { state: "complete" });
  } finally {
    cleanup(value);
  }
});
test("final Verifier accepts only complete passing evidence for the composed commit", async () => {
  const value = await fixture();
  try {
    const { core, project: info } = value;
    core.recordInputRevision(context(core, info.ownerCredential), {
      kind: "acceptance_criteria",
      content: ["criterion-one", "criterion-two"],
    });
    const pm = await addSeatAndActor(
      core,
      info.ownerCredential,
      "PM",
      "final-verification-dependency",
    );
    const verifier = await addSeatAndActor(
      core,
      info.ownerCredential,
      "Verifier",
      "final-verification",
    );
    const supervisor = await addSeatAndActor(
      core,
      info.ownerCredential,
      "Supervisor",
      "final-verification-review",
    );
    core.createWorkItem(context(core, info.ownerCredential), {
      workItemId: "accepted-slice-report",
      title: "Accepted slice report",
      description: "Dependency that must be accepted before final verification",
      requiredRole: "PM",
    });
    core.markReady(
      context(core, info.ownerCredential),
      "accepted-slice-report",
    );
    const sliceReport = core.assignWorkItem(
      context(core, info.ownerCredential),
      "accepted-slice-report",
      pm.seatId,
    );
    const sliceIdentity = {
      commandId: sliceReport.commandId,
      assignmentId: sliceReport.assignmentId,
      attempt: sliceReport.attempt,
      generation: sliceReport.generation,
    };
    core.beginCommandDelivery(
      context(core, info.ownerCredential),
      sliceReport.commandId,
    );
    core.recordBridgeReceipt(receipt(sliceIdentity, 1, "accepted", "PM"));
    core.beginCommandStart(
      context(core, info.ownerCredential),
      sliceReport.commandId,
    );
    core.recordBridgeReceipt(receipt(sliceIdentity, 2, "submitted", "PM"));
    core.recordBridgeReceipt(receipt(sliceIdentity, 3, "working", "PM"));
    core.recordBridgeReceipt(receipt(sliceIdentity, 4, "completed", "PM"));
    core.confirmContainment(
      context(core, info.ownerCredential),
      sliceReport.assignmentId,
      "containment:final-verification-dependency",
    );
    core.acceptNonCandidateReport(
      context(core, info.ownerCredential),
      "accepted-slice-report",
      sliceReport.assignmentId,
    );

    assert.throws(
      () =>
        core.createWorkItem(context(core, info.ownerCredential), {
          workItemId: "incomplete-final-verifier",
          title: "Incomplete final verification",
          description: "A subset cannot stand in for parent criteria",
          requiredRole: "Verifier",
          finalVerification: true,
          acceptanceCriteria: ["criterion-one"],
        }),
      /must match every parent acceptance criterion/,
    );
    core.createWorkItem(context(core, info.ownerCredential), {
      workItemId: "final-verifier-work",
      title: "Verify composed checkout",
      description: "Inspect the exact composed final checkout",
      requiredRole: "Verifier",
      finalVerification: true,
      acceptanceCriteria: ["criterion-one", "criterion-two"],
    });
    core.addDependency(
      context(core, info.ownerCredential),
      "final-verifier-work",
      "accepted-slice-report",
    );
    core.createWorkItem(context(core, info.ownerCredential), {
      workItemId: "final-verification-review",
      title: "Review final verification",
      description: "Supervisor consumes the final evidence",
      requiredRole: "Supervisor",
    });
    core.addDependency(
      context(core, info.ownerCredential),
      "final-verification-review",
      "final-verifier-work",
    );
    core.markReady(context(core, info.ownerCredential), "final-verifier-work");
    const assignment = core.assignWorkItem(
      context(core, info.ownerCredential),
      "final-verifier-work",
      verifier.seatId,
    );
    const identity = {
      commandId: assignment.commandId,
      assignmentId: assignment.assignmentId,
      attempt: assignment.attempt,
      generation: assignment.generation,
    };
    core.beginCommandDelivery(
      context(core, info.ownerCredential),
      assignment.commandId,
    );
    core.recordBridgeReceipt(receipt(identity, 1, "accepted", "Verifier"));
    core.beginCommandStart(
      context(core, info.ownerCredential),
      assignment.commandId,
    );
    core.recordBridgeReceipt(receipt(identity, 2, "submitted", "Verifier"));
    core.recordBridgeReceipt(receipt(identity, 3, "working", "Verifier"));
    const commitSha = "a".repeat(40);
    const evidence = [
      {
        evidenceId: "final-evidence-one",
        criterion: "criterion-one",
        passed: true,
        artifactRef: "/tmp/final-evidence/criterion-one",
        observation: "criteria verified",
        exitStatus: 0,
      },
      {
        evidenceId: "final-evidence-two",
        criterion: "criterion-two",
        passed: true,
        artifactRef: "/tmp/final-evidence/criterion-two",
        observation: "all browser checks passed",
        exitStatus: 0,
      },
    ] as const;
    core.recordBridgeReceipt({
      ...receipt(identity, 4, "completed", "Verifier"),
      reply: JSON.stringify({
        commitSha,
        evidence: evidence.map(
          ({ criterion, passed, observation, exitStatus }) => ({
            criterion,
            passed,
            observation,
            exitStatus,
            artifactRef:
              criterion === "criterion-two"
                ? "/evidence/./criterion-two"
                : `/evidence/${criterion}`,
          }),
        ),
      }),
    });
    core.confirmContainment(
      context(core, info.ownerCredential),
      assignment.assignmentId,
      "containment:final-verifier",
    );

    const incompleteContext = context(core, info.ownerCredential);
    const versionBeforeIncomplete = core.stateVersion;
    assert.throws(
      () =>
        core.acceptFinalVerification(incompleteContext, {
          workItemId: "final-verifier-work",
          assignmentId: assignment.assignmentId,
          commitSha,
          evidence: evidence.slice(0, 1),
        }),
      /exactly one passing artifact for every criterion/,
    );
    assert.equal(core.stateVersion, versionBeforeIncomplete);
    assert.equal(
      core
        .statusSnapshot()
        .work.find((work) => work.workItemId === "final-verifier-work")?.state,
      "awaiting_verification",
    );
    assert.throws(
      () =>
        core.acceptFinalVerification(context(core, info.ownerCredential), {
          workItemId: "final-verifier-work",
          assignmentId: assignment.assignmentId,
          commitSha,
          evidence: evidence.map((entry, index) =>
            index === 1 ? { ...entry, passed: false } : entry,
          ),
        }),
      /failed or omitted criterion/,
    );

    assert.throws(
      () =>
        core.acceptFinalVerification(context(core, info.ownerCredential), {
          workItemId: "final-verifier-work",
          assignmentId: assignment.assignmentId,
          commitSha: "b".repeat(40),
          evidence,
        }),
      /does not match the reported composed commit/,
    );
    assert.throws(
      () =>
        core.acceptFinalVerification(context(core, info.ownerCredential), {
          workItemId: "final-verifier-work",
          assignmentId: assignment.assignmentId,
          commitSha,
          evidence: evidence.map((entry, index) =>
            index === 1
              ? { ...entry, artifactRef: "/tmp/final-evidence/unreported" }
              : entry,
          ),
        }),
      /artifact does not match its reported evidence path/,
    );
    assert.deepEqual(
      core.acceptFinalVerification(context(core, info.ownerCredential), {
        workItemId: "final-verifier-work",
        assignmentId: assignment.assignmentId,
        commitSha,
        evidence,
      }),
      {
        acceptedWorkItemId: "final-verifier-work",
        commitSha,
      },
    );
    const status = core.statusSnapshot();
    assert.equal(
      status.work.find((work) => work.workItemId === "final-verifier-work")
        ?.state,
      "accepted",
    );
    assert.deepEqual(status.finalVerification, [
      {
        workItemId: "final-verifier-work",
        assignmentId: assignment.assignmentId,
        commitSha,
        evidence: [
          {
            evidenceId: "final-evidence-one",
            criterion: "criterion-one",
            passed: true,
            artifactRef: "/tmp/final-evidence/criterion-one",
            observation: "criteria verified",
            exitStatus: 0,
            evidenceHash: digestJson({
              workItemId: "final-verifier-work",
              assignmentId: assignment.assignmentId,
              inputRevision: core.inputRevision,
              commitSha,
              evidenceId: "final-evidence-one",
              criterion: "criterion-one",
              passed: true,
              artifactRef: "/tmp/final-evidence/criterion-one",
              observation: "criteria verified",
              exitStatus: 0,
            }),
          },
          {
            evidenceId: "final-evidence-two",
            criterion: "criterion-two",
            passed: true,
            artifactRef: "/tmp/final-evidence/criterion-two",
            observation: "all browser checks passed",
            exitStatus: 0,
            evidenceHash: digestJson({
              workItemId: "final-verifier-work",
              assignmentId: assignment.assignmentId,
              inputRevision: core.inputRevision,
              commitSha,
              evidenceId: "final-evidence-two",
              criterion: "criterion-two",
              passed: true,
              artifactRef: "/tmp/final-evidence/criterion-two",
              observation: "all browser checks passed",
              exitStatus: 0,
            }),
          },
        ],
      },
    ]);
    assert.throws(
      () =>
        core.acceptFinalVerification(context(core, info.ownerCredential), {
          workItemId: "final-verifier-work",
          assignmentId: assignment.assignmentId,
          commitSha,
          evidence,
        }),
      /not contained, current, and eligible for acceptance/,
    );
    core.markReady(
      context(core, info.ownerCredential),
      "final-verification-review",
    );
    const reviewAssignment = core.assignWorkItem(
      context(core, info.ownerCredential),
      "final-verification-review",
      supervisor.seatId,
    );
    const database = new Database(
      path.join(value.stateDirectory, "controller.sqlite"),
    );
    try {
      const row = database
        .prepare(
          "SELECT payload_json FROM commands WHERE project_id = ? AND command_id = ?",
        )
        .get(info.projectId, reviewAssignment.commandId) as
        { payload_json: string } | undefined;
      assert.ok(row);
      const dispatch = JSON.parse(row.payload_json) as { prompt: string };
      const capsule = JSON.parse(dispatch.prompt) as {
        dependencies: Array<{
          workItemId: string;
          finalVerification?: {
            commitSha: string;
            evidence: unknown[];
          };
        }>;
      };
      assert.deepEqual(
        capsule.dependencies.find(
          (dependency) => dependency.workItemId === "final-verifier-work",
        )?.finalVerification,
        {
          commitSha,
          evidence: status.finalVerification[0]?.evidence,
        },
      );
    } finally {
      database.close();
    }
  } finally {
    cleanup(value);
  }
});

test("evidence batches preserve all failed criteria and require fresh replacement evidence", async () => {
  const value = await fixture();
  try {
    const { core, project: info } = value;
    const developer = await addSeatAndActor(
      core,
      info.ownerCredential,
      "Developer",
      "batch-dev",
    );
    const verifier = await addSeatAndActor(
      core,
      info.ownerCredential,
      "Verifier",
      "batch-verifier",
    );
    core.recordInputRevision(context(core, info.ownerCredential), {
      kind: "acceptance_criteria",
      content: ["criterion-one", "criterion-two"],
    });
    core.createWorkItem(context(core, info.ownerCredential), {
      workItemId: "batch-feature",
      title: "Batch feature",
      description: "Persist complete independent observations",
      requiredRole: "Developer",
    });

    const complete = (
      assignment: AssignmentResult,
      role: "Developer" | "Verifier",
      firstSequence: number,
      proof: string,
    ) => {
      const identity = {
        commandId: assignment.commandId,
        assignmentId: assignment.assignmentId,
        attempt: assignment.attempt,
        generation: assignment.generation,
      };
      core.beginCommandDelivery(
        context(core, info.ownerCredential),
        assignment.commandId,
      );
      core.recordBridgeReceipt(
        receipt(identity, firstSequence, "accepted", role),
      );
      core.beginCommandStart(
        context(core, info.ownerCredential),
        assignment.commandId,
      );
      core.recordBridgeReceipt(
        receipt(identity, firstSequence + 1, "submitted", role),
      );
      core.recordBridgeReceipt(
        receipt(identity, firstSequence + 2, "working", role),
      );
      core.recordBridgeReceipt(
        receipt(identity, firstSequence + 3, "completed", role),
      );
      core.confirmContainment(
        context(core, info.ownerCredential),
        assignment.assignmentId,
        proof,
      );
    };

    core.markReady(context(core, info.ownerCredential), "batch-feature");
    const originalDeveloper = core.assignWorkItem(
      context(core, info.ownerCredential),
      "batch-feature",
      developer.seatId,
    );
    complete(originalDeveloper, "Developer", 1, "contained:batch-dev-one");
    const oldCandidate = core.submitCandidate(
      context(core, developer.credential),
      {
        candidateId: "batch-candidate-old",
        assignmentId: originalDeveloper.assignmentId,
        commitSha: "a".repeat(40),
        baseSha: "b".repeat(40),
        changedScope: ["src"],
        limitations: [],
        evidence: ["implemented and verified"],
      },
    );
    core.createWorkItem(context(core, info.ownerCredential), {
      workItemId: "batch-verify-old",
      title: "Verify old candidate",
      description: "Bind observations to the old candidate",
      requiredRole: "Verifier",
      parentWorkItemId: "batch-feature",
    });
    core.markReady(context(core, info.ownerCredential), "batch-verify-old");
    const oldVerifier = core.assignWorkItem(
      context(core, info.ownerCredential),
      "batch-verify-old",
      verifier.seatId,
      oldCandidate.candidateId,
    );
    complete(oldVerifier, "Verifier", 1, "contained:batch-verifier-old");
    const failedBatch = core.recordEvidenceBatch(
      context(core, verifier.credential),
      oldVerifier.assignmentId,
      [
        {
          evidenceId: "batch-failed-one",
          candidateId: oldCandidate.candidateId,
          criterion: "criterion-one",
          passed: false,
          artifactRef: "artifact://batch/failure-one",
          observation: "check failed",
          exitStatus: 1,
        },
        {
          evidenceId: "batch-failed-two",
          candidateId: oldCandidate.candidateId,
          criterion: "criterion-two",
          passed: false,
          artifactRef: "artifact://batch/failure-two",
          observation: "check failed",
          exitStatus: 1,
        },
      ],
    );
    assert.equal(failedBatch.evidence.length, 2);
    const evidenceDb = new Database(
      path.join(value.stateDirectory, "controller.sqlite"),
    );
    try {
      const rows = evidenceDb
        .prepare(
          "SELECT evidence_id, passed, observation, exit_status FROM candidate_evidence WHERE project_id = ? AND candidate_id = ? ORDER BY evidence_id",
        )
        .all(info.projectId, oldCandidate.candidateId) as Array<{
        evidence_id: string;
        passed: number;
        observation: string;
        exit_status: number;
      }>;
      assert.deepEqual(rows, [
        {
          evidence_id: "batch-failed-one",
          passed: 0,
          observation: "check failed",
          exit_status: 1,
        },
        {
          evidence_id: "batch-failed-two",
          passed: 0,
          observation: "check failed",
          exit_status: 1,
        },
      ]);
    } finally {
      evidenceDb.close();
    }
    assert.throws(
      () =>
        core.recordEvidence(
          context(core, verifier.credential),
          oldVerifier.assignmentId,
          {
            evidenceId: "late-old-candidate-evidence",
            candidateId: oldCandidate.candidateId,
            criterion: "criterion-one",
            passed: true,
            artifactRef: "artifact://batch/late",
            observation: "stale verification",
            exitStatus: 0,
          },
        ),
      CandidateBindingError,
    );
    const recovery = core.recordRecovery(context(core, info.ownerCredential), {
      recoveryId: "batch-implementation-remediation",
      workItemId: "batch-feature",
      assignmentId: originalDeveloper.assignmentId,
      recoveryType: "implementation_remediation",
      reason: "Replace the rejected candidate with a fresh implementation",
    });
    assert.equal(recovery.outcome, "pending");
    core.markReady(context(core, info.ownerCredential), "batch-feature");
    const replacementDeveloper = core.assignWorkItem(
      context(core, info.ownerCredential),
      "batch-feature",
      developer.seatId,
      undefined,
      recovery.recoveryId,
    );
    assert.equal(
      core.latestCompletedReport(
        "batch-feature",
        replacementDeveloper.assignmentId,
      ),
      undefined,
    );
    complete(replacementDeveloper, "Developer", 5, "contained:batch-dev-two");
    assert.equal(
      core.latestCompletedReport(
        "batch-feature",
        replacementDeveloper.assignmentId,
      )?.assignmentId,
      replacementDeveloper.assignmentId,
    );
    const freshCandidate = core.submitCandidate(
      context(core, developer.credential),
      {
        candidateId: "batch-candidate-fresh",
        assignmentId: replacementDeveloper.assignmentId,
        commitSha: "c".repeat(40),
        baseSha: "b".repeat(40),
        changedScope: ["src"],
        limitations: [],
        evidence: ["implemented and verified"],
      },
    );
    core.createWorkItem(context(core, info.ownerCredential), {
      workItemId: "batch-verify-fresh",
      title: "Verify fresh candidate",
      description: "Current evidence authorizes only the fresh candidate",
      requiredRole: "Verifier",
      parentWorkItemId: "batch-feature",
    });
    core.markReady(context(core, info.ownerCredential), "batch-verify-fresh");
    const freshVerifier = core.assignWorkItem(
      context(core, info.ownerCredential),
      "batch-verify-fresh",
      verifier.seatId,
      freshCandidate.candidateId,
    );
    complete(freshVerifier, "Verifier", 5, "contained:batch-verifier-fresh");
    core.recordEvidenceBatch(
      context(core, verifier.credential),
      freshVerifier.assignmentId,
      [
        {
          evidenceId: "batch-passed-one",
          candidateId: freshCandidate.candidateId,
          criterion: "criterion-one",
          passed: true,
          artifactRef: "artifact://batch/passed-one",
          observation: "check passed",
          exitStatus: 0,
        },
        {
          evidenceId: "batch-passed-two",
          candidateId: freshCandidate.candidateId,
          criterion: "criterion-two",
          passed: true,
          artifactRef: "artifact://batch/passed-two",
          observation: "check passed",
          exitStatus: 0,
        },
      ],
    );
    assert.throws(
      () =>
        core.acceptCandidate(
          context(core, info.ownerCredential),
          "batch-feature",
          oldCandidate.candidateId,
        ),
      CandidateBindingError,
    );
    assert.equal(
      core.acceptCandidate(
        context(core, info.ownerCredential),
        "batch-feature",
        freshCandidate.candidateId,
      ).acceptedCandidateId,
      freshCandidate.candidateId,
    );
  } finally {
    cleanup(value);
  }
});
