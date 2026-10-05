import assert from "node:assert/strict";

// No test may reach a real Herdr session: the daemon and `cstan start` stay out of it.
process.env.CAPSTAN_LAUNCH = "off";
// A shell that runs as a Capstan agent exports these; inherited, they would send every `cstan` call below to that agent's real controller.
delete process.env.CAPSTAN_TOKEN;
delete process.env.CAPSTAN_SOCKET;
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
  renameSync,
  symlinkSync,
  writeFileSync,
} from "node:fs";
import net from "node:net";
import os from "node:os";
import path from "node:path";
import { test } from "node:test";
import { openSqlite } from "../src/controller/sqlite.js";
import {
  assertTrackedCheckoutMatchesHead,
  createFindingFingerprint,
  findingDefectIdentity,
  reserveDispatchSlot,
  syncConfiguredRoles,
  pmMailLines,
} from "../src/cli.js";
import { loadCapstanConfig } from "../src/config/capstan-config.js";
import { DESIGNER_PROMPT } from "../src/roles/designer-prompt.js";
import { ControllerCore } from "../src/controller/core.js";
import {
  insertAssignment,
  insertCandidate,
  insertDependency,
  insertWorkItem,
  seedLedger,
  setAcceptedCandidate,
} from "./legacy-rows.js";
import type { MutationContext } from "../src/controller/types.js";
const cli = path.resolve("dist/src/cli.js");

/** `cstan start` refuses a folder without a git commit; these tests are about the daemon, so the folder gets an empty one. */
function ensureCommit(cwd: string, args: string[]): void {
  if (args[0] !== "start" || existsSync(path.join(cwd, ".git"))) return;
  for (const git of [
    ["init", "--quiet"],
    [
      "-c",
      "user.name=t",
      "-c",
      "user.email=t@example.com",
      "commit",
      "--quiet",
      "--allow-empty",
      "-m",
      "chore: initial commit",
    ],
  ])
    spawnSync("git", ["-C", cwd, ...git]);
}

function invokeWithEnv(cwd: string, env: NodeJS.ProcessEnv, ...args: string[]) {
  ensureCommit(cwd, args);
  return spawnSync(process.execPath, [cli, ...args], {
    cwd,
    encoding: "utf8",
    env: { ...process.env, ...env },
  });
}

function invoke(cwd: string, ...args: string[]) {
  return invokeWithEnv(cwd, {}, ...args);
}

function invokeAsync(
  cwd: string,
  ...args: string[]
): Promise<{ status: number | null; stdout: string; stderr: string }> {
  ensureCommit(cwd, args);
  return new Promise((resolve, reject) => {
    const child = spawn(process.execPath, [cli, ...args], {
      cwd,
      env: process.env,
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
    assert.deepEqual(snapshot.nextLegalActions, []);
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
test("cstan status and inspect read a populated ledger through the executable", async () => {
  const cwd = mkdtempSync(path.join(os.tmpdir(), "cstan-live-status-"));
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
    const pmActor = core.createActor(mutate(credential), {
      displayName: "Status PM",
      role: "PM",
      seatId: pm.seatId,
    });
    const prerequisiteAssignmentId = "status-prerequisite-assignment";
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
    seedLedger(config.stateDirectory, (db) => {
      const id = config.projectId;
      insertWorkItem(db, id, {
        workItemId: "status-prerequisite",
        title: "Status prerequisite",
        description: "Keep this prerequisite open for blocker reporting",
        requiredRole: "PM",
        state: "running",
      });
      insertAssignment(db, id, {
        assignmentId: prerequisiteAssignmentId,
        workItemId: "status-prerequisite",
        seatId: pm.seatId,
        workerActorId: pmActor.actorId,
        state: "running",
      });
      insertWorkItem(db, id, {
        workItemId: "status-dependent",
        title: "Status dependent",
        description: "Expose its prerequisite blocker",
      });
      insertDependency(db, id, "status-dependent", "status-prerequisite");
      insertWorkItem(db, id, {
        workItemId: "status-pending-ready",
        title: "Status pending ready",
        description: "Expose the next legal readiness transition",
      });
      insertWorkItem(db, id, {
        workItemId: "status-large-inspect",
        title: "Status large inspect",
        description: "x".repeat(20_000),
      });
      insertWorkItem(db, id, {
        workItemId: "status-candidate-work",
        title: "Status candidate",
        description: "Expose durable candidate evidence",
        state: "awaiting_verification",
      });
      insertAssignment(db, id, {
        assignmentId: "status-candidate-assignment",
        workItemId: "status-candidate-work",
        seatId: developer.seatId,
        workerActorId: developerActor.actorId,
        state: "completed",
        authorityState: "contained",
      });
      insertWorkItem(db, id, {
        workItemId: "status-verifier-work",
        title: "Status verifier",
        description: "Verify the status candidate",
        requiredRole: "Verifier",
        parentWorkItemId: "status-candidate-work",
        state: "accepted",
      });
      insertAssignment(db, id, {
        assignmentId: "status-verifier-assignment",
        workItemId: "status-verifier-work",
        seatId: verifier.seatId,
        workerActorId: verifierActor.actorId,
        state: "completed",
        authorityState: "contained",
      });
      insertCandidate(db, id, {
        candidateId: "status-candidate",
        assignmentId: "status-candidate-assignment",
        commitSha: "c".repeat(40),
        developerEvidence: ["implemented and verified"],
        evidence: [
          {
            evidenceId: "status-candidate-evidence",
            verifierAssignmentId: "status-verifier-assignment",
            criterion: "The controller is active",
            artifactRef: "artifact://status/candidate-evidence",
            observation: "status checks passed",
            exitStatus: 0,
          },
        ],
      });
      setAcceptedCandidate(db, id, "status-candidate-work", "status-candidate");
    });
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
    assert.equal(pmStatus?.assignmentId, prerequisiteAssignmentId);
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
    assert.equal(evidence.candidateId, "status-candidate");
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
  } finally {
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
    seedLedger(config.stateDirectory, (db) =>
      insertWorkItem(db, config.projectId, {
        workItemId: "inspectable",
        title: "Inspectable item",
        description: "Inspect this persisted task",
        requiredRole: "PM",
      }),
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
      ["pm", "developer", "designer", "reviewer", "tester", "supervisor"],
    );
    assert.match(created.stdout, /Wrote roles\/designer\.md/);
    assert.equal(
      readFileSync(path.join(fresh, "roles", "designer.md"), "utf8"),
      DESIGNER_PROMPT,
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

test("cstan init keeps an existing roles/designer.md and writes no role file next to a kept capstan.toml", () => {
  const withRole = mkdtempSync(path.join(os.tmpdir(), "cstan-role-keep-"));
  const withConfig = mkdtempSync(path.join(os.tmpdir(), "cstan-role-cfg-"));
  try {
    mkdirSync(path.join(withRole, "roles"));
    writeFileSync(path.join(withRole, "roles", "designer.md"), "mine\n");
    const kept = invoke(withRole, "init");
    assert.equal(kept.status, 0, kept.stderr);
    assert.match(kept.stdout, /Kept existing roles\/designer\.md/);
    assert.equal(
      readFileSync(path.join(withRole, "roles", "designer.md"), "utf8"),
      "mine\n",
    );
    writeFileSync(
      path.join(withConfig, "capstan.toml"),
      'schema_version = 1\n[hosts.h]\nkind = "claude"\n[roles.lead]\nkind = "PM"\nhost = "h"\n',
      { mode: 0o600 },
    );
    const second = invoke(withConfig, "init");
    assert.equal(second.status, 0, second.stderr);
    assert.equal(existsSync(path.join(withConfig, "roles")), false);
  } finally {
    rmSync(withRole, { recursive: true, force: true });
    rmSync(withConfig, { recursive: true, force: true });
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

test("cstan config check warns about every role that runs unattended on a host other than Claude", () => {
  const cwd = mkdtempSync(path.join(os.tmpdir(), "cstan-config-warn-"));
  try {
    writeFileSync(
      path.join(cwd, "capstan.toml"),
      [
        "schema_version = 1",
        '[hosts.claude]\nkind = "claude"',
        '[hosts.cx]\nkind = "codex"',
        '[roles.pm]\nkind = "PM"\nhost = "claude"',
        '[roles.dev]\nkind = "Developer"\nhost = "cx"\npermission_mode = "auto"',
        '[roles.dev2]\nkind = "Developer"\nhost = "claude"',
        "",
      ].join("\n\n"),
      { mode: 0o600 },
    );
    const result = invoke(cwd, "config", "check");
    assert.equal(result.status, 0, result.stderr);
    assert.match(
      result.stderr,
      /warning: role dev runs on codex with full access and no approval prompts/,
    );
    assert.doesNotMatch(result.stderr, /role (pm|dev2) runs/);
    assert.doesNotThrow(() => JSON.parse(result.stdout));
  } finally {
    rmSync(cwd, { recursive: true, force: true });
  }
});

test("cstan herdr-config prints a Herdr config snippet that uses the project token and takes no arguments", () => {
  const cwd = mkdtempSync(path.join(os.tmpdir(), "cstan-herdr-config-"));
  try {
    const result = invoke(cwd, "herdr-config");
    assert.equal(result.status, 0, result.stderr);
    assert.match(result.stdout, /\[ui\.sidebar\.agents\]/);
    assert.match(
      result.stdout,
      /rows = \[\["state_icon", "\$project", "workspace"\], \["agent"\]\]/,
    );
    assert.match(result.stdout, /\[ui\.sidebar\.spaces\]/);
    assert.equal(invoke(cwd, "herdr-config", "extra").status, 2);
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
      inserted: [
        "designer",
        "developer",
        "pm",
        "reviewer",
        "supervisor",
        "tester",
      ],
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
    assert.equal(core.roleDefinitions().length, 6);
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

function daemonPid(cwd: string): number | undefined {
  try {
    return Number(
      readFileSync(path.join(cwd, ".capstan/state/daemon.pid"), "utf8").trim(),
    );
  } catch {
    return undefined;
  }
}

function killDaemon(cwd: string): void {
  const pid = daemonPid(cwd);
  if (pid === undefined) return;
  try {
    process.kill(pid, "SIGKILL");
  } catch {
    // already gone
  }
}

async function waitGone(pid: number): Promise<void> {
  for (let attempt = 0; attempt < 100; attempt += 1) {
    try {
      process.kill(pid, 0);
    } catch {
      return;
    }
    await new Promise((resolve) => setTimeout(resolve, 50));
  }
  throw new Error("process did not exit");
}

function tableCounts(cwd: string): Record<string, number> {
  const db = openSqlite(path.join(cwd, ".capstan/state/controller.sqlite"), {
    readOnly: true,
  });
  try {
    const tables = (
      db
        .prepare(
          "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name",
        )
        .all() as Array<{ name: string }>
    ).map((row) => row.name);
    return Object.fromEntries(
      tables.map((name) => [
        name,
        (db.prepare(`SELECT COUNT(*) AS n FROM ${name}`).get() as { n: number })
          .n,
      ]),
    );
  } finally {
    db.close();
  }
}

function tableRows(cwd: string, table: string): string[] {
  const db = openSqlite(path.join(cwd, ".capstan/state/controller.sqlite"), {
    readOnly: true,
  });
  try {
    return (db.prepare(`SELECT * FROM ${table}`).all() as unknown[])
      .map((row) => JSON.stringify(row))
      .sort();
  } finally {
    db.close();
  }
}

test("cstan start runs one daemon, a repeat and a second daemon are handled, stop cleans up", () => {
  const cwd = mkdtempSync(path.join(os.tmpdir(), "cstan-daemon-life-"));
  try {
    assert.equal(invoke(cwd, "init").status, 0);
    const first = invoke(cwd, "start", "--json");
    assert.equal(first.status, 0, first.stderr);
    const started = JSON.parse(first.stdout) as {
      running: boolean;
      pid: number;
      started: boolean;
    };
    assert.deepEqual([started.running, started.started], [true, true]);
    assert.equal(daemonPid(cwd), started.pid);
    assert.equal(
      statSync(path.join(cwd, ".capstan/state/control.sock")).mode & 0o777,
      0o600,
    );
    assert.equal(
      statSync(path.join(cwd, ".capstan/state/daemon.pid")).mode & 0o777,
      0o600,
    );
    assert.equal(
      statSync(path.join(cwd, ".capstan/daemon.log")).mode & 0o777,
      0o600,
    );

    const again = JSON.parse(invoke(cwd, "start", "--json").stdout) as {
      pid: number;
      started: boolean;
    };
    assert.deepEqual([again.pid, again.started], [started.pid, false]);

    const second = invoke(cwd, "daemon");
    assert.equal(second.status, 4);
    assert.match(
      second.stderr,
      /another cooperating controller owns this project/,
    );
    assert.equal(
      daemonPid(cwd),
      started.pid,
      "the second daemon left the first one's pid file alone",
    );

    const status = invoke(cwd, "status", "--json");
    assert.equal(status.status, 0, status.stderr);
    assert.equal(
      (JSON.parse(status.stdout) as { schemaVersion: number }).schemaVersion,
      1,
    );
    const stopped = invoke(cwd, "stop", "--json");
    assert.equal(stopped.status, 0, stopped.stderr);
    assert.equal(
      (JSON.parse(stopped.stdout) as { result: string }).result,
      "stopped",
    );
    assert.equal(
      existsSync(path.join(cwd, ".capstan/state/control.sock")),
      false,
    );
    assert.equal(
      existsSync(path.join(cwd, ".capstan/state/daemon.pid")),
      false,
    );
    assert.equal(
      (JSON.parse(invoke(cwd, "stop", "--json").stdout) as { result: string })
        .result,
      "not_running",
    );
  } finally {
    killDaemon(cwd);
    rmSync(cwd, { recursive: true, force: true });
  }
});

test(
  "the spawned daemon does not inherit the agent token or socket variables",
  { skip: process.platform !== "linux" },
  () => {
    const cwd = mkdtempSync(path.join(os.tmpdir(), "cstan-daemon-env-"));
    try {
      assert.equal(invoke(cwd, "init").status, 0);
      const result = invokeWithEnv(
        cwd,
        {
          CAPSTAN_TOKEN: "t".repeat(40),
          CAPSTAN_SOCKET: "/tmp/elsewhere.sock",
        },
        "start",
        "--json",
      );
      assert.equal(result.status, 0, result.stderr);
      const pid = daemonPid(cwd)!;
      const environment = readFileSync(`/proc/${pid}/environ`, "utf8");
      assert.ok(
        !environment.includes("CAPSTAN_TOKEN") &&
          !environment.includes("CAPSTAN_SOCKET"),
      );
      assert.ok(environment.includes("PATH="));
    } finally {
      killDaemon(cwd);
      rmSync(cwd, { recursive: true, force: true });
    }
  },
);

test("after kill -9 the next command restarts the daemon and reconciles without duplicating anything", async () => {
  const cwd = mkdtempSync(path.join(os.tmpdir(), "cstan-daemon-restart-"));
  try {
    assert.equal(invoke(cwd, "init").status, 0);
    const first = JSON.parse(invoke(cwd, "start", "--json").stdout) as {
      pid: number;
    };
    killDaemon(cwd);
    await waitGone(first.pid);

    const core = await openInitializedCore(cwd);
    const owner = readFileSync(
      path.join(cwd, ".capstan/operator.key"),
      "utf8",
    ).trim();
    const context = (): MutationContext => ({
      credential: owner,
      requestId: `req-${randomUUID()}`,
      idempotencyKey: `idem-${randomUUID()}`,
      expectedVersion: core.stateVersion,
      inputRevision: core.inputRevision,
    });
    core.syncRoleDefinitions(context(), [
      { name: "pm", kind: "PM", host: "claude", configHash: "a".repeat(64) },
      {
        name: "developer",
        kind: "Developer",
        host: "claude",
        configHash: "b".repeat(64),
      },
    ]);
    const seat = (name: string, role: "PM" | "Developer") => {
      core.createSeat(context(), { seatId: `${name}-seat`, name, role });
      return core.createActor(context(), {
        displayName: name,
        role,
        seatId: `${name}-seat`,
      });
    };
    const pmActor = seat("pm", "PM");
    core.registerAgent(context(), {
      agentId: "pm-agent",
      roleName: "pm",
      seatId: "pm-seat",
      actorId: pmActor.actorId,
    });
    const devActor = seat("developer", "Developer");
    core.registerAgent(context(), {
      agentId: "dev-agent",
      roleName: "developer",
      seatId: "developer-seat",
      actorId: devActor.actorId,
    });
    const queued = core.enqueueMessage(context(), {
      recipientAgentId: "dev-agent",
      body: "one",
    }).messageId;
    const sent = core.enqueueMessage(context(), {
      recipientAgentId: "pm-agent",
      body: "two",
    }).messageId;
    core.pullMessage({ ...context(), credential: pmActor.credential });
    core.recordAgentObservation(context(), "pm-agent", "working");
    const workerActor = seat("worker", "Developer");
    seedLedger(path.join(cwd, ".capstan/state"), (db) => {
      const projectId = (
        db.prepare("SELECT project_id FROM projects").get() as {
          project_id: string;
        }
      ).project_id;
      insertWorkItem(db, projectId, {
        workItemId: "work-1",
        title: "Old flow work",
        description: "in flight",
        state: "running",
      });
      insertAssignment(db, projectId, {
        assignmentId: "work-1-assignment",
        workItemId: "work-1",
        seatId: "worker-seat",
        workerActorId: workerActor.actorId,
        command: { commandId: "work-1-command" },
      });
    });
    core.close();
    const seeded = tableCounts(cwd);
    const messageRows = tableRows(cwd, "messages");
    const agentRows = [
      tableRows(cwd, "agents"),
      tableRows(cwd, "agent_state_history"),
      tableRows(cwd, "agent_waits"),
    ];
    assert.ok(messageRows.length === 2 && queued !== sent);

    const ping = invoke(cwd, "ping", "--json");
    assert.equal(ping.status, 0, ping.stderr);
    const second = JSON.parse(ping.stdout) as { pid: number };
    assert.notEqual(second.pid, first.pid);
    assert.equal(daemonPid(cwd), second.pid);

    const afterFirst = tableCounts(cwd);
    for (const table of [
      "assignments",
      "messages",
      "agents",
      "work_items",
      "actors",
      "seats",
    ])
      assert.equal(afterFirst[table], seeded[table], table);
    assert.deepEqual(tableRows(cwd, "messages"), messageRows);
    assert.deepEqual(
      [
        tableRows(cwd, "agents"),
        tableRows(cwd, "agent_state_history"),
        tableRows(cwd, "agent_waits"),
      ],
      agentRows,
    );
    const db = openSqlite(path.join(cwd, ".capstan/state/controller.sqlite"), {
      readOnly: true,
    });
    let versionAfterFirst: number;
    try {
      const assignment = db
        .prepare("SELECT state, authority_state FROM assignments")
        .get() as { state: string; authority_state: string };
      assert.deepEqual(assignment, {
        state: "revoked",
        authority_state: "unknown",
      });
      assert.equal(
        (db.prepare("SELECT state FROM commands").get() as { state: string })
          .state,
        "unknown",
      );
      assert.equal(
        (
          db
            .prepare(
              "SELECT state FROM work_items WHERE work_item_id = 'work-1'",
            )
            .get() as { state: string }
        ).state,
        "blocked",
      );
      assert.deepEqual(
        db
          .prepare("SELECT send_attempts FROM messages ORDER BY sequence")
          .all(),
        [{ send_attempts: 0 }, { send_attempts: 1 }],
      );
      assert.ok(
        afterFirst.controller_events! > seeded.controller_events!,
        "the reconcile wrote its own events",
      );
      versionAfterFirst = (
        db.prepare("SELECT state_version FROM projects").get() as {
          state_version: number;
        }
      ).state_version;
    } finally {
      db.close();
    }

    killDaemon(cwd);
    await waitGone(second.pid);
    const third = JSON.parse(invoke(cwd, "ping", "--json").stdout) as {
      pid: number;
    };
    assert.notEqual(third.pid, second.pid);
    assert.deepEqual(tableCounts(cwd), afterFirst);
    const check = openSqlite(
      path.join(cwd, ".capstan/state/controller.sqlite"),
      { readOnly: true },
    );
    try {
      assert.equal(
        (
          check.prepare("SELECT state_version FROM projects").get() as {
            state_version: number;
          }
        ).state_version,
        versionAfterFirst,
      );
    } finally {
      check.close();
    }
    assert.deepEqual(tableRows(cwd, "messages"), messageRows);
  } finally {
    killDaemon(cwd);
    rmSync(cwd, { recursive: true, force: true });
  }
});

test("agent mode uses the token and the socket from the environment, operator mode never uses the token", async () => {
  const cwd = mkdtempSync(path.join(os.tmpdir(), "cstan-daemon-agent-"));
  const elsewhere = mkdtempSync(
    path.join(os.tmpdir(), "cstan-daemon-nowhere-"),
  );
  try {
    assert.equal(invoke(cwd, "init").status, 0);
    const core = await openInitializedCore(cwd);
    const owner = readFileSync(
      path.join(cwd, ".capstan/operator.key"),
      "utf8",
    ).trim();
    const context = (): MutationContext => ({
      credential: owner,
      requestId: `req-${randomUUID()}`,
      idempotencyKey: `idem-${randomUUID()}`,
      expectedVersion: core.stateVersion,
      inputRevision: core.inputRevision,
    });
    core.syncRoleDefinitions(context(), [
      {
        name: "developer",
        kind: "Developer",
        host: "claude",
        configHash: "b".repeat(64),
      },
    ]);
    core.createSeat(context(), {
      seatId: "developer-seat",
      name: "developer",
      role: "Developer",
    });
    const actor = core.createActor(context(), {
      displayName: "developer",
      role: "Developer",
      seatId: "developer-seat",
    });
    core.registerAgent(context(), {
      agentId: "dev-agent",
      roleName: "developer",
      seatId: "developer-seat",
      actorId: actor.actorId,
    });
    core.close();

    const socketPath = path.join(cwd, ".capstan/state/control.sock");
    const agentEnv = {
      CAPSTAN_TOKEN: actor.credential,
      CAPSTAN_SOCKET: socketPath,
    };
    const dead = invokeWithEnv(elsewhere, agentEnv, "inbox");
    assert.equal(dead.status, 4);
    assert.match(
      dead.stderr,
      /the controller is not running; ask the operator to run cstan start/,
    );

    assert.equal(invoke(cwd, "start").status, 0);
    const status = invokeWithEnv(elsewhere, agentEnv, "status", "--json");
    assert.equal(status.status, 0, status.stderr);
    const snapshot = JSON.parse(status.stdout) as {
      agents: Array<{ agentId: string }>;
    };
    assert.deepEqual(
      snapshot.agents.map((agent) => agent.agentId),
      ["dev-agent"],
    );
    assert.ok(!status.stdout.includes(actor.credential));
    const inbox = invokeWithEnv(elsewhere, agentEnv, "inbox");
    assert.equal(inbox.status, 0, inbox.stderr);
    assert.equal(inbox.stdout.trim(), "no messages");
    assert.equal(invokeWithEnv(elsewhere, agentEnv, "ping").status, 0);

    const noProject = invokeWithEnv(elsewhere, agentEnv, "assign", "hello");
    assert.equal(noProject.status, 3);
    assert.match(
      noProject.stderr,
      /operator commands need the operator credential/,
    );
    const fromProject = invokeWithEnv(cwd, agentEnv, "assign", "hello");
    assert.equal(fromProject.status, 4);
    assert.match(
      fromProject.stderr,
      /not_implemented/,
      "the operator credential was used, not the token",
    );
    assert.ok(!fromProject.stderr.includes("forbidden"));

    const operatorAgentCommand = invoke(cwd, "ack", "message-1");
    assert.equal(operatorAgentCommand.status, 3);
    assert.match(operatorAgentCommand.stderr, /must be run by an agent/);
  } finally {
    killDaemon(cwd);
    rmSync(cwd, { recursive: true, force: true });
    rmSync(elsewhere, { recursive: true, force: true });
  }
});

test("the message commands work end to end through the executable: send, inbox, wait, ack, resolve and cancel", async () => {
  const cwd = mkdtempSync(path.join(os.tmpdir(), "cstan-daemon-messages-"));
  const elsewhere = mkdtempSync(path.join(os.tmpdir(), "cstan-daemon-away-"));
  try {
    assert.equal(invoke(cwd, "init").status, 0);
    const core = await openInitializedCore(cwd);
    const owner = readFileSync(
      path.join(cwd, ".capstan/operator.key"),
      "utf8",
    ).trim();
    const context = (): MutationContext => ({
      credential: owner,
      requestId: `req-${randomUUID()}`,
      idempotencyKey: `idem-${randomUUID()}`,
      expectedVersion: core.stateVersion,
      inputRevision: core.inputRevision,
    });
    core.syncRoleDefinitions(context(), [
      { name: "pm", kind: "PM", host: "claude", configHash: "a".repeat(64) },
      {
        name: "developer",
        kind: "Developer",
        host: "claude",
        configHash: "b".repeat(64),
      },
    ]);
    const member = (name: string, kind: "PM" | "Developer") => {
      const seatId = `${name}-seat`;
      core.createSeat(context(), { seatId, name, role: kind });
      const actor = core.createActor(context(), {
        displayName: name,
        role: kind,
        seatId,
      });
      core.registerAgent(context(), {
        agentId: `${name}-agent`,
        roleName: name,
        seatId,
        actorId: actor.actorId,
      });
      return actor.credential;
    };
    const pmToken = member("pm", "PM");
    const devToken = member("developer", "Developer");
    core.close();

    const socketPath = path.join(cwd, ".capstan/state/control.sock");
    const env = (token: string) => ({
      CAPSTAN_TOKEN: token,
      CAPSTAN_SOCKET: socketPath,
    });
    assert.equal(invoke(cwd, "start").status, 0);

    const sent = invoke(
      cwd,
      "send",
      "--json",
      "@pm",
      "hello from the operator",
    );
    assert.equal(sent.status, 0, sent.stderr);
    const { messageId } = JSON.parse(sent.stdout) as { messageId: string };

    const inbox = invokeWithEnv(elsewhere, env(pmToken), "inbox");
    assert.equal(inbox.status, 0, inbox.stderr);
    assert.match(
      inbox.stdout,
      new RegExp(`message ${messageId} \\[sent\\] from operator`),
    );
    assert.match(inbox.stdout, /hello from the operator/);

    const peek = invoke(cwd, "inbox", "--json", "pm-agent");
    assert.equal(peek.status, 0, peek.stderr);
    assert.equal(
      (JSON.parse(peek.stdout) as { messages: Array<{ state: string }> })
        .messages[0]!.state,
      "sent",
    );

    const acked = invokeWithEnv(
      elsewhere,
      env(pmToken),
      "ack",
      "--json",
      messageId,
    );
    assert.equal(acked.status, 0, acked.stderr);
    assert.equal(
      (JSON.parse(acked.stdout) as { state: string }).state,
      "acked",
    );
    assert.equal(
      invokeWithEnv(elsewhere, env(pmToken), "inbox").stdout.trim(),
      "no messages",
    );

    const fromDeveloper = invokeWithEnv(
      elsewhere,
      env(devToken),
      "send",
      "--json",
      "@pm",
      "done",
    );
    assert.equal(fromDeveloper.status, 0, fromDeveloper.stderr);
    const reply = (JSON.parse(fromDeveloper.stdout) as { messageId: string })
      .messageId;
    const waited = invokeWithEnv(elsewhere, env(pmToken), "wait");
    assert.equal(waited.status, 0, waited.stderr);
    assert.match(waited.stdout, /from developer \(developer-agent\)/);
    assert.match(waited.stdout, /done/);

    const retried = invoke(cwd, "resolve", "--json", reply, "retry");
    assert.equal(retried.status, 0, retried.stderr);
    assert.match(
      retried.stderr,
      /warning: the recipient may already have received/,
    );
    const cancelled = invoke(cwd, "cancel", "--json", reply);
    assert.equal(cancelled.status, 0, cancelled.stderr);
    assert.equal(
      (JSON.parse(cancelled.stdout) as { state: string }).state,
      "cancelled",
    );

    const workerToWorker = invokeWithEnv(
      elsewhere,
      env(devToken),
      "send",
      "developer-agent",
      "x",
    );
    assert.equal(workerToWorker.status, 4);
    assert.match(workerToWorker.stderr, /self_send/);
    for (const bad of ["0", "61", " 5", "1e1", "0x3", "5.0", "-1", "05"]) {
      const status = invoke(cwd, "status", "--watch", "--interval", bad);
      assert.equal(status.status, 3, JSON.stringify(bad));
      assert.match(status.stderr, /--interval must be an integer from 1 to 60/);
    }
    const replacement = invoke(cwd, "send", "@pm", "broken \ufffd text");
    assert.equal(replacement.status, 3);
    assert.match(replacement.stderr, /replacement character/);
    const literal = invoke(cwd, "status", "--", "--watch");
    assert.equal(
      literal.status,
      2,
      "a --watch after -- is not the watch option",
    );
  } finally {
    killDaemon(cwd);
    rmSync(cwd, { recursive: true, force: true });
    rmSync(elsewhere, { recursive: true, force: true });
  }
});

test("a broken capstan.toml stops the daemon at start with the loader's message", () => {
  const cwd = mkdtempSync(path.join(os.tmpdir(), "cstan-daemon-badconfig-"));
  try {
    assert.equal(invoke(cwd, "init").status, 0);
    writeFileSync(path.join(cwd, "capstan.toml"), "schema_version = 2\n", {
      mode: 0o600,
    });
    const result = invoke(cwd, "start");
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, /schema_version/);
    assert.equal(daemonPid(cwd), undefined, "no daemon is left running");
  } finally {
    killDaemon(cwd);
    rmSync(cwd, { recursive: true, force: true });
  }
});

test("the CLI behaves the same when it is started through a symlink, as npm link and a global install do", () => {
  const cwd = mkdtempSync(path.join(os.tmpdir(), "cstan-symlink-"));
  try {
    const link = path.join(cwd, "cstan");
    symlinkSync(cli, link);
    const project = path.join(cwd, "project");
    mkdirSync(project);
    assert.equal(
      spawnSync("git", ["init", "-q", "."], { cwd: project }).status,
      0,
    );
    const init = spawnSync(process.execPath, [link, "init"], {
      cwd: project,
      encoding: "utf8",
      env: { ...process.env, CAPSTAN_LAUNCH: "off" },
    });
    assert.equal(init.status, 0, init.stderr);
    assert.match(init.stdout, /Initialized Capstan project/);
    assert.ok(existsSync(path.join(project, ".capstan/operator.key")));
    const ping = spawnSync(process.execPath, [link, "ping"], {
      cwd: project,
      encoding: "utf8",
      env: { ...process.env, CAPSTAN_LAUNCH: "off" },
    });
    assert.equal(ping.status, 0, ping.stderr);
    assert.match(ping.stdout, /^pong: true$/m);
    const unknown = spawnSync(process.execPath, [link, "no-such-command"], {
      cwd: project,
      encoding: "utf8",
      env: { ...process.env, CAPSTAN_LAUNCH: "off" },
    });
    assert.notEqual(
      unknown.status,
      0,
      "an invalid command is reported, not ignored",
    );
    assert.match(
      unknown.stderr,
      /usage: cstan init/,
      "the CLI ran and printed its usage",
    );
    const extensionless = spawnSync(
      process.execPath,
      [cli.replace(/\.js$/, ""), "ping"],
      {
        cwd: project,
        encoding: "utf8",
        env: { ...process.env, CAPSTAN_LAUNCH: "off" },
      },
    );
    assert.equal(extensionless.status, 0, extensionless.stderr);
    assert.match(
      extensionless.stdout,
      /^pong: true$/m,
      "node dist/src/cli works without the extension",
    );
  } finally {
    killDaemon(path.join(cwd, "project"));
    rmSync(cwd, { recursive: true, force: true });
  }
});

test("cancel with one id is a routed command; the legacy forms keep their usage rules", () => {
  const cwd = mkdtempSync(path.join(os.tmpdir(), "cstan-daemon-cancel-"));
  try {
    assert.equal(invoke(cwd, "init").status, 0);
    for (const args of [["cancel", "a", "b"]]) {
      const result = invoke(cwd, ...args);
      assert.equal(result.status, 2, args.join(" "));
      assert.match(result.stderr, /usage: cstan init \| cstan start/);
    }
    assert.equal(
      existsSync(path.join(cwd, ".capstan/state/daemon.pid")),
      false,
      "usage errors never start the daemon",
    );
    const routed = invoke(cwd, "cancel", "--json", "work-1");
    assert.equal(routed.status, 4, routed.stderr);
    assert.match(routed.stderr, /rejected: unknown_message/);
    assert.ok(daemonPid(cwd) !== undefined);
    const bare = invoke(cwd, "cancel");
    assert.equal(bare.status, 2);
  } finally {
    killDaemon(cwd);
    rmSync(cwd, { recursive: true, force: true });
  }
});

test("a daemon that dies during startup is reported at once with the log tail", () => {
  const cwd = mkdtempSync(path.join(os.tmpdir(), "cstan-daemon-fail-"));
  try {
    assert.equal(invoke(cwd, "init").status, 0);
    writeFileSync(
      path.join(cwd, ".capstan/state/control.sock"),
      "not a socket",
      { mode: 0o600 },
    );
    const began = Date.now();
    const result = invoke(cwd, "start");
    assert.equal(result.status, 5, result.stderr);
    assert.match(
      result.stderr,
      /the daemon exited during startup \(exit code \d+\)/,
    );
    assert.match(
      result.stderr,
      /control socket path exists and is not a socket/,
    );
    assert.ok(Date.now() - began < 8000, "did not wait for the full timeout");
    assert.equal(daemonPid(cwd), undefined);
    assert.equal(
      readFileSync(path.join(cwd, ".capstan/state/control.sock"), "utf8"),
      "not a socket",
    );
  } finally {
    killDaemon(cwd);
    rmSync(cwd, { recursive: true, force: true });
  }
});

test("two cstan start commands at once end with one daemon and both succeed", async () => {
  const cwd = mkdtempSync(path.join(os.tmpdir(), "cstan-daemon-race-"));
  try {
    assert.equal(invoke(cwd, "init").status, 0);
    const [left, right] = await Promise.all([
      invokeAsync(cwd, "start", "--json"),
      invokeAsync(cwd, "start", "--json"),
    ]);
    assert.equal(left.status, 0, left.stderr);
    assert.equal(right.status, 0, right.stderr);
    const results = [left, right].map(
      (run) => JSON.parse(run.stdout) as { pid: number; started: boolean },
    );
    assert.equal(results[0]!.pid, results[1]!.pid);
    assert.equal(results[0]!.pid, daemonPid(cwd));
    assert.ok(
      results.some((result) => result.started),
      "one of them started the daemon",
    );
  } finally {
    killDaemon(cwd);
    rmSync(cwd, { recursive: true, force: true });
  }
});

test("SIGTERM stops the daemon cleanly and the next start needs no recovery", async () => {
  const cwd = mkdtempSync(path.join(os.tmpdir(), "cstan-daemon-term-"));
  try {
    assert.equal(invoke(cwd, "init").status, 0);
    const first = JSON.parse(invoke(cwd, "start", "--json").stdout) as {
      pid: number;
    };
    process.kill(first.pid, "SIGTERM");
    await waitGone(first.pid);
    assert.equal(
      existsSync(path.join(cwd, ".capstan/state/control.sock")),
      false,
    );
    assert.equal(
      existsSync(path.join(cwd, ".capstan/state/daemon.pid")),
      false,
    );
    const second = JSON.parse(invoke(cwd, "start", "--json").stdout) as {
      pid: number;
      started: boolean;
    };
    assert.equal(second.started, true);
    assert.notEqual(second.pid, first.pid);
  } finally {
    killDaemon(cwd);
    rmSync(cwd, { recursive: true, force: true });
  }
});

test("the pm-restart alias is not a command, and daemon without a project explains what is missing", () => {
  const cwd = mkdtempSync(path.join(os.tmpdir(), "cstan-daemon-alias-"));
  const empty = mkdtempSync(path.join(os.tmpdir(), "cstan-daemon-empty-"));
  try {
    assert.equal(invoke(cwd, "init").status, 0);
    const alias = invoke(cwd, "pm-restart");
    assert.equal(alias.status, 2);
    assert.equal(daemonPid(cwd), undefined);
    const noProject = invoke(empty, "daemon");
    assert.equal(noProject.status, 3);
    assert.match(
      noProject.stderr,
      /operator commands need the operator credential in \.capstan/,
    );
  } finally {
    killDaemon(cwd);
    rmSync(cwd, { recursive: true, force: true });
    rmSync(empty, { recursive: true, force: true });
  }
});

test("every line of daemon.log is JSON, and a partial or invalid agent environment is named", () => {
  const cwd = mkdtempSync(path.join(os.tmpdir(), "cstan-daemon-logfmt-"));
  try {
    assert.equal(invoke(cwd, "init").status, 0);
    writeFileSync(path.join(cwd, ".nexora.toml"), "");
    assert.equal(invoke(cwd, "start").status, 0);
    assert.equal(invoke(cwd, "ping").status, 0);
    const lines = readFileSync(path.join(cwd, ".capstan/daemon.log"), "utf8")
      .trim()
      .split("\n");
    assert.ok(lines.length >= 2);
    for (const line of lines) {
      const entry = JSON.parse(line) as { ts: string };
      assert.match(entry.ts, /^\d{4}-\d{2}-\d{2}T/);
    }
    assert.deepEqual(JSON.parse(lines[0]!), {
      ts: JSON.parse(lines[0]!).ts,
      event: "ready",
      pid: daemonPid(cwd),
    });

    const partial = invokeWithEnv(
      cwd,
      { CAPSTAN_TOKEN: "t".repeat(40) },
      "status",
    );
    assert.equal(partial.status, 3);
    assert.match(
      partial.stderr,
      /CAPSTAN_TOKEN and CAPSTAN_SOCKET must both be set/,
    );
    const relative = invokeWithEnv(
      cwd,
      {
        CAPSTAN_TOKEN: "t".repeat(40),
        CAPSTAN_SOCKET: "relative.sock",
      },
      "ping",
    );
    assert.equal(relative.status, 3);
    assert.match(relative.stderr, /absolute path/);
    const emptyToken = invokeWithEnv(
      cwd,
      {
        CAPSTAN_TOKEN: "",
        CAPSTAN_SOCKET: "/tmp/x.sock",
      },
      "inbox",
    );
    assert.equal(emptyToken.status, 3);
    assert.match(emptyToken.stderr, /must both be set/);
    const operatorUnaffected = invokeWithEnv(
      cwd,
      { CAPSTAN_TOKEN: "t".repeat(40) },
      "assign",
      "x",
    );
    assert.equal(operatorUnaffected.status, 4);
    assert.match(operatorUnaffected.stderr, /not_implemented/);
  } finally {
    killDaemon(cwd);
    rmSync(cwd, { recursive: true, force: true });
  }
});

test("two termination signals in a row still leave no socket or pid file", async () => {
  const cwd = mkdtempSync(path.join(os.tmpdir(), "cstan-daemon-twice-"));
  try {
    assert.equal(invoke(cwd, "init").status, 0);
    const started = JSON.parse(invoke(cwd, "start", "--json").stdout) as {
      pid: number;
    };
    process.kill(started.pid, "SIGTERM");
    process.kill(started.pid, "SIGINT");
    await waitGone(started.pid);
    assert.equal(
      existsSync(path.join(cwd, ".capstan/state/control.sock")),
      false,
    );
    assert.equal(
      existsSync(path.join(cwd, ".capstan/state/daemon.pid")),
      false,
    );
  } finally {
    killDaemon(cwd);
    rmSync(cwd, { recursive: true, force: true });
  }
});

test("routed arguments keep a literal --json after --, empty arguments are refused and a bad token is named", () => {
  const cwd = mkdtempSync(path.join(os.tmpdir(), "cstan-daemon-args-"));
  try {
    assert.equal(invoke(cwd, "init").status, 0);
    assert.equal(invoke(cwd, "start").status, 0);
    const literal = invoke(cwd, "assign", "--", "--json");
    assert.equal(literal.status, 4);
    assert.match(
      literal.stderr,
      /not_implemented/,
      "the text was not treated as --json output",
    );
    const entries = readFileSync(path.join(cwd, ".capstan/daemon.log"), "utf8")
      .trim()
      .split("\n")
      .map(
        (line) =>
          JSON.parse(line) as {
            command?: string;
            argCount?: number;
            argBytes?: number;
          },
      );
    const sendEntry = entries
      .filter((entry) => entry.command === "assign")
      .at(-1)!;
    assert.deepEqual(
      [sendEntry.argCount, sendEntry.argBytes],
      [1, "--json".length],
    );
    const asOption = invoke(cwd, "assign", "hello", "--json");
    const optionEntry = readFileSync(
      path.join(cwd, ".capstan/daemon.log"),
      "utf8",
    )
      .trim()
      .split("\n")
      .map(
        (line) => JSON.parse(line) as { command?: string; argCount?: number },
      )
      .filter((entry) => entry.command === "assign")
      .at(-1)!;
    assert.equal(asOption.status, 4);
    assert.equal(
      optionEntry.argCount,
      1,
      "--json before the separator is an option",
    );

    for (const args of [
      ["cancel", ""],
      ["assign", ""],
    ]) {
      const result = invoke(cwd, ...args);
      assert.equal(result.status, 3, args.join(" "));
      assert.match(result.stderr, /command arguments must not be empty/);
    }
    for (const token of [
      " ",
      "abc def",
      `${"t".repeat(40)}\n`,
      `${"t".repeat(40)}\r`,
    ]) {
      const result = invokeWithEnv(
        cwd,
        {
          CAPSTAN_TOKEN: token,
          CAPSTAN_SOCKET: path.join(cwd, ".capstan/state/control.sock"),
        },
        "inbox",
      );
      assert.equal(result.status, 3, JSON.stringify(token));
      assert.match(
        result.stderr,
        /CAPSTAN_TOKEN must not contain whitespace or control characters/,
      );
    }
  } finally {
    killDaemon(cwd);
    rmSync(cwd, { recursive: true, force: true });
  }
});

test("a failed start shows only this start's log lines, without control characters", () => {
  const cwd = mkdtempSync(path.join(os.tmpdir(), "cstan-daemon-tail-"));
  try {
    assert.equal(invoke(cwd, "init").status, 0);
    writeFileSync(
      path.join(cwd, ".capstan/daemon.log"),
      "OLD-RUN-LINE from an earlier start\n\u001b[31mOLD-ANSI\u001b[0m\n",
      { mode: 0o600 },
    );
    writeFileSync(
      path.join(cwd, ".capstan/state/control.sock"),
      "not a socket",
      { mode: 0o600 },
    );
    const result = invoke(cwd, "start");
    assert.equal(result.status, 5, result.stderr);
    assert.match(
      result.stderr,
      /control socket path exists and is not a socket/,
    );
    assert.ok(
      !result.stderr.includes("OLD-RUN-LINE") &&
        !result.stderr.includes("OLD-ANSI"),
    );
    assert.ok(
      !/[\u0000-\u0009\u000b-\u001f]/.test(result.stderr.replace(/\n$/, "")),
    );
  } finally {
    killDaemon(cwd);
    rmSync(cwd, { recursive: true, force: true });
  }
});

test("stop followed at once by start always gets a fresh daemon because the lock is free when stop returns", () => {
  const cwd = mkdtempSync(path.join(os.tmpdir(), "cstan-daemon-cycle-"));
  try {
    assert.equal(invoke(cwd, "init").status, 0);
    const pids = new Set<number>();
    for (let round = 0; round < 5; round += 1) {
      const started = invoke(cwd, "start", "--json");
      assert.equal(started.status, 0, `round ${round}: ${started.stderr}`);
      const result = JSON.parse(started.stdout) as {
        pid: number;
        started: boolean;
      };
      assert.equal(result.started, true, `round ${round}`);
      pids.add(result.pid);
      const stopped = invoke(cwd, "stop", "--json");
      assert.equal(stopped.status, 0, stopped.stderr);
      assert.equal(
        (JSON.parse(stopped.stdout) as { result: string }).result,
        "stopped",
      );
    }
    assert.equal(pids.size, 5);
  } finally {
    killDaemon(cwd);
    rmSync(cwd, { recursive: true, force: true });
  }
});

test("a project lock held by something that never answers fails a start within the grace period", async () => {
  const cwd = mkdtempSync(path.join(os.tmpdir(), "cstan-daemon-lockheld-"));
  try {
    assert.equal(invoke(cwd, "init").status, 0);
    const holder = await openInitializedCore(cwd);
    try {
      const began = Date.now();
      const result = await invokeAsync(cwd, "start");
      assert.equal(result.status, 5, result.stderr);
      assert.match(result.stderr, /holds the project lock but does not answer/);
      assert.ok(Date.now() - began < 9000, "did not wait for the full timeout");
    } finally {
      holder.close();
    }
  } finally {
    killDaemon(cwd);
    rmSync(cwd, { recursive: true, force: true });
  }
});

test("a restrictive umask does not stop a start, and the fresh log is still mode 0600", () => {
  const cwd = mkdtempSync(path.join(os.tmpdir(), "cstan-daemon-umask-"));
  try {
    assert.equal(invoke(cwd, "init").status, 0);
    ensureCommit(cwd, ["start"]);
    const result = spawnSync(
      "sh",
      ["-c", `umask 0277 && exec "${process.execPath}" "${cli}" start --json`],
      {
        cwd,
        encoding: "utf8",
      },
    );
    assert.equal(result.status, 0, result.stderr);
    assert.equal(
      statSync(path.join(cwd, ".capstan/daemon.log")).mode & 0o777,
      0o600,
    );
  } finally {
    killDaemon(cwd);
    rmSync(cwd, { recursive: true, force: true });
  }
});

test("a bad CAPSTAN_SOCKET is named and cstan pm restart accepts --json in either position", () => {
  const cwd = mkdtempSync(path.join(os.tmpdir(), "cstan-daemon-pm-"));
  try {
    assert.equal(invoke(cwd, "init").status, 0);
    for (const socket of ["/tmp/a.sock\n", "/tmp/a b.sock", "/tmp/a.sock\r"]) {
      const result = invokeWithEnv(
        cwd,
        {
          CAPSTAN_TOKEN: "t".repeat(40),
          CAPSTAN_SOCKET: socket,
        },
        "inbox",
      );
      assert.equal(result.status, 3, JSON.stringify(socket));
      assert.match(
        result.stderr,
        /CAPSTAN_SOCKET must not contain whitespace or control characters/,
      );
    }
    for (const args of [
      ["pm", "restart", "--json"],
      ["pm", "--json", "restart"],
      ["pm", "restart"],
    ]) {
      const result = invoke(cwd, ...args);
      assert.equal(result.status, 4, `${args.join(" ")}: ${result.stderr}`);
      assert.match(
        result.stderr,
        /not_configured: restarting the PM needs capstan.toml and Herdr/,
      );
    }
    assert.equal(invoke(cwd, "pm").status, 2);
    assert.equal(invoke(cwd, "pm", "--json").status, 2);
    assert.equal(
      invoke(cwd, "pm", "--", "restart").status,
      2,
      "-- ends option parsing, so this is not the restart subcommand",
    );
  } finally {
    killDaemon(cwd);
    rmSync(cwd, { recursive: true, force: true });
  }
});

test("shutdown releases the project lock before it removes the socket", () => {
  const cwd = mkdtempSync(path.join(os.tmpdir(), "cstan-daemon-order-"));
  try {
    assert.equal(invoke(cwd, "init").status, 0);
    assert.equal(invoke(cwd, "start").status, 0);
    assert.equal(invoke(cwd, "stop").status, 0);
    const events = readFileSync(path.join(cwd, ".capstan/daemon.log"), "utf8")
      .trim()
      .split("\n")
      .map((line) => (JSON.parse(line) as { event?: string }).event)
      .filter((event): event is string => event !== undefined);
    assert.deepEqual(events, ["ready", "lock_released", "socket_removed"]);
  } finally {
    killDaemon(cwd);
    rmSync(cwd, { recursive: true, force: true });
  }
});

test("the usage line lists the plan subcommands", () => {
  const cwd = mkdtempSync(path.join(os.tmpdir(), "cstan-plan-usage-"));
  try {
    const result = invoke(cwd, "no-such-command");
    assert.match(result.stderr, /cstan plan open normal\|high-risk <title>/);
    assert.match(result.stderr, /cstan plan submit <plan-id> <json>/);
    assert.match(result.stderr, /cstan plan show \[<plan-id>\]/);
  } finally {
    rmSync(cwd, { recursive: true, force: true });
  }
});

test("inbox --hook prints nothing and exits 0 without the agent environment, with an unreachable daemon, and at count 0; it prints PostToolUse JSON at count>0", async () => {
  const dir = mkdtempSync(path.join(os.tmpdir(), "capstan-hook-"));
  const socketPath = path.join(dir, "hook.sock");
  let reply = {
    ok: true,
    result: { count: 0, oldestQueuedAt: null, messageIds: [] },
  } as unknown;
  const requests: string[] = [];
  const server = net.createServer((socket) => {
    socket.on("data", (chunk) => {
      requests.push(chunk.toString("utf8"));
      socket.end(`${JSON.stringify(reply)}\n`);
    });
  });
  await new Promise<void>((resolve) => server.listen(socketPath, resolve));
  try {
    const bare = invokeWithEnv(dir, {}, "inbox", "--hook");
    assert.equal(bare.status, 0, bare.stderr);
    assert.equal(bare.stdout, "");
    const env = {
      CAPSTAN_TOKEN: "token-1",
      CAPSTAN_SOCKET: socketPath,
      CAPSTAN_AGENT_ID: "developer-1",
    };
    for (const missing of Object.keys(env)) {
      const partial: NodeJS.ProcessEnv = { ...env, [missing]: undefined };
      const run = await invokeAsyncWithEnv(dir, partial, "inbox", "--hook");
      assert.equal(run.status, 0, run.stderr);
      assert.equal(run.stdout, "", `${missing} missing`);
    }
    assert.equal(requests.length, 0, "no call without the full environment");
    const empty = await invokeAsyncWithEnv(dir, env, "inbox", "--hook");
    assert.equal(empty.status, 0, empty.stderr);
    assert.equal(empty.stdout, "");
    assert.match(requests[0]!, /"command":"inbox","args":\["--hook"\]/);
    reply = {
      ok: true,
      result: {
        count: 2,
        oldestQueuedAt: new Date(Date.now() - 5 * 60_000).toISOString(),
        messageIds: ["a", "b"],
      },
    };
    const waiting = await invokeAsyncWithEnv(dir, env, "inbox", "--hook");
    assert.equal(waiting.status, 0, waiting.stderr);
    const hook = JSON.parse(waiting.stdout) as {
      hookSpecificOutput: { hookEventName: string; additionalContext: string };
    };
    assert.equal(hook.hookSpecificOutput.hookEventName, "PostToolUse");
    assert.equal(
      hook.hookSpecificOutput.additionalContext,
      "2 Capstan message(s) are waiting for you (oldest 5 min). Run cstan inbox now and ack each one before you continue or report.",
    );
  } finally {
    server.close();
    const gone = invokeWithEnv(
      dir,
      {
        CAPSTAN_TOKEN: "token-1",
        CAPSTAN_SOCKET: path.join(dir, "none.sock"),
        CAPSTAN_AGENT_ID: "developer-1",
      },
      "inbox",
      "--hook",
    );
    assert.equal(gone.status, 0, "an unreachable daemon exits 0");
    assert.equal(gone.stdout, "");
    rmSync(dir, { recursive: true, force: true });
  }
});

test("an agent command prints the unread notice with the action count, inbox marks action-needed frames and send --action reaches the daemon", async () => {
  const dir = mkdtempSync(path.join(os.tmpdir(), "capstan-notice-"));
  const socketPath = path.join(dir, "notice.sock");
  let reply = { ok: true, result: {} } as unknown;
  const requests: string[] = [];
  const server = net.createServer((socket) => {
    socket.on("data", (chunk) => {
      requests.push(chunk.toString("utf8"));
      socket.end(`${JSON.stringify(reply)}\n`);
    });
  });
  await new Promise<void>((resolve) => server.listen(socketPath, resolve));
  try {
    const env = {
      CAPSTAN_TOKEN: "token-1",
      CAPSTAN_SOCKET: socketPath,
      CAPSTAN_AGENT_ID: "pm-1",
    };
    const oldest = new Date(Date.now() - 7 * 60_000).toISOString();
    reply = {
      ok: true,
      result: {
        messageId: "m-9",
        state: "acked",
        unread: { count: 2, oldestQueuedAt: oldest, actionNeeded: 1 },
      },
    };
    const noisy = await invokeAsyncWithEnv(dir, env, "ack", "m-9");
    assert.equal(noisy.status, 0, noisy.stderr);
    assert.match(
      noisy.stderr,
      /notice: 2 message\(s\) wait for you \(oldest 7 min\): run cstan inbox, 1 need action\n/,
    );
    reply = {
      ok: true,
      result: {
        messageId: "m-9",
        state: "acked",
        unread: { count: 1, oldestQueuedAt: oldest, actionNeeded: 0 },
      },
    };
    const plain = await invokeAsyncWithEnv(dir, env, "ack", "m-9");
    assert.match(plain.stderr, /run cstan inbox\n/);
    assert.doesNotMatch(plain.stderr, /need action/);
    reply = { ok: true, result: { messageId: "m-9", state: "acked" } };
    const quiet = await invokeAsyncWithEnv(dir, env, "ack", "m-9");
    assert.doesNotMatch(quiet.stderr, /notice:/);

    reply = {
      ok: true,
      result: {
        count: 2,
        actionNeededCount: 1,
        messages: [
          {
            messageId: "m-1",
            state: "sent",
            from: "controller",
            fromAgentId: "controller",
            body: "first",
          },
          {
            messageId: "m-2",
            state: "sent",
            from: "controller",
            fromAgentId: "controller",
            body: "Delivery problem: x",
            actionNeeded: true,
          },
        ],
      },
    };
    const inbox = await invokeAsyncWithEnv(dir, env, "inbox");
    assert.equal(inbox.status, 0, inbox.stderr);
    assert.ok(
      inbox.stdout.indexOf("message m-1") <
        inbox.stdout.indexOf("[ACTION NEEDED]\nmessage m-2"),
      "sequence order is kept and only the second frame is marked",
    );
    assert.equal(inbox.stdout.split("[ACTION NEEDED]").length - 1, 1);

    reply = { ok: true, result: { messageId: "m-3" } };
    const sent = await invokeAsyncWithEnv(
      dir,
      env,
      "send",
      "--action",
      "developer-1",
      "please decide",
    );
    assert.equal(sent.status, 0, sent.stderr);
    assert.match(
      requests.at(-1)!,
      /"command":"send","args":\["--action","developer-1","please decide"\]/,
    );
  } finally {
    server.close();
    rmSync(dir, { recursive: true, force: true });
  }
});

function invokeAsyncWithEnv(
  cwd: string,
  env: NodeJS.ProcessEnv,
  ...args: string[]
): Promise<{ status: number | null; stdout: string; stderr: string }> {
  ensureCommit(cwd, args);
  return new Promise((resolve, reject) => {
    const child = spawn(process.execPath, [cli, ...args], {
      cwd,
      env: { ...process.env, ...env },
    });
    let stdout = "";
    let stderr = "";
    child.stdout.on("data", (chunk) => (stdout += String(chunk)));
    child.stderr.on("data", (chunk) => (stderr += String(chunk)));
    child.on("error", reject);
    child.on("close", (status) => resolve({ status, stdout, stderr }));
  });
}

test("no ExperimentalWarning from node:sqlite reaches stderr from init or status", () => {
  const cwd = mkdtempSync(path.join(os.tmpdir(), "cstan-no-warning-"));
  try {
    const git = spawnSync("git", ["init", "--quiet"], { cwd });
    assert.equal(git.status, 0);
    const init = invoke(cwd, "init");
    assert.equal(init.status, 0, init.stderr);
    assert.doesNotMatch(
      init.stderr,
      /ExperimentalWarning|SQLite is an experimental/,
    );
    const status = invoke(cwd, "status");
    assert.doesNotMatch(
      status.stderr,
      /ExperimentalWarning|SQLite is an experimental/,
    );
  } finally {
    rmSync(cwd, { recursive: true, force: true });
  }
});

test("text status prints the PM mail line: STALE once when stale, the plain pending line otherwise", () => {
  const stale = pmMailLines({
    pmMail: {
      pending: 2,
      oldestAgeSeconds: 725,
      oldestMessageId: "m-1",
      stale: true,
    },
  });
  assert.deepEqual(stale, [
    "PM MAIL STALE: 2 message(s) pending, oldest 12 min (m-1)",
  ]);
  assert.deepEqual(
    pmMailLines({ pmMail: { pending: 1, oldestAgeSeconds: 60, stale: false } }),
    ["pm mail: 1 pending, oldest 1 min"],
  );
  assert.deepEqual(pmMailLines({ pmMail: null }), []);
  assert.deepEqual(pmMailLines({}), []);
});
