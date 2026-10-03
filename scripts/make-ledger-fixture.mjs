// Generates test/fixtures/ledger-better-sqlite3.sqlite: a v31 controller ledger written by the
// better-sqlite3 build, so test/ledger-compat.test.ts can prove the node:sqlite adapter reads it.
// It needs a PRE-SWAP checkout (better-sqlite3 installed and src still importing it) and a built
// dist/ (npm run build). Re-run only from such a checkout: node scripts/make-ledger-fixture.mjs
import {
  mkdtempSync,
  rmSync,
  copyFileSync,
  mkdirSync,
  existsSync,
} from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import Database from "better-sqlite3";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const { ControllerCore } = await import(
  path.join(root, "dist/src/controller/core.js")
);

const temp = mkdtempSync(path.join(tmpdir(), "capstan-fixture-"));
const stateDirectory = path.join(temp, "state");
mkdirSync(stateDirectory, { mode: 0o700 });
try {
  const id = `p${crypto.randomUUID().replaceAll("-", "")}`;
  const owner = `owner-${crypto.randomUUID().replaceAll("-", "")}`;
  const kinds = [
    "project_config",
    "task_brief",
    "acceptance_criteria",
    "policy",
    "plan",
  ];
  const core = await ControllerCore.open({
    stateDirectory,
    project: {
      projectId: id,
      name: "Ledger fixture project",
      ownerCredential: owner,
      initialInputs: kinds.map((kind) => ({
        kind,
        content:
          kind === "acceptance_criteria"
            ? ["criterion"]
            : { kind, revision: 1 },
      })),
    },
  });
  const ctx = (credential) => {
    const n = crypto.randomUUID();
    return {
      credential,
      requestId: `req-${n}`,
      idempotencyKey: `idem-${n}`,
      expectedVersion: core.stateVersion,
      inputRevision: core.inputRevision,
    };
  };
  core.syncRoleDefinitions(ctx(owner), [
    { name: "pm", kind: "PM", host: "claude", configHash: "a".repeat(64) },
    {
      name: "developer",
      kind: "Developer",
      host: "claude",
      configHash: "b".repeat(64),
    },
  ]);
  const member = (name, kind) => {
    const seatId = `${name}-seat`;
    core.createSeat(ctx(owner), { seatId, name, role: kind });
    const actor = core.createActor(ctx(owner), {
      displayName: name,
      role: kind,
      seatId,
    });
    core.registerAgent(ctx(owner), {
      agentId: `${name}-agent`,
      roleName: name,
      seatId,
      actorId: actor.actorId,
    });
    return actor.credential;
  };
  member("pm", "PM");
  const developer = member("developer", "Developer");
  core.openPlan(ctx(owner), { tier: "normal", title: "Fixture plan" });
  const branch = "capstan/developer-1-g1";
  core.recordAgentPane(ctx(owner), {
    agentId: "developer-agent",
    workspaceId: null,
    paneId: null,
    worktreePath: null,
    branch,
    baseSha: "a".repeat(40),
  });
  core.recordAgentReport(ctx(developer), {
    commitSha: "b".repeat(40),
    summary: "fixture report",
    evidence: {
      generation: 1,
      branch,
      baseSha: "a".repeat(40),
      commitExists: true,
      branchTip: "b".repeat(40),
      isAncestorOfTip: true,
      isAncestorOfBase: false,
      checkedAt: "2026-10-01T00:00:00.000Z",
    },
  });
  core.close();

  const databasePath = path.join(stateDirectory, "controller.sqlite");
  const db = new Database(databasePath);
  db.pragma("wal_checkpoint(TRUNCATE)");
  const version = db
    .prepare("SELECT MAX(version) AS v FROM schema_migrations")
    .get().v;
  if (version !== 31) throw new Error(`expected schema v31, got v${version}`);
  db.close();
  const target = path.join(root, "test/fixtures/ledger-better-sqlite3.sqlite");
  copyFileSync(databasePath, target);
  for (const suffix of ["-wal", "-shm"])
    if (existsSync(target + suffix))
      throw new Error(`stray ${suffix} beside the fixture`);
  console.log(`wrote ${target}`);
} finally {
  rmSync(temp, { recursive: true, force: true });
}
