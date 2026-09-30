import assert from "node:assert/strict";
import { randomUUID } from "node:crypto";
import net from "node:net";
import { spawn, spawnSync } from "node:child_process";
import {
  chmodSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  statSync,
  renameSync,
  symlinkSync,
  writeFileSync,
} from "node:fs";
import os from "node:os";
import path from "node:path";
import { test } from "node:test";
import Database from "better-sqlite3";
import {
  assertTrackedCheckoutMatchesHead,
  createFindingFingerprint,
  findingDefectIdentity,
  escalateSupervisorOverlapFinding,
  reserveDispatchSlot,
  syncConfiguredRoles,
} from "../src/cli.js";
import { listenControl, requestControl } from "../src/control.js";
import { loadCapstanConfig } from "../src/config/capstan-config.js";
import { ControllerCore } from "../src/controller/core.js";
import type { MutationContext } from "../src/controller/types.js";
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

test("Verifier dispatch reserves a slot only while capacity remains", () => {
  assert.equal(reserveDispatchSlot(2, 3), 3);
  assert.throws(
    () => reserveDispatchSlot(3, 3),
    /correction Verifier dispatch exceeded its bounded budget/,
  );
});

test("Supervisor defect codes preserve distinct same-event findings without paraphrase duplicates", () => {
  const fingerprint = (code: unknown) =>
    createFindingFingerprint(
      "assignment-1",
      findingDefectIdentity("event-123", code),
    );
  const first = fingerprint("artifact-missing");
  assert.equal(first, fingerprint(" ARTIFACT-MISSING "));
  assert.notEqual(first, fingerprint("invalid-revision"));
  assert.notEqual(
    first,
    createFindingFingerprint(
      "assignment-2",
      findingDefectIdentity("event-123", "artifact-missing"),
    ),
  );
  assert.throws(() => fingerprint(undefined), /requires a defectCode/);
  assert.throws(() => fingerprint("  "), /bounded stable code/);
  assert.throws(() => fingerprint("x".repeat(65)), /bounded stable code/);
  assert.throws(() => fingerprint("free text"), /bounded stable code/);
});

test("unknown-only Supervisor overlap escalates its finding", async () => {
  const cwd = mkdtempSync(path.join(os.tmpdir(), "cstan-unknown-overlap-"));
  let core: ControllerCore | undefined;
  try {
    assert.equal(invoke(cwd, "init").status, 0);
    const config = JSON.parse(
      readFileSync(path.join(cwd, ".capstan/project.json"), "utf8"),
    ) as { projectId: string; name: string; stateDirectory: string };
    const ownerCredential = readFileSync(
      path.join(cwd, ".capstan/operator.key"),
      "utf8",
    ).trim();
    core = await ControllerCore.open({
      stateDirectory: config.stateDirectory,
      project: {
        projectId: config.projectId,
        name: config.name,
        ownerCredential,
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
              taskId: "unknown-overlap",
              objective: "Escalate an unknown Supervisor overlap",
            },
          },
          { kind: "acceptance_criteria", content: ["Observe overlap"] },
          {
            kind: "policy",
            content: { maxSlices: 2, maxRunMs: 1_000, maxDispatches: 2 },
          },
          { kind: "plan", content: { schemaVersion: 1 } },
        ],
      },
    });
    const mutate = (credential: string) => {
      const requestId = randomUUID();
      return {
        credential,
        requestId,
        idempotencyKey: requestId,
        expectedVersion: core!.stateVersion,
        inputRevision: core!.inputRevision,
      };
    };
    const seatId = "unknown-overlap-supervisor";
    core.createSeat(mutate(ownerCredential), {
      seatId,
      name: "Unknown overlap Supervisor",
      role: "Supervisor",
    });
    const supervisor = core.createActor(mutate(ownerCredential), {
      displayName: "Unknown overlap Supervisor",
      role: "Supervisor",
      seatId,
    });
    const assign = (workItemId: string) => {
      core!.createWorkItem(mutate(ownerCredential), {
        workItemId,
        title: workItemId,
        description: "Observe Supervisor authority",
        requiredRole: "Supervisor",
      });
      core!.markReady(mutate(ownerCredential), workItemId);
      return core!.assignWorkItem(mutate(ownerCredential), workItemId, seatId);
    };
    const previous = assign("unknown-overlap-previous");
    core.confirmContainment(
      mutate(ownerCredential),
      previous.assignmentId,
      "unknown-overlap-previous-contained",
    );
    const current = assign("unknown-overlap-current");
    const db = new Database(
      path.join(config.stateDirectory, "controller.sqlite"),
    );
    try {
      db.prepare(
        "UPDATE assignments SET authority_state = 'unknown' WHERE project_id = ? AND assignment_id IN (?, ?)",
      ).run(config.projectId, previous.assignmentId, current.assignmentId);
    } finally {
      db.close();
    }
    const findingId = "unknown-overlap-finding";
    core.createFinding(mutate(supervisor.credential), {
      findingId,
      workItemId: current.workItemId,
      assignmentId: current.assignmentId,
      generation: current.generation,
      affectedWorkItemId: previous.workItemId,
      affectedSeatId: seatId,
      affectedAssignmentId: previous.assignmentId,
      affectedGeneration: previous.generation,
      fingerprint: "unknown-supervisor-overlap",
      severity: "critical",
      evidence: { code: "overlapping_authority" },
      requestedCorrection: "Operator must resolve Supervisor seat overlap",
      acknowledgementDeadline: new Date(Date.now() + 60_000).toISOString(),
      resolutionCondition: "Operator verifies exclusive Supervisor authority",
      escalationRoute: "operator",
    });
    assert.equal(
      core
        .statusSnapshot()
        .findings.find((entry) => entry.findingId === findingId)?.state,
      "detected",
    );
    escalateSupervisorOverlapFinding(
      core,
      findingId,
      "unknown",
      supervisor.credential,
      ownerCredential,
    );
    assert.equal(
      core
        .statusSnapshot()
        .findings.find((entry) => entry.findingId === findingId)?.state,
      "escalated",
    );
  } finally {
    core?.close();
    rmSync(cwd, { recursive: true, force: true });
  }
});

test("immutable checkout check detects tracked bytes hidden by assume-unchanged", () => {
  const cwd = mkdtempSync(path.join(os.tmpdir(), "cstan-verifier-checkout-"));
  try {
    const git = (...args: string[]) => {
      const result = spawnSync("git", args, { cwd, encoding: "utf8" });
      assert.equal(result.status, 0, result.stderr);
      return result.stdout.trim();
    };
    git("init", "--quiet");
    git("config", "user.name", "Capstan Test");
    git("config", "user.email", "capstan@example.invalid");
    writeFileSync(path.join(cwd, "source.txt"), "committed\n");
    git("add", "source.txt");
    git("commit", "--quiet", "-m", "seed");
    const sha = git("rev-parse", "HEAD");
    assert.doesNotThrow(() => assertTrackedCheckoutMatchesHead(cwd, sha));
    git("update-index", "--assume-unchanged", "source.txt");
    writeFileSync(path.join(cwd, "source.txt"), "edited but hidden\n");
    assert.equal(git("status", "--porcelain"), "");
    assert.throws(
      () => assertTrackedCheckoutMatchesHead(cwd, sha),
      /verification checkout bytes differ/,
    );
    writeFileSync(path.join(cwd, "source.txt"), "committed\n");
    writeFileSync(
      path.join(cwd, ".git", "info", "exclude"),
      ".home/\ngenerated.js\n",
    );
    mkdirSync(path.join(cwd, ".home"));
    writeFileSync(path.join(cwd, ".home", "runtime-state"), "isolated home\n");
    assert.doesNotThrow(() => assertTrackedCheckoutMatchesHead(cwd, sha));
    writeFileSync(path.join(cwd, "generated.js"), "export default 1;\n");
    assert.equal(git("status", "--porcelain"), "");
    assert.throws(
      () => assertTrackedCheckoutMatchesHead(cwd, sha),
      /untracked files in verification checkout/,
    );
    const alternate = mkdtempSync(
      path.join(os.tmpdir(), "cstan-alternate-worktree-"),
    );
    try {
      git("config", "core.worktree", alternate);
      assert.throws(
        () => assertTrackedCheckoutMatchesHead(cwd, sha),
        /untracked files in verification checkout/,
      );
    } finally {
      rmSync(alternate, { recursive: true, force: true });
    }
  } finally {
    rmSync(cwd, { recursive: true, force: true });
  }
});

test("immutable checkout check rejects a symlinked tracked parent directory", () => {
  const cwd = mkdtempSync(path.join(os.tmpdir(), "cstan-verifier-parent-"));
  const external = mkdtempSync(
    path.join(os.tmpdir(), "cstan-verifier-external-"),
  );
  try {
    const git = (...args: string[]) => {
      const result = spawnSync("git", args, { cwd, encoding: "utf8" });
      assert.equal(result.status, 0, result.stderr);
      return result.stdout.trim();
    };
    git("init", "--quiet");
    git("config", "user.name", "Capstan Test");
    git("config", "user.email", "capstan@example.invalid");
    mkdirSync(path.join(cwd, "lib"));
    writeFileSync(path.join(cwd, "lib", "source.txt"), "committed\n");
    git("add", "lib/source.txt");
    git("commit", "--quiet", "-m", "seed");
    const sha = git("rev-parse", "HEAD");
    assert.doesNotThrow(() => assertTrackedCheckoutMatchesHead(cwd, sha));
    git("update-index", "--assume-unchanged", "lib/source.txt");
    renameSync(path.join(cwd, "lib"), path.join(external, "lib"));
    symlinkSync(path.join(external, "lib"), path.join(cwd, "lib"));
    assert.throws(
      () => assertTrackedCheckoutMatchesHead(cwd, sha),
      /symlinked parent directory/,
    );
  } finally {
    rmSync(cwd, { recursive: true, force: true });
    rmSync(external, { recursive: true, force: true });
  }
});

test("immutable checkout check rejects tracked links into mutable runtime home", () => {
  const cwd = mkdtempSync(path.join(os.tmpdir(), "cstan-verifier-link-"));
  try {
    const git = (...args: string[]) => {
      const result = spawnSync("git", args, { cwd, encoding: "utf8" });
      assert.equal(result.status, 0, result.stderr);
      return result.stdout.trim();
    };
    git("init", "--quiet");
    git("config", "user.name", "Capstan Test");
    git("config", "user.email", "capstan@example.invalid");
    writeFileSync(path.join(cwd, ".git", "info", "exclude"), ".home/\n");
    mkdirSync(path.join(cwd, ".home"));
    writeFileSync(path.join(cwd, ".home", "source.txt"), "mutable\n");
    symlinkSync(".home/source.txt", path.join(cwd, "source.txt"));
    git("add", "source.txt");
    git("commit", "--quiet", "-m", "link");
    assert.throws(
      () => assertTrackedCheckoutMatchesHead(cwd, git("rev-parse", "HEAD")),
      /unsupported verification checkout entry/,
    );
  } finally {
    rmSync(cwd, { recursive: true, force: true });
  }
});

test("immutable checkout check rejects a symlinked runtime home", () => {
  const cwd = mkdtempSync(path.join(os.tmpdir(), "cstan-verifier-home-link-"));
  const external = mkdtempSync(path.join(os.tmpdir(), "cstan-verifier-home-"));
  try {
    const git = (...args: string[]) => {
      const result = spawnSync("git", args, { cwd, encoding: "utf8" });
      assert.equal(result.status, 0, result.stderr);
      return result.stdout.trim();
    };
    git("init", "--quiet");
    git("config", "user.name", "Capstan Test");
    git("config", "user.email", "capstan@example.invalid");
    writeFileSync(path.join(cwd, "tracked.txt"), "tracked\n");
    git("add", "tracked.txt");
    git("commit", "--quiet", "-m", "seed");
    writeFileSync(path.join(cwd, ".git", "info", "exclude"), ".home/\n");
    symlinkSync(external, path.join(cwd, ".home"));
    assert.throws(
      () =>
        assertTrackedCheckoutMatchesHead(
          cwd,
          git("rev-parse", "--verify", "HEAD"),
        ),
      /root entry .home is not a real directory/,
    );
  } finally {
    rmSync(cwd, { recursive: true, force: true });
    rmSync(external, { recursive: true, force: true });
  }
});

test("immutable checkout check ignores local replacement objects", () => {
  const cwd = mkdtempSync(path.join(os.tmpdir(), "cstan-verifier-replace-"));
  try {
    const git = (...args: string[]) => {
      const result = spawnSync("git", args, { cwd, encoding: "utf8" });
      assert.equal(result.status, 0, result.stderr);
      return result.stdout.trim();
    };
    git("init", "--quiet");
    git("config", "user.name", "Capstan Test");
    git("config", "user.email", "capstan@example.invalid");
    writeFileSync(path.join(cwd, "source.txt"), "original\n");
    git("add", "source.txt");
    git("commit", "--quiet", "-m", "original");
    const original = git("rev-parse", "HEAD");
    writeFileSync(path.join(cwd, "source.txt"), "substitute\n");
    git("add", "source.txt");
    git("commit", "--quiet", "-m", "replacement");
    const replacement = git("rev-parse", "HEAD");
    git("replace", original, replacement);
    assert.throws(
      () => assertTrackedCheckoutMatchesHead(cwd, original),
      /verification checkout bytes differ/,
    );
    const objectDirectory = process.env.GIT_OBJECT_DIRECTORY;
    process.env.GIT_OBJECT_DIRECTORY = path.join(cwd, "missing-objects");
    try {
      assert.throws(
        () => assertTrackedCheckoutMatchesHead(cwd, original),
        /verification checkout bytes differ/,
      );
    } finally {
      if (objectDirectory === undefined)
        delete process.env.GIT_OBJECT_DIRECTORY;
      else process.env.GIT_OBJECT_DIRECTORY = objectDirectory;
    }
  } finally {
    rmSync(cwd, { recursive: true, force: true });
  }
});

test("cstan init creates private project-local config and status exposes four seats as JSON", () => {
  const cwd = mkdtempSync(path.join(os.tmpdir(), "cstan-cli-"));
  try {
    const gitInit = spawnSync("git", ["init", "--quiet"], {
      cwd,
      encoding: "utf8",
    });
    assert.equal(gitInit.status, 0, gitInit.stderr);
    const init = invoke(cwd, "init");
    assert.equal(init.status, 0, init.stderr);
    const ignored = spawnSync(
      "git",
      ["check-ignore", "-q", ".capstan/operator.key"],
      { cwd, encoding: "utf8" },
    );
    assert.equal(
      ignored.status,
      0,
      "operator key must be excluded from staging",
    );
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
      evidence: ["implemented and verified"],
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
        observation: "status checks passed",
        exitStatus: 0,
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
    const malformedResponse = await new Promise<string>((resolve, reject) => {
      const socket = net.createConnection(
        path.join(config.stateDirectory, "control.sock"),
      );
      let response = "";
      socket.once("connect", () => {
        socket.write(
          Buffer.concat([
            Buffer.from(
              JSON.stringify({ token: credential, action: "status" }),
            ),
            Buffer.from([0xff, 0x0a]),
          ]),
        );
      });
      socket.on("data", (chunk) => (response += chunk.toString("utf8")));
      socket.once("end", () => resolve(response));
      socket.once("error", reject);
    });
    const malformedResult = JSON.parse(malformedResponse) as {
      error?: string;
      result?: unknown;
    };
    assert.equal(malformedResult.result, undefined);
    assert.match(malformedResult.error ?? "", /encoded data/i);
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
        developerEvidence: string[];
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
    assert.deepEqual(evidence.developerEvidence, ["implemented and verified"]);
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
    const deeplyNested = path.join(cwd, "deeply-nested.json");
    writeFileSync(
      deeplyNested,
      `{"value":${"[".repeat(300)}0${"]".repeat(300)}}`,
    );
    const overNested = invoke(cwd, "run", "--brief", deeplyNested);
    assert.equal(overNested.status, 3, overNested.stderr);
    assert.match(overNested.stderr, /JSON nesting depth/);
    const overLimit = path.join(cwd, "over-limit.json");
    writeFileSync(
      overLimit,
      `\uFEFF${JSON.stringify({
        schemaVersion: 1,
        taskId: "over-limit",
        objective: "Reject limits beyond project configuration",
        acceptanceCriteria: ["first criterion", "second criterion"],
        limits: { maxSlices: 2, maxRunMs: 3_600_001, maxDispatches: 16 },
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
        limits: { maxSlices: 2, maxRunMs: 60_000, maxDispatches: 16 },
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
    writeFileSync(
      validBomBrief,
      `\uFEFF${readFileSync(validBomBrief, "utf8")}`,
    );
    const doubleBom = invoke(cwd, "run", "--brief", validBomBrief);
    assert.equal(doubleBom.status, 3, doubleBom.stderr);
    writeFileSync(validBomBrief, readFileSync(validBomBrief, "utf8").slice(1));
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
        limits: { maxSlices: 2, maxRunMs: 1_000, maxDispatches: 16 },
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
            content: { maxSlices: 2, maxRunMs: 1_000, maxDispatches: 16 },
          },
          { kind: "plan", content: JSON.parse(readFileSync(brief, "utf8")) },
        ],
      },
    });
    core.close();
    core = undefined;
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

test("cstan rejects nested controller state and non-root project clones", () => {
  const cwd = mkdtempSync(path.join(os.tmpdir(), "cstan-nested-state-"));
  try {
    assert.equal(invoke(cwd, "init").status, 0);
    const baseline = path.join(cwd, "README.txt");
    writeFileSync(baseline, "baseline\n");
    for (const args of [
      ["init", "--quiet"],
      ["config", "user.name", "Capstan Test"],
      ["config", "user.email", "capstan-test@example.invalid"],
      ["add", "README.txt"],
      ["commit", "--quiet", "-m", "baseline"],
    ]) {
      const result = spawnSync("git", args, { cwd, encoding: "utf8" });
      assert.equal(result.status, 0, result.stderr);
    }
    const brief = path.join(cwd, "brief.json");
    writeFileSync(
      brief,
      JSON.stringify({
        schemaVersion: 1,
        taskId: "isolation",
        objective: "Keep operator credentials out of role workspaces",
        acceptanceCriteria: ["first", "second"],
        limits: { maxSlices: 2, maxRunMs: 1000, maxDispatches: 16 },
        slices: [
          {
            id: "first",
            title: "First",
            description: "First",
            role: "Developer",
            dependsOn: [],
            writeScope: ["src"],
            acceptanceCriteria: ["first"],
          },
          {
            id: "second",
            title: "Second",
            description: "Second",
            role: "Developer",
            dependsOn: ["first"],
            writeScope: ["src"],
            acceptanceCriteria: ["second"],
          },
        ],
      }),
    );
    const nested = path.join(cwd, "nested");
    mkdirSync(nested);
    assert.equal(invoke(nested, "init").status, 0);
    const subdirectoryRun = invoke(nested, "run", "--brief", brief);
    assert.equal(subdirectoryRun.status, 3, subdirectoryRun.stderr);
    assert.match(subdirectoryRun.stderr, /Git repository root/);
    assert.equal(
      existsSync(path.join(nested, ".capstan/state/controller.sqlite")),
      false,
    );
    writeFileSync(path.join(nested, "secret.txt"), "private\n");
    for (const args of [
      ["add", "-f", "nested/.capstan/operator.key"],
      ["commit", "--quiet", "-m", "nested credential in history"],
      ["rm", "--cached", "--quiet", "nested/.capstan/operator.key"],
      ["commit", "--quiet", "-m", "remove nested credential"],
    ]) {
      const result = spawnSync("git", args, { cwd, encoding: "utf8" });
      assert.equal(result.status, 0, result.stderr);
    }
    const nestedHistory = invoke(cwd, "run", "--brief", brief);
    assert.equal(nestedHistory.status, 3, nestedHistory.stderr);
    assert.match(nestedHistory.stderr, /contains \.capstan state/);
    assert.equal(
      existsSync(path.join(cwd, ".capstan/state/controller.sqlite")),
      false,
    );
  } finally {
    rmSync(cwd, { recursive: true, force: true });
  }
});

test("cstan rejects duplicate JSON members before validating a brief", () => {
  const cwd = mkdtempSync(path.join(os.tmpdir(), "cstan-duplicate-json-"));
  try {
    assert.equal(invoke(cwd, "init").status, 0);
    const brief = path.join(cwd, "brief.json");
    writeFileSync(
      brief,
      '{"schemaVersion":1,"limits":{"maxSlices":2,"maxSlices":99}}\n',
    );
    const result = invoke(cwd, "run", "--brief", brief);
    assert.equal(result.status, 3, result.stderr);
    assert.match(result.stderr, /duplicate JSON member: maxSlices/);
    assert.equal(
      existsSync(path.join(cwd, ".capstan/state/controller.sqlite")),
      false,
    );
  } finally {
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

test("cstan pause and cancel need the live controller and an authenticated socket", async () => {
  const cwd = mkdtempSync(path.join(os.tmpdir(), "cstan-control-"));
  const resumeGate = Promise.withResolvers<void>();
  let core: ControllerCore | undefined;
  let closeControl: (() => Promise<void>) | undefined;
  try {
    assert.equal(invoke(cwd, "init").status, 0);
    const offline = invoke(cwd, "pause");
    assert.equal(offline.status, 4);
    assert.match(offline.stderr, /requires the foreground controller/);

    const config = JSON.parse(
      readFileSync(path.join(cwd, ".capstan/project.json"), "utf8"),
    ) as { projectId: string; name: string; stateDirectory: string };
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
          { kind: "project_config", content: { name: config.name } },
          { kind: "task_brief", content: { objective: "control" } },
          { kind: "acceptance_criteria", content: ["control"] },
          { kind: "policy", content: { maxRunMs: 60_000 } },
          { kind: "plan", content: { slices: [] } },
        ],
      },
      workspaceRoot: cwd,
    });
    const actions: string[] = [];
    const socketPath = path.join(config.stateDirectory, "control.sock");
    closeControl = await listenControl(
      socketPath,
      credential,
      core,
      async (action) => {
        actions.push(action);
        if (action === "cancel")
          throw new Error(
            "cancellation containment incomplete: Cannot connect to the Docker daemon",
          );
        if (action === "resume") await resumeGate.promise;
        return { run: { state: "paused" } };
      },
    );
    await assert.rejects(
      requestControl(socketPath, "wrong-credential", "pause"),
      /unauthorized/,
    );
    assert.equal(actions.length, 0);

    const pause = await invokeAsync(cwd, "pause");
    assert.equal(pause.status, 0, pause.stderr);
    assert.deepEqual(actions, ["pause"]);

    const cancel = await invokeAsync(cwd, "cancel");
    assert.equal(cancel.status, 5);
    assert.match(cancel.stderr, /containment incomplete.*Docker daemon/);
    assert.doesNotMatch(cancel.stderr, /requires the foreground controller/);
    assert.deepEqual(actions, ["pause", "cancel"]);

    const abandoned = net.createConnection(socketPath);
    await new Promise<void>((resolve, reject) => {
      abandoned.once("connect", () => {
        abandoned.write(
          `${JSON.stringify({ token: credential, action: "resume" })}\n`,
        );
        resolve();
      });
      abandoned.once("error", reject);
    });
    while (!actions.includes("resume"))
      await new Promise((resolve) => setTimeout(resolve, 10));
    abandoned.destroy();
    resumeGate.resolve();
    await new Promise((resolve) => setTimeout(resolve, 100));
    await requestControl(socketPath, credential, "pause");
    assert.deepEqual(actions, ["pause", "cancel", "resume", "pause"]);
  } finally {
    resumeGate.resolve();
    await closeControl?.();
    core?.close();
    rmSync(cwd, { recursive: true, force: true });
  }
});

const configInputKinds = [
  "project_config",
  "task_brief",
  "acceptance_criteria",
  "policy",
  "plan",
] as const;

async function openInitializedCore(cwd: string): Promise<ControllerCore> {
  const config = JSON.parse(
    readFileSync(path.join(cwd, ".capstan/project.json"), "utf8"),
  ) as { projectId: string; name: string; stateDirectory: string };
  return ControllerCore.open({
    stateDirectory: config.stateDirectory,
    project: {
      projectId: config.projectId,
      name: config.name,
      ownerCredential: readFileSync(
        path.join(cwd, ".capstan/operator.key"),
        "utf8",
      ).trim(),
      initialInputs: configInputKinds.map((kind) => ({
        kind,
        content:
          kind === "acceptance_criteria"
            ? ["criterion"]
            : { kind, revision: 1 },
      })),
    },
  });
}

test("cstan init writes a starter capstan.toml and never replaces an existing one", () => {
  const fresh = mkdtempSync(path.join(os.tmpdir(), "cstan-config-init-"));
  const existing = mkdtempSync(path.join(os.tmpdir(), "cstan-config-keep-"));
  try {
    const created = invoke(fresh, "init");
    assert.equal(created.status, 0, created.stderr);
    assert.match(created.stdout, /Wrote starter capstan\.toml/);
    assert.equal(
      statSync(path.join(fresh, "capstan.toml")).mode & 0o777,
      0o600,
    );
    const check = invoke(fresh, "config", "check");
    assert.equal(check.status, 0, check.stderr);
    const resolved = JSON.parse(check.stdout) as {
      roles: Array<{ name: string }>;
    };
    assert.deepEqual(
      resolved.roles.map((role) => role.name),
      ["pm", "developer", "reviewer"],
    );

    const custom =
      'schema_version = 1\n# mine\n[hosts.h]\nkind = "claude"\n[roles.lead]\nkind = "PM"\nhost = "h"\n';
    writeFileSync(path.join(existing, "capstan.toml"), custom, { mode: 0o600 });
    const kept = invoke(existing, "init");
    assert.equal(kept.status, 0, kept.stderr);
    assert.match(kept.stdout, /Kept existing capstan\.toml/);
    assert.equal(
      readFileSync(path.join(existing, "capstan.toml"), "utf8"),
      custom,
    );
  } finally {
    rmSync(fresh, { recursive: true, force: true });
    rmSync(existing, { recursive: true, force: true });
  }
});

test("cstan init keeps a dangling capstan.toml symlink instead of failing half-done", () => {
  const cwd = mkdtempSync(path.join(os.tmpdir(), "cstan-config-dangling-"));
  try {
    symlinkSync("nowhere.toml", path.join(cwd, "capstan.toml"));
    const result = invoke(cwd, "init");
    assert.equal(result.status, 0, result.stderr);
    assert.match(result.stdout, /Kept existing capstan\.toml/);
    assert.ok(existsSync(path.join(cwd, ".capstan/operator.key")));
    assert.ok(existsSync(path.join(cwd, ".capstan/project.json")));
  } finally {
    rmSync(cwd, { recursive: true, force: true });
  }
});

test("cstan config check exits 3 for a missing or invalid file and does not echo a secret", () => {
  const cwd = mkdtempSync(path.join(os.tmpdir(), "cstan-config-check-"));
  try {
    const missing = invoke(cwd, "config", "check");
    assert.equal(missing.status, 3);
    assert.match(missing.stderr, /capstan\.toml does not exist/);
    writeFileSync(
      path.join(cwd, "capstan.toml"),
      "model = sk-live-ABCDEFGHIJKLMNOP1234\n",
      {
        mode: 0o600,
      },
    );
    const invalid = invoke(cwd, "config", "check");
    assert.equal(invalid.status, 3);
    assert.match(invalid.stderr, /not valid TOML at line 1/);
    assert.ok(!invalid.stderr.includes("ABCDEFGH"));
    assert.equal(invoke(cwd, "config").status, 2);
    assert.equal(invoke(cwd, "config", "check", "extra").status, 2);
    assert.equal(invoke(cwd, "config", "other").status, 2);
  } finally {
    rmSync(cwd, { recursive: true, force: true });
  }
});

test("cstan config sync writes once and a second run writes nothing", async () => {
  const cwd = mkdtempSync(path.join(os.tmpdir(), "cstan-config-sync-"));
  try {
    assert.equal(invoke(cwd, "init").status, 0);
    const blocked = invoke(cwd, "config", "sync");
    assert.equal(blocked.status, 4);
    assert.match(blocked.stderr, /controller record does not exist/);

    (await openInitializedCore(cwd)).close();
    const first = invoke(cwd, "config", "sync");
    assert.equal(first.status, 0, first.stderr);
    assert.deepEqual(JSON.parse(first.stdout), {
      schemaVersion: 1,
      changed: true,
      inserted: ["developer", "pm", "reviewer"],
      updated: [],
      reactivated: [],
      retired: [],
    });
    const versionOf = async (): Promise<number> => {
      const core = await openInitializedCore(cwd);
      try {
        return core.stateVersion;
      } finally {
        core.close();
      }
    };
    const afterFirst = await versionOf();
    const second = invoke(cwd, "config", "sync");
    assert.equal(second.status, 0, second.stderr);
    assert.equal(
      (JSON.parse(second.stdout) as { changed: boolean }).changed,
      false,
    );
    assert.equal(await versionOf(), afterFirst);

    const toml = path.join(cwd, "capstan.toml");
    writeFileSync(
      toml,
      readFileSync(toml, "utf8").replace(
        'kind = "Verifier"',
        'kind = "Verifier"\nmodel = "opus"',
      ),
      { mode: 0o600 },
    );
    const third = invoke(cwd, "config", "sync");
    assert.equal(third.status, 0, third.stderr);
    assert.deepEqual(
      (JSON.parse(third.stdout) as { updated: string[] }).updated,
      ["reviewer"],
    );

    writeFileSync(
      toml,
      readFileSync(toml, "utf8").replace(
        "schema_version = 1\n",
        'schema_version = 1\n[project]\nname = "other"\n',
      ),
      { mode: 0o600 },
    );
    const mismatched = invoke(cwd, "config", "sync");
    assert.equal(mismatched.status, 3);
    assert.match(mismatched.stderr, /project\.name does not match/);
  } finally {
    rmSync(cwd, { recursive: true, force: true });
  }
});

test("cstan config sync matches the project name across Unicode normalization forms", async () => {
  const cwd = mkdtempSync(path.join(os.tmpdir(), "cstan-caf\u00e9-"));
  try {
    assert.equal(invoke(cwd, "init").status, 0);
    (await openInitializedCore(cwd)).close();
    const name = path.basename(cwd);
    assert.notEqual(name.normalize("NFD"), name.normalize("NFC"));
    const toml = path.join(cwd, "capstan.toml");
    writeFileSync(
      toml,
      readFileSync(toml, "utf8").replace(
        "schema_version = 1\n",
        `schema_version = 1\n[project]\nname = ${JSON.stringify(name.normalize("NFD"))}\n\n`,
      ),
      { mode: 0o600 },
    );
    const result = invoke(cwd, "config", "sync");
    assert.equal(result.status, 0, result.stderr);
  } finally {
    rmSync(cwd, { recursive: true, force: true });
  }
});

test("role sync retries one version conflict and exits blocked on a second", async () => {
  const cwd = mkdtempSync(path.join(os.tmpdir(), "cstan-config-conflict-"));
  let core: ControllerCore | undefined;
  try {
    assert.equal(invoke(cwd, "init").status, 0);
    core = await openInitializedCore(cwd);
    const credential = readFileSync(
      path.join(cwd, ".capstan/operator.key"),
      "utf8",
    ).trim();
    const roleConfig = loadCapstanConfig(cwd);
    let interleavedSeats = 0;
    let conflictsToForce = 0;
    const freshContext = (): MutationContext => ({
      credential,
      requestId: `req-${randomUUID()}`,
      idempotencyKey: `idem-${randomUUID()}`,
      expectedVersion: core!.stateVersion,
      inputRevision: core!.inputRevision,
    });
    const contextThenInterleave = (): MutationContext => {
      const stale = freshContext();
      if (conflictsToForce > 0) {
        conflictsToForce -= 1;
        interleavedSeats += 1;
        core!.createSeat(freshContext(), {
          seatId: `interleaved-${interleavedSeats}`,
          name: `interleaved-${interleavedSeats}`,
          role: "Developer",
        });
      }
      return stale;
    };
    conflictsToForce = 1;
    assert.equal(
      syncConfiguredRoles(core, roleConfig, contextThenInterleave).changed,
      true,
    );
    assert.equal(core.roleDefinitions().length, 3);
    const changed = {
      ...roleConfig,
      roles: roleConfig.roles.map((role) => ({
        ...role,
        configHash: "f".repeat(64),
      })),
    };
    conflictsToForce = 2;
    assert.throws(
      () => syncConfiguredRoles(core!, changed, contextThenInterleave),
      /conflicted with another change twice/,
    );
    assert.ok(
      core
        .roleDefinitions()
        .every((role) => role.configHash !== "f".repeat(64)),
    );
  } finally {
    core?.close();
    rmSync(cwd, { recursive: true, force: true });
  }
});
