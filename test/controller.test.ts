import assert from "node:assert/strict";
import { mkdtempSync, rmSync, statSync } from "node:fs";
import net from "node:net";
import { tmpdir } from "node:os";
import path from "node:path";
import { spawnSync } from "node:child_process";
import { test } from "node:test";
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
import { M1BridgeAdapter } from "../src/controller/m1-bridge.js";
import {
  M1_MAX_PROMPT_BYTES,
  M1ResponseFrameParser,
} from "../src/controller/m1-protocol.js";
import type {
  BridgeReceipt,
  InitialProject,
  MutationContext,
  Role,
} from "../src/controller/types.js";
import { digestJson } from "../src/controller/canonical.js";
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

async function fixture(): Promise<Fixture> {
  const stateDirectory = mkdtempSync(
    path.join(tmpdir(), "capstan-controller-test-"),
  );
  const info = project();
  const core = await ControllerCore.open({ stateDirectory, project: info });
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
  const credential = `${suffix}-credential-${crypto.randomUUID()}`;
  core.createSeat(context(core, ownerCredential), {
    seatId,
    name: `${role} ${suffix}`,
    role,
  });
  core.createActor(context(core, ownerCredential), {
    displayName: `${role} ${suffix}`,
    role,
    credential,
    seatId,
  });
  return { seatId, credential };
}

test("credential hashing rejects ill-formed UTF-16", () => {
  const prefix = "c".repeat(31);
  assert.throws(() => credentialHash(`${prefix}\ud800`), AuthenticationError);
  assert.throws(() => credentialHash(`${prefix}\udc00`), AuthenticationError);
  assert.equal(credentialHash(`${prefix}\ud83d\ude00`).length, 64);
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
test("M1 adapter rejects duplicate responses after acknowledgement", async () => {
  const value = await fixture();
  const bridgeSocket = path.join(value.stateDirectory, "bridge.sock");
  const receiptSocket = path.join(value.stateDirectory, "receipt.sock");
  const server = net.createServer((socket) => {
    socket.once("data", (chunk) => {
      const request = JSON.parse(chunk.toString("utf8"));
      assert.equal(request.singleResponse, true);
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
    adapter = new M1BridgeAdapter(core, receiptSocket, bridgeSocket);
    await adapter.listen();
    await assert.rejects(
      adapter.dispatchAndStart(
        context(core, info.ownerCredential),
        assignment.commandId,
      ),
      /unexpected frame after bridge response/,
    );
  } finally {
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
    await addSeatAndActor(
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
      core.recordBridgeReceipt(receipt(identity, 2, "submitted", role));
      core.recordBridgeReceipt(receipt(identity, 3, "working", role));
      const versionBeforeEmptyReport = core.stateVersion;
      assert.throws(
        () =>
          core.recordBridgeReceipt({
            ...receipt(identity, 4, "completed", role),
            reply: "   ",
          }),
        /non-empty reply text/,
      );
      assert.equal(core.commandState(identity.commandId), "started");
      assert.equal(core.stateVersion, versionBeforeEmptyReport);
      core.recordBridgeReceipt(receipt(identity, 4, "completed", role));
      assert.equal(core.commandState(identity.commandId), "completed");
      core.confirmContainment(
        context(core, info.ownerCredential),
        assignment.assignmentId,
        `containment:${suffix}`,
      );
      assert.deepEqual(
        core.acceptNonCandidateReport(
          context(core, info.ownerCredential),
          workItemId,
          assignment.assignmentId,
        ),
        { acceptedWorkItemId: workItemId },
      );
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
    core.recordInputRevision(context(core, info.ownerCredential), {
      kind: "policy",
      content: { reportContextChanged: true },
    });
    assert.equal(
      core.rebindWorkItem(context(core, info.ownerCredential), "uses-pm-report")
        .inputRevision,
      core.inputRevision,
    );
    assert.equal(core.readiness("uses-pm-report").ready, false);
    assert.match(
      core.readiness("uses-pm-report").reasons.join(";"),
      /dependency report-pm is bound to a stale input revision/,
    );
    assert.deepEqual(
      core.removeDependency(
        context(core, info.ownerCredential),
        "uses-pm-report",
        "report-pm",
      ),
      { removed: true },
    );
    assert.equal(core.readiness("uses-pm-report").ready, true);
  } finally {
    cleanup(value);
  }
});

test("assignment capsule includes accepted non-candidate report evidence", async () => {
  const value = await fixture();
  try {
    const { core, project: info } = value;
    const pm = await addSeatAndActor(
      core,
      info.ownerCredential,
      "PM",
      "capsule-report-pm",
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
    const dependentAssignment = core.assignWorkItem(
      context(core, info.ownerCredential),
      "capsule-report-dependent",
      developer.seatId,
    );
    const dispatch = core.beginCommandDelivery(
      context(core, info.ownerCredential),
      dependentAssignment.commandId,
    );
    const payload = dispatch.payload as {
      prompt: string;
      commandId: string;
      assignmentId: string;
      attempt: number;
      generation: number;
      singleResponse: boolean;
    };
    assert.equal(payload.singleResponse, true);
    assert.deepEqual(
      {
        commandId: payload.commandId,
        assignmentId: payload.assignmentId,
        attempt: payload.attempt,
        generation: payload.generation,
      },
      {
        commandId: dependentAssignment.commandId,
        assignmentId: dependentAssignment.assignmentId,
        attempt: dependentAssignment.attempt,
        generation: dependentAssignment.generation,
      },
    );
    const capsule = JSON.parse(payload.prompt) as {
      assignment: {
        commandId: string;
        assignmentId: string;
        attempt: number;
        generation: number;
      };
      dependencies: Array<Record<string, unknown>>;
    };
    assert.deepEqual(capsule.assignment, {
      commandId: dependentAssignment.commandId,
      assignmentId: dependentAssignment.assignmentId,
      attempt: dependentAssignment.attempt,
      generation: dependentAssignment.generation,
    });
    assert.deepEqual(capsule.dependencies, [
      {
        workItemId: "capsule-pm-report",
        candidateId: null,
        inputRevision: core.inputRevision,
        report: completedReport,
        reportHash: digestJson(completedReport),
      },
    ]);
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
    core.createWorkItem(context(core, pm.credential), {
      workItemId: "feature",
      title: "Feature",
      description: "Implement safely",
      requiredRole: "Developer",
    });
    assert.equal(core.readiness("feature").ready, true);
    core.markReady(context(core, info.ownerCredential), "feature");
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
    const candidate = core.submitCandidate(
      context(core, developer.credential),
      {
        candidateId: "candidate-1",
        assignmentId: devAssignment.assignmentId,
        commitSha: "a".repeat(40),
        baseSha: "b".repeat(40),
        changedScope: ["src"],
        limitations: [],
      },
    );
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
          },
        ),
      CandidateBindingError,
    );
    core.recordEvidence(
      context(core, verifier.credential),
      verifierAssignment.assignmentId,
      {
        evidenceId: "evidence-1",
        candidateId: candidate.candidateId,
        criterion: "criterion-one",
        passed: true,
        artifactRef: "artifact://test/evidence-1",
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
    core.createWorkItem(context(core, pm.credential), {
      workItemId: "needs-verifier",
      title: "Needs verifier seat",
      description: "Cannot start without a verifier",
      requiredRole: "Verifier",
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
    core.recordBridgeReceipt(receipt(originalIdentity, 1, "aborted"));
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
    core.confirmContainment(
      context(core, info.ownerCredential),
      replacement.assignmentId,
      "operator-proof:replacement",
    );
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
