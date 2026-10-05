import assert from "node:assert/strict";
import {
  mkdtempSync,
  readdirSync,
  readFileSync,
  rmSync,
  statSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { spawnSync } from "node:child_process";
import { test } from "node:test";
import { createHash } from "node:crypto";
import { openSqlite } from "../src/controller/sqlite.js";
import {
  ControllerCore,
  IdempotencyConflictError,
  StateVersionConflictError,
  TransitionAuthorizationError,
} from "../src/controller/core.js";
import type { InitialProject } from "../src/controller/types.js";
import {
  commandStateOf,
  insertAssignment,
  insertCandidate,
  insertDependency,
  insertFinalVerification,
  insertFinding,
  insertRecovery,
  insertWorkItem,
  seedLedger,
  setSupervisionDegraded,
} from "./legacy-rows.js";
import {
  addSeatAndActor,
  cleanup,
  context,
  fixture,
  inputKinds,
  project,
} from "./controller-harness.js";

test("legacy finding target migration binds only one durable delivery target", () => {
  const db = openSqlite(":memory:");
  try {
    db.exec(`
      CREATE TABLE findings (
        project_id TEXT NOT NULL, finding_id TEXT NOT NULL,
        affected_assignment_id TEXT, affected_seat_id TEXT,
        affected_work_item_id TEXT, affected_generation INTEGER,
        state TEXT NOT NULL
      );
      CREATE TABLE finding_deliveries (
        project_id TEXT NOT NULL, finding_id TEXT NOT NULL,
        delivery_id TEXT NOT NULL, seat_id TEXT NOT NULL, command_id TEXT NOT NULL,
        delivered_at TEXT
      );
      CREATE TABLE commands (
        project_id TEXT NOT NULL, command_id TEXT NOT NULL,
        assignment_id TEXT NOT NULL, generation INTEGER NOT NULL
      );
      CREATE TABLE assignments (
        project_id TEXT NOT NULL, assignment_id TEXT NOT NULL,
        seat_id TEXT NOT NULL, work_item_id TEXT NOT NULL
      );
      INSERT INTO findings VALUES
        ('p', 'unique', NULL, NULL, NULL, NULL, 'reported'),
        ('p', 'ambiguous', NULL, NULL, NULL, NULL, 'reported'),
        ('p', 'ambiguous-generation', NULL, NULL, NULL, NULL, 'reported'),
        ('p', 'conflicting-seat', NULL, NULL, NULL, NULL, 'reported');
      INSERT INTO assignments VALUES
        ('p', 'a1', 'seat-1', 'work-1'),
        ('p', 'a2', 'seat-2', 'work-2');
      INSERT INTO commands VALUES
        ('p', 'c1', 'a1', 3),
        ('p', 'c2', 'a2', 4),
        ('p', 'c3', 'a1', 4);
      INSERT INTO finding_deliveries VALUES
        ('p', 'unique', 'd1', 'seat-1', 'c1', '2026-01-01T00:00:00Z'),
        ('p', 'ambiguous', 'd2', 'seat-1', 'c1', '2026-01-01T00:00:00Z'),
        ('p', 'ambiguous', 'd3', 'seat-2', 'c2', '2026-01-01T00:00:01Z'),
        ('p', 'ambiguous-generation', 'd4', 'seat-1', 'c1', '2026-01-01T00:00:00Z'),
        ('p', 'ambiguous-generation', 'd5', 'seat-1', 'c3', '2026-01-01T00:00:01Z'),
        ('p', 'conflicting-seat', 'd6', 'seat-1', 'c1', '2026-01-01T00:00:00Z'),
        ('p', 'conflicting-seat', 'd7', 'seat-2', 'c1', '2026-01-01T00:00:01Z');
    `);
    db.exec(
      readFileSync(
        new URL(
          "../migrations/0012_backfill_finding_targets.sql",
          import.meta.url,
        ),
        "utf8",
      ),
    );
    assert.deepEqual(
      db
        .prepare(
          "SELECT affected_assignment_id, affected_seat_id, affected_work_item_id, affected_generation FROM findings WHERE finding_id = 'unique'",
        )
        .get(),
      {
        affected_assignment_id: "a1",
        affected_seat_id: "seat-1",
        affected_work_item_id: "work-1",
        affected_generation: 3,
      },
    );
    assert.deepEqual(
      db
        .prepare(
          "SELECT affected_assignment_id, affected_seat_id FROM findings WHERE finding_id = 'ambiguous'",
        )
        .get(),
      { affected_assignment_id: null, affected_seat_id: null },
    );
    assert.deepEqual(
      db
        .prepare(
          "SELECT affected_assignment_id, affected_generation FROM findings WHERE finding_id = 'ambiguous-generation'",
        )
        .get(),
      { affected_assignment_id: null, affected_generation: null },
    );
    assert.deepEqual(
      db
        .prepare(
          "SELECT affected_assignment_id, affected_seat_id, affected_work_item_id, affected_generation FROM findings WHERE finding_id = 'conflicting-seat'",
        )
        .get(),
      {
        affected_assignment_id: null,
        affected_seat_id: null,
        affected_work_item_id: null,
        affected_generation: null,
      },
    );
  } finally {
    db.close();
  }
});
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
    const assignment = { commandId: "offline-status-command" };
    seedLedger(value.stateDirectory, (db) => {
      insertWorkItem(db, value.project.projectId, {
        workItemId: "offline-status-work",
        title: "Offline status",
        description: "Preserve uncertain state during inspection",
        state: "running",
      });
      insertAssignment(db, value.project.projectId, {
        assignmentId: "offline-status-assignment",
        workItemId: "offline-status-work",
        seatId: developer.seatId,
        workerActorId: developer.actorId,
        state: "running",
        command: { commandId: assignment.commandId, state: "attempting" },
      });
    });
    const expectedVersion = core.stateVersion;
    const expectedRunState = core.statusSnapshot().run.state;
    const expectedCommandState = seedLedger(value.stateDirectory, (db) =>
      commandStateOf(db, assignment.commandId),
    );
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
      seedLedger(value.stateDirectory, (db) =>
        commandStateOf(db, assignment.commandId),
      ),
      expectedCommandState,
    );
    assert.deepEqual(readOnly.inspect("offline-status-work"), expectedInspect);
    assert.equal(readOnly.stateVersion, expectedVersion);
    assert.throws(
      () =>
        readOnly.createSeat(context(readOnly, value.project.ownerCredential), {
          seatId: "read-only-seat",
          name: "Read only",
          role: "PM",
        }),
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
    const db = openSqlite(path.join(stateDirectory, "controller.sqlite"), {
      readOnly: true,
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
    const db = openSqlite(
      path.join(value.stateDirectory, "controller.sqlite"),
      { readOnly: true },
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
    assert.equal(core.identify(actor.credential).actorId, actor.actorId);
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
    const db = openSqlite(path.join(value.stateDirectory, "controller.sqlite"));
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
    const assignment = { commandId: "restart-command" };
    seedLedger(stateDirectory, (db) => {
      insertWorkItem(db, info.projectId, {
        workItemId: "restart-work",
        title: "Restart work",
        description: "Bounded task",
        state: "running",
      });
      insertAssignment(db, info.projectId, {
        assignmentId: "restart-assignment",
        workItemId: "restart-work",
        seatId: developer.seatId,
        workerActorId: developer.actorId,
        command: { commandId: assignment.commandId, state: "attempting" },
      });
      assert.equal(commandStateOf(db, assignment.commandId), "attempting");
    });
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
      assert.equal(
        seedLedger(stateDirectory, (db) =>
          commandStateOf(db, assignment.commandId),
        ),
        "unknown",
      );
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
    seedLedger(value.stateDirectory, (db) => {
      insertWorkItem(db, info.projectId, {
        workItemId: "revocation-active-work",
        title: "Worker receipt after revocation attempt",
        description: "The active seat principal must remain able to report",
        state: "running",
      });
      insertAssignment(db, info.projectId, {
        assignmentId: "revocation-active-assignment",
        workItemId: "revocation-active-work",
        seatId: developer.seatId,
        workerActorId: developer.actorId,
        state: "running",
        command: { commandId: "revocation-active-command", state: "started" },
      });
    });
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
    assert.throws(
      () => core.readiness("post-terminal-work"),
      /work item does not exist/,
    );
  } finally {
    cleanup(value);
  }
});
test("mutation replay is exact and conflicting idempotency-key reuse has no side effect", async () => {
  const value = await fixture();
  try {
    const { core, project: info } = value;
    const request = context(core, info.ownerCredential, "seat-create");
    const input = {
      seatId: "replay-seat",
      name: "Original",
      role: "Developer" as const,
    };
    const first = core.createSeat(request, input);
    const versionAfterFirst = core.stateVersion;
    assert.deepEqual(core.createSeat(request, input), first);
    assert.equal(core.stateVersion, versionAfterFirst);
    assert.throws(
      () => core.createSeat(request, { ...input, name: "Conflicting reuse" }),
      IdempotencyConflictError,
    );
    const staleVersionId = crypto.randomUUID();
    assert.throws(
      () =>
        core.createSeat(
          {
            ...request,
            requestId: `req-${staleVersionId}`,
            idempotencyKey: `idem-${staleVersionId}`,
          },
          { ...input, seatId: "stale-version-seat" },
        ),
      StateVersionConflictError,
    );
    assert.equal(core.stateVersion, versionAfterFirst);
  } finally {
    cleanup(value);
  }
});

test("opening the controller reconciles an active assignment with an in-flight command fail-closed", async () => {
  const value = await fixture();
  const { stateDirectory, project: info } = value;
  try {
    const developer = await addSeatAndActor(
      value.core,
      info.ownerCredential,
      "Developer",
      "open-reconcile",
    );
    const reporter = await addSeatAndActor(
      value.core,
      info.ownerCredential,
      "Developer",
      "open-reported",
    );
    value.core.close();
    seedLedger(stateDirectory, (db) => {
      insertWorkItem(db, info.projectId, {
        workItemId: "in-flight-work",
        state: "running",
      });
      insertAssignment(db, info.projectId, {
        assignmentId: "in-flight-assignment",
        workItemId: "in-flight-work",
        seatId: developer.seatId,
        workerActorId: developer.actorId,
        state: "running",
        command: {
          commandId: "in-flight-command",
          state: "attempting",
          startRequested: true,
        },
      });
      insertWorkItem(db, info.projectId, {
        workItemId: "reported-work",
        state: "awaiting_verification",
      });
      insertAssignment(db, info.projectId, {
        assignmentId: "reported-assignment",
        workItemId: "reported-work",
        seatId: reporter.seatId,
        workerActorId: reporter.actorId,
        state: "reported",
        command: { commandId: "reported-command", state: "completed" },
      });
    });
    const eventsBefore = seedLedger(stateDirectory, (db) => ({
      version: (
        db
          .prepare("SELECT state_version FROM projects WHERE project_id = ?")
          .get(info.projectId) as { state_version: number }
      ).state_version,
      events: (
        db
          .prepare(
            "SELECT COUNT(*) AS n FROM controller_events WHERE project_id = ?",
          )
          .get(info.projectId) as { n: number }
      ).n,
    }));
    const reopened = await ControllerCore.open({
      stateDirectory,
      project: info,
    });
    reopened.close();
    const rows = seedLedger(stateDirectory, (db) => {
      const one = (sql: string, ...args: string[]): unknown =>
        db.prepare(sql).get(...args);
      return {
        inFlightCommand: one(
          "SELECT state, state_version FROM commands WHERE command_id = ?",
          "in-flight-command",
        ),
        reportedCommand: one(
          "SELECT state, state_version FROM commands WHERE command_id = ?",
          "reported-command",
        ),
        outbox: db
          .prepare(
            "SELECT command_id, ordinal, outcome, response_json FROM outbox_delivery_attempts ORDER BY command_id, ordinal",
          )
          .all(),
        inFlightAssignment: one(
          "SELECT state, authority_state, state_version, ended_at IS NOT NULL AS ended FROM assignments WHERE assignment_id = ?",
          "in-flight-assignment",
        ),
        inFlightAttempt: one(
          "SELECT state, authority_state, state_version FROM assignment_attempts WHERE assignment_id = ?",
          "in-flight-assignment",
        ),
        reportedAssignment: one(
          "SELECT state, authority_state, state_version FROM assignments WHERE assignment_id = ?",
          "reported-assignment",
        ),
        reportedAttempt: one(
          "SELECT state, authority_state, state_version FROM assignment_attempts WHERE assignment_id = ?",
          "reported-assignment",
        ),
        works: db
          .prepare(
            "SELECT work_item_id, state, state_version FROM work_items ORDER BY work_item_id",
          )
          .all(),
        events: db
          .prepare(
            `SELECT entity_type, entity_id, from_state, to_state, state_version, request_id, payload_json
             FROM controller_events WHERE request_id LIKE 'restart-reconcile:%' ORDER BY request_id`,
          )
          .all(),
        eventTotal: (
          one(
            "SELECT COUNT(*) AS n FROM controller_events WHERE project_id = ?",
            info.projectId,
          ) as { n: number }
        ).n,
        version: (
          one(
            "SELECT state_version FROM projects WHERE project_id = ?",
            info.projectId,
          ) as { state_version: number }
        ).state_version,
      };
    });
    assert.deepEqual(rows.inFlightCommand, {
      state: "unknown",
      state_version: 2,
    });
    assert.deepEqual(rows.reportedCommand, {
      state: "completed",
      state_version: 1,
    });
    assert.deepEqual(
      (rows.outbox as Array<{ command_id: string; outcome: string }>).map(
        (row) => [row.command_id, row.outcome],
      ),
      [["in-flight-command", "unknown"]],
    );
    assert.deepEqual(rows.inFlightAssignment, {
      state: "revoked",
      authority_state: "unknown",
      state_version: 2,
      ended: 1,
    });
    assert.deepEqual(rows.inFlightAttempt, {
      state: "revoked",
      authority_state: "unknown",
      state_version: 2,
    });
    assert.deepEqual(rows.reportedAssignment, {
      state: "reported",
      authority_state: "unknown",
      state_version: 2,
    });
    assert.deepEqual(rows.reportedAttempt, {
      state: "reported",
      authority_state: "unknown",
      state_version: 2,
    });
    assert.deepEqual(rows.works, [
      { work_item_id: "in-flight-work", state: "blocked", state_version: 2 },
      { work_item_id: "reported-work", state: "blocked", state_version: 2 },
    ]);
    assert.deepEqual(
      (
        rows.events as Array<{
          entity_type: string;
          entity_id: string;
          from_state: string;
          to_state: string;
          request_id: string;
          payload_json: string;
        }>
      ).map((event) => [
        event.entity_type,
        event.entity_id,
        event.from_state,
        event.to_state,
        event.request_id,
        JSON.parse(event.payload_json),
      ]),
      [
        [
          "assignment_attempt",
          "in-flight-assignment",
          "running",
          "revoked",
          "restart-reconcile:in-flight-command",
          {
            commandState: "attempting",
            authorityState: "unknown",
            preservedReport: false,
            startRequested: true,
          },
        ],
        [
          "assignment_attempt",
          "reported-assignment",
          "reported",
          "reported",
          "restart-reconcile:reported-command",
          {
            commandState: "completed",
            authorityState: "unknown",
            preservedReport: true,
            startRequested: false,
          },
        ],
      ],
    );
    assert.equal(rows.eventTotal, eventsBefore.events + 2);
    assert.equal(rows.version, eventsBefore.version + 2);
    const again = await ControllerCore.open({ stateDirectory, project: info });
    try {
      assert.equal(again.stateVersion, rows.version);
    } finally {
      again.close();
    }
  } finally {
    rmSync(stateDirectory, { recursive: true, force: true });
  }
});
test("status and inspect over one row of each legacy kind match the pre-removal golden", async () => {
  const stateDirectory = mkdtempSync(
    path.join(tmpdir(), "capstan-controller-golden-"),
  );
  const info: InitialProject = {
    projectId: "pgoldenlegacyrows",
    name: "Legacy golden project",
    ownerCredential: "owner-golden-legacy-credential-0123456789",
    initialInputs: inputKinds.map((kind) => ({
      kind,
      content:
        kind === "acceptance_criteria"
          ? ["criterion-one"]
          : { kind, revision: 1 },
    })),
  };
  const first = await ControllerCore.open({ stateDirectory, project: info });
  try {
    const developer = await addSeatAndActor(
      first,
      info.ownerCredential,
      "Developer",
      "golden-dev",
    );
    const verifier = await addSeatAndActor(
      first,
      info.ownerCredential,
      "Verifier",
      "golden-verifier",
    );
    first.close();
    seedLedger(stateDirectory, (db) => {
      const id = info.projectId;
      insertWorkItem(db, id, {
        workItemId: "golden-prerequisite",
        title: "Prerequisite",
        state: "accepted",
        createdAt: "2026-01-01T00:00:01.000Z",
      });
      insertWorkItem(db, id, {
        workItemId: "golden-work",
        title: "Golden work",
        state: "running",
        createdAt: "2026-01-01T00:00:02.000Z",
        acceptanceCriteria: ["criterion-one"],
      });
      insertWorkItem(db, id, {
        workItemId: "golden-blocked-by",
        title: "Waits on work",
        state: "pending",
        createdAt: "2026-01-01T00:00:03.000Z",
      });
      for (const [workItemId, state] of [
        ["golden-ready", "ready"],
        ["golden-pending", "pending"],
        ["golden-awaiting", "awaiting_verification"],
        ["golden-failed", "blocked"],
      ] as const)
        insertWorkItem(db, id, {
          workItemId,
          state,
          createdAt: "2026-01-01T00:00:04.000Z",
        });
      insertDependency(db, id, "golden-blocked-by", "golden-work");
      insertDependency(db, id, "golden-work", "golden-prerequisite");
      insertAssignment(db, id, {
        assignmentId: "golden-assignment",
        workItemId: "golden-work",
        seatId: developer.seatId,
        workerActorId: developer.actorId,
        state: "running",
      });
      insertAssignment(db, id, {
        assignmentId: "golden-verifier-assignment",
        workItemId: "golden-prerequisite",
        seatId: verifier.seatId,
        workerActorId: verifier.actorId,
        state: "completed",
        authorityState: "contained",
      });
      insertCandidate(db, id, {
        candidateId: "golden-candidate",
        assignmentId: "golden-assignment",
        developerEvidence: ["npm test"],
        evidence: [
          {
            evidenceId: "golden-evidence",
            verifierAssignmentId: "golden-verifier-assignment",
            criterion: "criterion-one",
          },
        ],
      });
      insertFinalVerification(db, id, {
        workItemId: "golden-prerequisite",
        assignmentId: "golden-verifier-assignment",
        evidence: [
          { evidenceId: "golden-final-evidence", criterion: "criterion-one" },
        ],
      });
      insertFinding(db, id, {
        findingId: "golden-finding",
        workItemId: "golden-work",
        assignmentId: "golden-assignment",
        seatId: developer.seatId,
        generation: 1,
      });
      insertRecovery(db, id, {
        recoveryId: "golden-recovery",
        workItemId: "golden-work",
        assignmentId: "golden-assignment",
      });
      setSupervisionDegraded(db, id, "golden supervision failure");
    });
  } finally {
    // the first controller is already closed
  }
  const core = await ControllerCore.open({ stateDirectory, project: info });
  try {
    const names = new Map<string, string>([
      [
        actorIdByRole(stateDirectory, info.projectId, "Developer"),
        "developer-actor",
      ],
      [
        actorIdByRole(stateDirectory, info.projectId, "Verifier"),
        "verifier-actor",
      ],
      [
        actorIdByRole(stateDirectory, info.projectId, "operator"),
        "owner-actor",
      ],
    ]);
    let text = JSON.stringify({
      status: core.statusSnapshot(),
      inspect: Object.fromEntries(
        [
          "golden-work",
          "golden-assignment",
          "golden-candidate",
          "golden-finding",
          "golden-recovery",
        ].map((target) => [target, core.inspect(target)]),
      ),
    });
    for (const [actorId, name] of names) text = text.replaceAll(actorId, name);
    const actual = JSON.stringify(JSON.parse(text), null, 2) + "\n";
    const goldenPath = path.resolve(
      import.meta.dirname,
      "..",
      "..",
      "test",
      "golden",
      "legacy-status-inspect.json",
    );
    if (process.env.UPDATE_GOLDEN === "1") writeFileSync(goldenPath, actual);
    assert.deepEqual(
      JSON.parse(actual),
      JSON.parse(readFileSync(goldenPath, "utf8")),
    );
  } finally {
    core.close();
    rmSync(stateDirectory, { recursive: true, force: true });
  }
});

function actorIdByRole(
  stateDirectory: string,
  projectId: string,
  role: string,
): string {
  return seedLedger(
    stateDirectory,
    (db) =>
      (
        db
          .prepare(
            "SELECT actor_id FROM actors WHERE project_id = ? AND role = ? ORDER BY created_at, actor_id LIMIT 1",
          )
          .get(projectId, role) as { actor_id: string }
      ).actor_id,
  );
}
