import assert from "node:assert/strict";
import { randomUUID } from "node:crypto";
import { spawn, spawnSync } from "node:child_process";
import {
  chmodSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  statSync,
  writeFileSync,
} from "node:fs";
import os from "node:os";
import path from "node:path";
import { test } from "node:test";
import { listenControl } from "../src/control.js";
import { ControllerCore } from "../src/controller/core.js";
const cli = path.resolve("dist/src/cli.js");

function invokeWithEnv(cwd: string, env: NodeJS.ProcessEnv, ...args: string[]) {
  return spawnSync(process.execPath, [cli, ...args], {
    cwd,
    encoding: "utf8",
    env: { ...process.env, ...env },
  });
}

function invoke(cwd: string, ...args: string[]) {
  return invokeWithEnv(
    cwd,
    {
      M1_PROVIDER_HOST: "",
      M1_HERDR_BINARY: "",
      M1_OMP_BINARY: "",
      M1_OMP_NATIVE_ADDON: "",
      M1_NODE_BINARY: "",
    },
    ...args,
  );
}

function invokeAsync(
  cwd: string,
  ...args: string[]
): Promise<{ status: number | null; stdout: string; stderr: string }> {
  return new Promise((resolve, reject) => {
    const child = spawn(process.execPath, [cli, ...args], {
      cwd,
      env: {
        ...process.env,
        M1_PROVIDER_HOST: "",
        M1_HERDR_BINARY: "",
        M1_OMP_BINARY: "",
        M1_OMP_NATIVE_ADDON: "",
        M1_NODE_BINARY: "",
      },
    });
    let stdout = "";
    let stderr = "";
    child.stdout.setEncoding("utf8").on("data", (chunk: string) => {
      stdout += chunk;
    });
    child.stderr.setEncoding("utf8").on("data", (chunk: string) => {
      stderr += chunk;
    });
    child.once("error", reject);
    child.once("close", (status) => resolve({ status, stdout, stderr }));
  });
}

test("cstan init creates private project-local config and status exposes four seats as JSON", () => {
  const cwd = mkdtempSync(path.join(os.tmpdir(), "cstan-cli-"));
  try {
    const init = invoke(cwd, "init");
    assert.equal(init.status, 0, init.stderr);
    const config = JSON.parse(
      readFileSync(path.join(cwd, ".capstan/project.json"), "utf8"),
    ) as { stateDirectory: string };
    assert.equal(config.stateDirectory, path.join(cwd, ".capstan/state"));
    assert.equal(
      readFileSync(path.join(cwd, ".capstan/operator.key"), "utf8").trim()
        .length >= 32,
      true,
    );
    const status = invoke(cwd, "status", "--json");
    assert.equal(status.status, 0, status.stderr);
    const snapshot = JSON.parse(status.stdout) as {
      schemaVersion: number;
      run: { state: string };
      roles: { role: string }[];
      nextLegalActions: string[];
    };
    assert.equal(snapshot.schemaVersion, 1);
    assert.equal(snapshot.run.state, "not_started");
    assert.deepEqual(
      snapshot.roles.map((role) => role.role),
      ["PM", "Developer", "Verifier", "Supervisor"],
    );
    assert.deepEqual(snapshot.nextLegalActions, ["cstan run --brief <file>"]);
  } finally {
    rmSync(cwd, { recursive: true, force: true });
  }
});
test("cstan init refuses to change an existing project-local directory", () => {
  const cwd = mkdtempSync(path.join(os.tmpdir(), "cstan-existing-dir-"));
  const directory = path.join(cwd, ".capstan");
  try {
    mkdirSync(directory, { mode: 0o755 });
    chmodSync(directory, 0o755);
    const marker = path.join(directory, "operator-data");
    writeFileSync(marker, "keep", { mode: 0o600 });
    const beforeMode = statSync(directory).mode & 0o777;
    const init = invoke(cwd, "init");
    assert.equal(init.status, 3, init.stderr);
    assert.equal(readFileSync(marker, "utf8"), "keep");
    assert.equal(statSync(directory).mode & 0o777, beforeMode);
  } finally {
    rmSync(cwd, { recursive: true, force: true });
  }
});

test("cstan init rejects a project name the CLI cannot load", () => {
  const parent = mkdtempSync(path.join(os.tmpdir(), "cstan-name-"));
  const cwd = path.join(parent, " ");
  try {
    mkdirSync(cwd);
    const init = invoke(cwd, "init");
    assert.equal(init.status, 3, init.stderr);
    assert.equal(existsSync(path.join(cwd, ".capstan")), false);
  } finally {
    rmSync(parent, { recursive: true, force: true });
  }
});
test("cstan status reads the authenticated live control socket through the executable", async () => {
  const cwd = mkdtempSync(path.join(os.tmpdir(), "cstan-live-status-"));
  let core: ControllerCore | undefined;
  let closeControl: (() => Promise<void>) | undefined;
  try {
    assert.equal(invoke(cwd, "init").status, 0);
    const config = JSON.parse(
      readFileSync(path.join(cwd, ".capstan/project.json"), "utf8"),
    ) as {
      projectId: string;
      name: string;
      stateDirectory: string;
    };
    const credential = readFileSync(
      path.join(cwd, ".capstan/operator.key"),
      "utf8",
    ).trim();
    core = await ControllerCore.open({
      stateDirectory: config.stateDirectory,
      project: {
        projectId: config.projectId,
        name: config.name,
        ownerCredential: credential,
        initialInputs: [
          {
            kind: "project_config",
            content: {
              schemaVersion: 1,
              projectId: config.projectId,
              name: config.name,
              baseSha: "a".repeat(40),
            },
          },
          {
            kind: "task_brief",
            content: {
              taskId: "status-test",
              objective: "Read a live controller snapshot",
            },
          },
          {
            kind: "acceptance_criteria",
            content: ["The controller is active"],
          },
          {
            kind: "policy",
            content: { maxSlices: 2, maxRunMs: 1_000, maxDispatches: 2 },
          },
          { kind: "plan", content: { schemaVersion: 1 } },
        ],
      },
    });
    const mutate = (actorCredential: string) => {
      const requestId = randomUUID();
      return {
        credential: actorCredential,
        requestId,
        idempotencyKey: requestId,
        expectedVersion: core!.stateVersion,
        inputRevision: core!.inputRevision,
      };
    };
    const pm = core.createSeat(mutate(credential), {
      seatId: "status-pm-seat",
      name: "Status PM",
      role: "PM",
    });
    core.createActor(mutate(credential), {
      displayName: "Status PM",
      role: "PM",
      seatId: pm.seatId,
    });
    const developer = core.createSeat(mutate(credential), {
      seatId: "status-developer-seat",
      name: "Status Developer",
      role: "Developer",
    });
    const developerActor = core.createActor(mutate(credential), {
      displayName: "Status Developer",
      role: "Developer",
      seatId: developer.seatId,
    });
    const verifier = core.createSeat(mutate(credential), {
      seatId: "status-verifier-seat",
      name: "Status Verifier",
      role: "Verifier",
    });
    const verifierActor = core.createActor(mutate(credential), {
      displayName: "Status Verifier",
      role: "Verifier",
      seatId: verifier.seatId,
    });
    core.createWorkItem(mutate(credential), {
      workItemId: "status-prerequisite",
      title: "Status prerequisite",
      description: "Keep this prerequisite open for blocker reporting",
      requiredRole: "PM",
    });
    core.markReady(mutate(credential), "status-prerequisite");
    const prerequisite = core.assignWorkItem(
      mutate(credential),
      "status-prerequisite",
      pm.seatId,
    );
    core.createWorkItem(mutate(credential), {
      workItemId: "status-dependent",
      title: "Status dependent",
      description: "Expose its prerequisite blocker",
      requiredRole: "Developer",
    });
    core.addDependency(
      mutate(credential),
      "status-dependent",
      "status-prerequisite",
    );
    core.createWorkItem(mutate(credential), {
      workItemId: "status-pending-ready",
      title: "Status pending ready",
      description: "Expose the next legal readiness transition",
      requiredRole: "Developer",
    });
    core.createWorkItem(mutate(credential), {
      workItemId: "status-large-inspect",
      title: "Status large inspect",
      description: "x".repeat(20_000),
      requiredRole: "Developer",
    });

    core.createWorkItem(mutate(credential), {
      workItemId: "status-candidate-work",
      title: "Status candidate",
      description: "Expose durable candidate evidence",
      requiredRole: "Developer",
    });
    core.markReady(mutate(credential), "status-candidate-work");
    const candidateAssignment = core.assignWorkItem(
      mutate(credential),
      "status-candidate-work",
      developer.seatId,
    );
    const complete = (
      assignment: typeof candidateAssignment,
      role: "Developer" | "Verifier",
    ) => {
      const identity = {
        commandId: assignment.commandId,
        assignmentId: assignment.assignmentId,
        attempt: assignment.attempt,
        generation: assignment.generation,
      };
      core!.beginCommandDelivery(mutate(credential), identity.commandId);
      core!.recordBridgeReceipt({
        ...identity,
        sequence: 1,
        type: "accepted",
        role,
        timestamp: "2026-01-01T00:00:01.000Z",
      });
      core!.beginCommandStart(mutate(credential), identity.commandId);
      core!.recordBridgeReceipt({
        ...identity,
        sequence: 2,
        type: "submitted",
        role,
        timestamp: "2026-01-01T00:00:02.000Z",
      });
      core!.recordBridgeReceipt({
        ...identity,
        sequence: 3,
        type: "working",
        role,
        timestamp: "2026-01-01T00:00:03.000Z",
      });
      core!.recordBridgeReceipt({
        ...identity,
        sequence: 4,
        type: "completed",
        role,
        timestamp: "2026-01-01T00:00:04.000Z",
        reply: "completed",
        evidenceRef: { journal: "/tmp/status-journal", sequence: 4 },
      });
      core!.confirmContainment(
        mutate(credential),
        assignment.assignmentId,
        `status-contained:${assignment.assignmentId}`,
      );
    };
    complete(candidateAssignment, "Developer");
    const candidate = core.submitCandidate(mutate(developerActor.credential), {
      candidateId: "status-candidate",
      assignmentId: candidateAssignment.assignmentId,
      commitSha: "c".repeat(40),
      baseSha: "a".repeat(40),
      changedScope: ["src/status.ts"],
      limitations: [],
    });
    core.createWorkItem(mutate(credential), {
      workItemId: "status-verifier-work",
      title: "Status verifier",
      description: "Verify the status candidate",
      requiredRole: "Verifier",
      parentWorkItemId: "status-candidate-work",
    });
    core.markReady(mutate(credential), "status-verifier-work");
    const verifierAssignment = core.assignWorkItem(
      mutate(credential),
      "status-verifier-work",
      verifier.seatId,
      candidate.candidateId,
    );
    complete(verifierAssignment, "Verifier");
    core.recordEvidence(
      mutate(verifierActor.credential),
      verifierAssignment.assignmentId,
      {
        evidenceId: "status-candidate-evidence",
        candidateId: candidate.candidateId,
        criterion: "The controller is active",
        passed: true,
        artifactRef: "artifact://status/candidate-evidence",
      },
    );
    core.acceptCandidate(
      mutate(credential),
      "status-candidate-work",
      candidate.candidateId,
    );
    closeControl = await listenControl(
      path.join(config.stateDirectory, "control.sock"),
      credential,
      core,
    );
    const status = await invokeAsync(cwd, "status", "--json");
    assert.equal(status.status, 0, status.stderr);
    const result = JSON.parse(status.stdout) as {
      schemaVersion: number;
      run: { state: string };
      roles: {
        role: string;
        seatId: string;
        actorActive: boolean;
        assignmentId: string | null;
        sessionState: string | null;
      }[];
      work: {
        workItemId: string;
        state: string;
        nextLegalActions: string[];
      }[];
      ownership: {
        workItemId: string;
        role: string;
        owner: string | null;
      }[];
      blockers: {
        workItemId: string;
        state: string;
        blockers: string[];
      }[];
      evidence: {
        candidateId: string;
        commitSha: string;
        reportHash: string;
        evidenceRef: string | null;
      }[];
      limits: { maxSlices: number; maxRunMs: number; maxDispatches: number };
      nextLegalActions: string[];
    };
    assert.equal(result.schemaVersion, 1);
    assert.equal(result.run.state, "active");
    assert.deepEqual(result.limits, {
      maxSlices: 4,
      maxRunMs: 3_600_000,
      maxDispatches: 16,
    });
    const pmStatus = result.roles.find((role) => role.role === "PM");
    assert.equal(pmStatus?.seatId, pm.seatId);
    assert.equal(pmStatus?.actorActive, true);
    assert.equal(pmStatus?.assignmentId, prerequisite.assignmentId);
    assert.equal(pmStatus?.sessionState, null);
    const ownership = result.ownership.find(
      (item) => item.workItemId === "status-prerequisite",
    );
    assert.deepEqual(ownership, {
      workItemId: "status-prerequisite",
      role: "PM",
      owner: "Status PM",
    });
    const dependentBlocker = result.blockers.find(
      (item) => item.workItemId === "status-dependent",
    );
    assert.deepEqual(dependentBlocker, {
      workItemId: "status-dependent",
      state: "pending",
      blockers: ["status-prerequisite"],
    });
    const markReady = result.work.find(
      (item) => item.workItemId === "status-pending-ready",
    );
    assert.deepEqual(markReady, {
      workItemId: "status-pending-ready",
      title: "Status pending ready",
      role: "Developer",
      state: "pending",
      owner: null,
      blockers: [],
      nextLegalActions: ["mark_ready"],
    });
    assert.equal(result.evidence.length, 1);
    const evidence = result.evidence[0];
    assert.ok(evidence);
    assert.equal(evidence.candidateId, candidate.candidateId);
    assert.equal(evidence.commitSha, "c".repeat(40));
    assert.match(evidence.reportHash, /^[a-f0-9]{64}$/);
    assert.equal(evidence.evidenceRef, "artifact://status/candidate-evidence");
    assert.deepEqual(result.nextLegalActions, ["wait", "mark_ready"]);
    const largeInspection = await invokeAsync(
      cwd,
      "inspect",
      "status-large-inspect",
      "--json",
    );
    assert.equal(largeInspection.status, 0, largeInspection.stderr);
    const largeRecord = JSON.parse(largeInspection.stdout) as {
      record: { description: string };
    };
    assert.equal(largeRecord.record.description.length, 20_000);
    await closeControl();
    await closeControl();
    assert.equal(
      existsSync(path.join(config.stateDirectory, "control.sock")),
      false,
    );
  } finally {
    await closeControl?.();
    core?.close();
    rmSync(cwd, { recursive: true, force: true });
  }
});

test("cstan inspect JSON has the versioned work-item record schema", async () => {
  const cwd = mkdtempSync(path.join(os.tmpdir(), "cstan-inspect-json-"));
  let core: ControllerCore | undefined;
  try {
    assert.equal(invoke(cwd, "init").status, 0);
    const config = JSON.parse(
      readFileSync(path.join(cwd, ".capstan/project.json"), "utf8"),
    ) as {
      projectId: string;
      name: string;
      stateDirectory: string;
    };
    const credential = readFileSync(
      path.join(cwd, ".capstan/operator.key"),
      "utf8",
    ).trim();
    core = await ControllerCore.open({
      stateDirectory: config.stateDirectory,
      project: {
        projectId: config.projectId,
        name: config.name,
        ownerCredential: credential,
        initialInputs: [
          {
            kind: "project_config",
            content: {
              schemaVersion: 1,
              projectId: config.projectId,
              name: config.name,
              baseSha: "a".repeat(40),
            },
          },
          {
            kind: "task_brief",
            content: {
              taskId: "inspect-test",
              objective: "Inspect a durable work item",
            },
          },
          {
            kind: "acceptance_criteria",
            content: ["The inspected item is present"],
          },
          {
            kind: "policy",
            content: { maxSlices: 2, maxRunMs: 1_000, maxDispatches: 2 },
          },
          { kind: "plan", content: { schemaVersion: 1 } },
        ],
      },
    });
    const requestId = randomUUID();
    core.createWorkItem(
      {
        credential,
        requestId,
        idempotencyKey: requestId,
        expectedVersion: core.stateVersion,
        inputRevision: core.inputRevision,
      },
      {
        workItemId: "inspectable",
        title: "Inspectable item",
        description: "Inspect this persisted task",
        requiredRole: "PM",
      },
    );
    core.close();
    core = undefined;
    const inspect = invoke(cwd, "inspect", "inspectable", "--json");
    assert.equal(inspect.status, 0, inspect.stderr);
    const result = JSON.parse(inspect.stdout) as {
      schemaVersion: number;
      kind: string;
      id: string;
      record: { work_item_id: string };
    };
    assert.equal(result.schemaVersion, 1);
    assert.equal(result.kind, "work_item");
    assert.equal(result.id, "inspectable");
    assert.equal(result.record.work_item_id, "inspectable");
  } finally {
    core?.close();
    rmSync(cwd, { recursive: true, force: true });
  }
});

test("cstan rejects malformed briefs with its invalid-input exit code before creating controller state", () => {
  const cwd = mkdtempSync(path.join(os.tmpdir(), "cstan-invalid-"));
  try {
    assert.equal(invoke(cwd, "init").status, 0);
    const brief = path.join(cwd, "brief.json");
    writeFileSync(brief, JSON.stringify({ schemaVersion: 1, taskId: "x" }));
    const run = invoke(cwd, "run", "--brief", brief);
    assert.equal(run.status, 3, run.stderr);
    assert.match(
      run.stderr,
      /acceptanceCriteria|limits|slices|objective|unknown or missing fields/,
    );
    writeFileSync(
      brief,
      Buffer.from([0x7b, 0x22, 0x78, 0x22, 0x3a, 0x22, 0xff, 0x22, 0x7d]),
    );
    const malformedUtf8 = invoke(cwd, "run", "--brief", brief);
    assert.equal(malformedUtf8.status, 3, malformedUtf8.stderr);
    const missing = invoke(
      cwd,
      "run",
      "--brief",
      path.join(cwd, "missing.json"),
    );
    assert.equal(missing.status, 3, missing.stderr);
    const overLimit = path.join(cwd, "over-limit.json");
    writeFileSync(
      overLimit,
      `\uFEFF${JSON.stringify({
        schemaVersion: 1,
        taskId: "over-limit",
        objective: "Reject limits beyond project configuration",
        acceptanceCriteria: ["first criterion", "second criterion"],
        limits: { maxSlices: 2, maxRunMs: 3_600_001, maxDispatches: 7 },
        slices: [
          {
            id: "first",
            title: "First",
            description: "First bounded slice",
            role: "Developer",
            dependsOn: [],
            writeScope: ["src"],
            acceptanceCriteria: ["first criterion"],
          },
          {
            id: "second",
            title: "Second",
            description: "Second bounded slice",
            role: "Developer",
            dependsOn: ["first"],
            writeScope: ["src"],
            acceptanceCriteria: ["second criterion"],
          },
        ],
      })}`,
    );
    const bounded = invoke(cwd, "run", "--brief", overLimit);
    assert.equal(bounded.status, 3, bounded.stderr);
    assert.match(bounded.stderr, /project-local configuration/);
    const validBomBrief = path.join(cwd, "valid-bom.json");
    writeFileSync(
      validBomBrief,
      `\uFEFF${JSON.stringify({
        schemaVersion: 1,
        taskId: "bom-brief",
        objective: "Parse a UTF-8 BOM before runtime preflight",
        acceptanceCriteria: ["The plan is parsed"],
        limits: { maxSlices: 2, maxRunMs: 60_000, maxDispatches: 7 },
        slices: [
          {
            id: "first",
            title: "First",
            description: "First bounded slice",
            role: "Developer",
            dependsOn: [],
            writeScope: ["src"],
            acceptanceCriteria: ["The plan is parsed"],
          },
          {
            id: "second",
            title: "Second",
            description: "Second bounded slice",
            role: "Developer",
            dependsOn: ["first"],
            writeScope: ["src"],
            acceptanceCriteria: ["The plan is parsed"],
          },
        ],
      })}`,
    );
    const parsedBom = invoke(cwd, "run", "--brief", validBomBrief);
    assert.equal(parsedBom.status, 5, parsedBom.stderr);
    assert.match(
      parsedBom.stderr,
      /cannot resolve deterministic project Git base/,
    );
    assert.equal(invoke(cwd, "status", "--json").status, 0);
    writeFileSync(path.join(cwd, ".capstan/project.json"), "{}\n");
    const invalidConfig = invoke(cwd, "status", "--json");
    assert.equal(invalidConfig.status, 3, invalidConfig.stderr);
  } finally {
    rmSync(cwd, { recursive: true, force: true });
  }
});

test("cstan runtime preflight fails closed before creating controller database", async () => {
  const cwd = mkdtempSync(path.join(os.tmpdir(), "cstan-runtime-preflight-"));
  let core: ControllerCore | undefined;
  try {
    assert.equal(invoke(cwd, "init").status, 0);
    writeFileSync(path.join(cwd, "README.txt"), "preflight fixture\n");
    for (const args of [
      ["init", "--quiet"],
      ["-C", cwd, "config", "user.name", "Capstan Test"],
      ["-C", cwd, "config", "user.email", "capstan-test@example.invalid"],
      ["-C", cwd, "add", "README.txt"],
      ["-C", cwd, "commit", "--quiet", "-m", "baseline"],
    ]) {
      const git = spawnSync("git", args, { cwd, encoding: "utf8" });
      assert.equal(git.status, 0, git.stderr);
    }
    const brief = path.join(cwd, "valid-brief.json");
    writeFileSync(
      brief,
      JSON.stringify({
        schemaVersion: 1,
        taskId: "preflight",
        objective: "Require configured runtime before durable run state",
        acceptanceCriteria: ["first criterion", "second criterion"],
        limits: { maxSlices: 2, maxRunMs: 1_000, maxDispatches: 7 },
        slices: [
          {
            id: "first",
            title: "First",
            description: "First slice",
            role: "Developer",
            dependsOn: [],
            writeScope: ["src"],
            acceptanceCriteria: ["first criterion"],
          },
          {
            id: "second",
            title: "Second",
            description: "Second slice",
            role: "Developer",
            dependsOn: ["first"],
            writeScope: ["src"],
            acceptanceCriteria: ["second criterion"],
          },
        ],
      }),
    );
    const run = invokeWithEnv(
      cwd,
      {
        M1_PROVIDER_HOST: "provider.example",
        M1_HERDR_BINARY: "",
        M1_OMP_BINARY: "",
        M1_OMP_NATIVE_ADDON: "",
        M1_NODE_BINARY: "",
      },
      "run",
      "--brief",
      brief,
    );
    assert.equal(run.status, 5, run.stderr);
    assert.match(run.stderr, /M1_HERDR_BINARY/);
    assert.equal(
      existsSync(path.join(cwd, ".capstan/state/controller.sqlite")),
      false,
    );
    const config = JSON.parse(
      readFileSync(path.join(cwd, ".capstan/project.json"), "utf8"),
    ) as { projectId: string; name: string; stateDirectory: string };
    const credential = readFileSync(
      path.join(cwd, ".capstan/operator.key"),
      "utf8",
    ).trim();
    const base = spawnSync(
      "git",
      ["-C", cwd, "rev-parse", "--verify", "HEAD^{commit}"],
      { encoding: "utf8" },
    );
    assert.equal(base.status, 0, base.stderr);
    core = await ControllerCore.open({
      stateDirectory: config.stateDirectory,
      project: {
        projectId: config.projectId,
        name: config.name,
        ownerCredential: credential,
        initialInputs: [
          {
            kind: "project_config",
            content: {
              schemaVersion: 1,
              projectId: config.projectId,
              name: config.name,
              baseSha: base.stdout.trim(),
            },
          },
          {
            kind: "task_brief",
            content: {
              taskId: "preflight",
              objective: "Require configured runtime before durable run state",
            },
          },
          {
            kind: "acceptance_criteria",
            content: ["first criterion", "second criterion"],
          },
          {
            kind: "policy",
            content: { maxSlices: 2, maxRunMs: 1_000, maxDispatches: 7 },
          },
          { kind: "plan", content: JSON.parse(readFileSync(brief, "utf8")) },
        ],
      },
    });
    core.close();
    core = undefined;
    const priorRun = invoke(cwd, "run", "--brief", brief);
    assert.equal(priorRun.status, 4, priorRun.stderr);
    assert.match(priorRun.stderr, /prior controller run exists/);
    rmSync(path.join(cwd, ".capstan/state"), { recursive: true, force: true });
    mkdirSync(path.join(cwd, ".capstan/state"), { mode: 0o700 });
    for (const args of [
      ["-C", cwd, "add", "-f", ".capstan/operator.key"],
      ["-C", cwd, "commit", "--quiet", "-m", "Accidentally track operator key"],
      ["-C", cwd, "rm", "--cached", "--quiet", ".capstan/operator.key"],
    ]) {
      const git = spawnSync("git", args, { cwd, encoding: "utf8" });
      assert.equal(git.status, 0, git.stderr);
    }
    const leaked = invoke(cwd, "run", "--brief", brief);
    assert.equal(leaked.status, 3, leaked.stderr);
    assert.match(leaked.stderr, /contains \.capstan state/);
    assert.equal(
      existsSync(path.join(cwd, ".capstan/state/controller.sqlite")),
      false,
    );
    const removedKey = spawnSync(
      "git",
      ["-C", cwd, "commit", "--quiet", "-m", "Remove tracked operator key"],
      { encoding: "utf8" },
    );
    assert.equal(removedKey.status, 0, removedKey.stderr);
    const historicalSecret = invoke(cwd, "run", "--brief", brief);
    assert.equal(historicalSecret.status, 3, historicalSecret.stderr);
    assert.match(historicalSecret.stderr, /contains \.capstan state/);
  } finally {
    core?.close();
    rmSync(cwd, { recursive: true, force: true });
  }
});

test("cstan inspect requires an identifier and returns the usage exit code", () => {
  const cwd = mkdtempSync(path.join(os.tmpdir(), "cstan-usage-"));
  try {
    assert.equal(invoke(cwd, "init").status, 0);
    const inspect = invoke(cwd, "inspect");
    assert.equal(inspect.status, 2);
    assert.match(inspect.stderr, /usage:/);
  } finally {
    rmSync(cwd, { recursive: true, force: true });
  }
});
