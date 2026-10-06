import assert from "node:assert/strict";
import { openSqlite, type Database } from "../src/controller/sqlite.js";
import { test } from "node:test";
import { readdirSync, rmSync } from "node:fs";
import path from "node:path";
import {
  AuthenticationError,
  AuthorizationError,
} from "../src/controller/auth.js";
import {
  ControllerCore,
  IdempotencyConflictError,
  MutationConflictError,
  StateVersionConflictError,
} from "../src/controller/core.js";
import type { RoleDefinitionInput } from "../src/controller/types.js";
import {
  addSeatAndActor,
  cleanup,
  context,
  fixture,
} from "./controller-harness.js";

function desiredRoles(
  overrides: Partial<Record<string, Partial<RoleDefinitionInput>>> = {},
): RoleDefinitionInput[] {
  const base: RoleDefinitionInput[] = [
    { name: "pm", kind: "PM", host: "claude", configHash: "a".repeat(64) },
    {
      name: "reviewer",
      kind: "Verifier",
      host: "claude",
      configHash: "b".repeat(64),
    },
  ];
  return base.map((role) => ({ ...role, ...overrides[role.name] }));
}

function ledgerCounts(stateDirectory: string, projectId: string) {
  const db = openSqlite(path.join(stateDirectory, "controller.sqlite"));
  try {
    const count = (table: string): number =>
      (
        db
          .prepare(`SELECT COUNT(*) AS n FROM ${table} WHERE project_id = ?`)
          .get(projectId) as { n: number }
      ).n;
    return {
      version: (
        db
          .prepare("SELECT state_version FROM projects WHERE project_id = ?")
          .get(projectId) as { state_version: number }
      ).state_version,
      events: count("controller_events"),
      requests: count("mutation_requests"),
    };
  } finally {
    db.close();
  }
}

test("migration 0014 adds one table and leaves every existing row unchanged", async () => {
  const value = await fixture();
  const { core, project: info } = value;
  try {
    await addSeatAndActor(core, info.ownerCredential, "Developer", "before");
    core.syncRoleDefinitions(
      context(core, info.ownerCredential),
      desiredRoles(),
    );
    core.close();
    const databasePath = path.join(value.stateDirectory, "controller.sqlite");
    const snapshot = (db: Database): Record<string, unknown[]> => {
      const tables = (
        db
          .prepare(
            "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' AND name NOT IN ('schema_migrations', 'role_definitions', 'pauses', 'prompt_relays', 'operator_grants', 'operator_runs', 'operator_proposals', 'external_links', 'plans', 'plan_revisions', 'plan_packages', 'plan_signoffs', 'pm_notices', 'pm_wakes', 'supervision_checks', 'agent_finding_notices', 'agent_finding_checks', 'agent_finding_deliveries', 'agent_findings', 'reviews', 'integration_covered_reports', 'integration_reports', 'integrations', 'agent_reports', 'pm_restarts', 'orphan_panes', 'agent_panes', 'fallback_panes', 'message_input_clears', 'message_rejections', 'message_resolutions', 'rounds', 'messages', 'agent_waits', 'agent_state_history', 'agents') ORDER BY name",
          )
          .all() as Array<{ name: string }>
      ).map((table) => table.name);
      return Object.fromEntries(
        tables.map((name) => [
          name,
          db
            .prepare(`SELECT * FROM ${name}`)
            .all()
            .map((row) => JSON.stringify(row))
            .filter(
              (row) =>
                !row.includes('"message:') &&
                !row.includes('"plan:') &&
                !row.includes('"report:') &&
                !row.includes('"review:') &&
                !row.includes('"operator:') &&
                !row.includes('"finding:raise') &&
                !row.includes('"finding:check') &&
                !row.includes('"agent:observe') &&
                !row.includes('"prompt:relay') &&
                !row.includes('"capability":"run:control"'),
            )
            .sort(),
        ]),
      );
    };
    const db = openSqlite(databasePath);
    let before: Record<string, unknown[]>;
    try {
      for (const table of [
        "pauses",
        "prompt_relays",
        "operator_grants",
        "operator_runs",
        "operator_proposals",
        "external_links",
        "plan_signoffs",
        "plan_packages",
        "plan_revisions",
        "plans",
        "pm_notices",
        "pm_wakes",
        "supervision_checks",
        "agent_finding_notices",
        "agent_finding_checks",
        "agent_finding_deliveries",
        "agent_findings",
        "reviews",
        "integration_covered_reports",
        "integration_reports",
        "integrations",
        "agent_reports",
        "pm_restarts",
        "orphan_panes",
        "agent_panes",
        "fallback_panes",
        "message_input_clears",
        "message_rejections",
        "message_resolutions",
        "rounds",
        "messages",
        "agent_waits",
        "agent_state_history",
        "agents",
        "role_definitions",
      ])
        db.exec(`DROP TABLE ${table}`);
      db.exec(
        "DELETE FROM capability_grants WHERE capability LIKE 'message:%' OR capability LIKE 'plan:%' OR capability LIKE 'report:%' OR capability LIKE 'review:%' OR capability LIKE 'operator:%' OR capability IN ('finding:raise', 'finding:check', 'agent:observe', 'prompt:relay')",
      );
      db.exec(
        "DELETE FROM role_capabilities WHERE capability LIKE 'message:%' OR capability LIKE 'plan:%' OR capability LIKE 'report:%' OR capability LIKE 'review:%' OR capability LIKE 'operator:%' OR capability IN ('finding:raise', 'finding:check', 'agent:observe', 'prompt:relay')",
      );
      db.exec(
        "DELETE FROM transition_rules WHERE role = 'PM' AND entity_type = 'run_control'; DELETE FROM capability_grants WHERE capability = 'run:control' AND actor_id IN (SELECT actor_id FROM actors WHERE role = 'PM'); DELETE FROM role_capabilities WHERE role = 'PM' AND capability = 'run:control'",
      );
      db.exec(
        "DROP INDEX actors_by_seat_active; DROP INDEX assignments_by_seat_authority",
      );
      db.prepare("DELETE FROM schema_migrations WHERE version >= 14").run();
      before = snapshot(db);
    } finally {
      db.close();
    }
    const reopened = await ControllerCore.open({
      stateDirectory: value.stateDirectory,
      project: info,
      keepMigrationBackups: 50,
    });
    reopened.close();
    const check = openSqlite(databasePath);
    try {
      assert.deepEqual(snapshot(check), before);
      assert.deepEqual(check.pragma("foreign_key_check"), []);
      assert.deepEqual(
        check
          .prepare(
            "SELECT version, name FROM schema_migrations WHERE version >= 13 ORDER BY version",
          )
          .all(),
        [
          { version: 13, name: "0013_reconcile_queued_commands.sql" },
          { version: 14, name: "0014_role_definitions.sql" },
          { version: 15, name: "0015_messages.sql" },
          { version: 16, name: "0016_panes_and_pm_restarts.sql" },
          { version: 17, name: "0017_agent_reports.sql" },
          { version: 18, name: "0018_reviews.sql" },
          { version: 19, name: "0019_integrations.sql" },
          { version: 20, name: "0020_agent_findings.sql" },
          { version: 21, name: "0021_oversight.sql" },
          { version: 22, name: "0022_plans.sql" },
          { version: 23, name: "0023_plan_reviews.sql" },
          { version: 24, name: "0024_developer_review_request.sql" },
          { version: 25, name: "0025_external_links.sql" },
          { version: 26, name: "0026_operator.sql" },
          { version: 27, name: "0027_integration_coverage.sql" },
          { version: 28, name: "0028_operator_grants.sql" },
          { version: 29, name: "0029_coverage_merge.sql" },
          { version: 30, name: "0030_prompt_relay.sql" },
          { version: 31, name: "0031_pauses.sql" },
          { version: 32, name: "0032_integration_branch_names.sql" },
          { version: 33, name: "0033_message_action_needed.sql" },
          { version: 34, name: "0034_agent_pane_task.sql" },
          { version: 35, name: "0035_status_query_indexes.sql" },
        ],
      );
      assert.equal(
        (
          check.prepare("SELECT COUNT(*) AS n FROM role_definitions").get() as {
            n: number;
          }
        ).n,
        0,
      );
    } finally {
      check.close();
    }
    assert.ok(
      readdirSync(value.stateDirectory).some((entry) =>
        entry.startsWith("controller.sqlite.pre-v14-"),
      ),
      "a backup is taken before the migration",
    );
  } finally {
    rmSync(value.stateDirectory, { recursive: true, force: true });
  }
});

test("role sync inserts, updates, retires and reactivates with an audited event", async () => {
  const value = await fixture();
  try {
    const { core, project: info } = value;
    const owner = info.ownerCredential;
    assert.deepEqual(core.roleDefinitions(), []);
    const first = core.syncRoleDefinitions(
      context(core, owner),
      desiredRoles(),
    );
    assert.deepEqual(first, {
      changed: true,
      inserted: ["pm", "reviewer"],
      updated: [],
      reactivated: [],
      retired: [],
    });
    assert.equal(core.roleKind("reviewer"), "Verifier");
    assert.throws(
      () => core.roleKind("designer"),
      /not an active configured role/,
    );

    const second = core.syncRoleDefinitions(
      context(core, owner),
      desiredRoles({
        reviewer: { host: "codex", configHash: "c".repeat(64) },
      }).slice(1),
    );
    assert.deepEqual(second.updated, ["reviewer"]);
    assert.deepEqual(second.retired, ["pm"]);
    assert.throws(() => core.roleKind("pm"), /not an active configured role/);
    assert.deepEqual(
      core.roleDefinitions().map((role) => [role.name, role.host, role.state]),
      [
        ["pm", "claude", "retired"],
        ["reviewer", "codex", "active"],
      ],
    );

    const third = core.syncRoleDefinitions(
      context(core, owner),
      desiredRoles({ reviewer: { host: "codex", configHash: "c".repeat(64) } }),
    );
    assert.deepEqual(third.reactivated, ["pm"]);
    assert.equal(core.roleKind("pm"), "PM");

    const db = openSqlite(path.join(value.stateDirectory, "controller.sqlite"));
    try {
      const events = db
        .prepare(
          "SELECT entity_type, entity_id, payload_json FROM controller_events WHERE entity_type = 'role_definition' ORDER BY sequence",
        )
        .all() as Array<{
        entity_type: string;
        entity_id: string;
        payload_json: string;
      }>;
      assert.equal(events.length, 3);
      assert.equal(events[0]!.entity_id, info.projectId);
      const payload = JSON.parse(events[1]!.payload_json) as {
        action: string;
        payload: { roles: unknown[] };
        details: { updated: string[]; retired: string[] };
      };
      assert.equal(payload.action, "role.sync");
      assert.deepEqual(payload.payload.roles, [
        {
          name: "reviewer",
          kind: "Verifier",
          host: "codex",
          configHash: "c".repeat(64),
        },
      ]);
      assert.deepEqual(payload.details.updated, ["reviewer"]);
    } finally {
      db.close();
    }
  } finally {
    cleanup(value);
  }
});

test("a repeated identical role sync writes nothing", async () => {
  const value = await fixture();
  try {
    const { core, project: info } = value;
    core.syncRoleDefinitions(
      context(core, info.ownerCredential),
      desiredRoles(),
    );
    const before = ledgerCounts(value.stateDirectory, info.projectId);
    const result = core.syncRoleDefinitions(
      context(core, info.ownerCredential),
      desiredRoles(),
    );
    assert.equal(result.changed, false);
    assert.deepEqual(
      ledgerCounts(value.stateDirectory, info.projectId),
      before,
    );
  } finally {
    cleanup(value);
  }
});

test("a role sync with a stale context conflicts and writes nothing", async () => {
  const value = await fixture();
  try {
    const { core, project: info } = value;
    const stale = context(core, info.ownerCredential);
    core.createSeat(context(core, info.ownerCredential), {
      seatId: "interleaved-seat",
      name: "interleaved",
      role: "Developer",
    });
    const before = ledgerCounts(value.stateDirectory, info.projectId);
    assert.throws(
      () => core.syncRoleDefinitions(stale, desiredRoles()),
      StateVersionConflictError,
    );
    assert.deepEqual(
      ledgerCounts(value.stateDirectory, info.projectId),
      before,
    );
    assert.deepEqual(core.roleDefinitions(), []);
    assert.equal(
      core.syncRoleDefinitions(
        context(core, info.ownerCredential),
        desiredRoles(),
      ).changed,
      true,
    );
  } finally {
    cleanup(value);
  }
});

test("every write to seats and role definitions bumps the project version", async () => {
  const value = await fixture();
  try {
    const { core, project: info } = value;
    const versions = [core.stateVersion];
    core.createSeat(context(core, info.ownerCredential), {
      seatId: "version-seat",
      name: "version",
      role: "Developer",
    });
    versions.push(core.stateVersion);
    core.syncRoleDefinitions(
      context(core, info.ownerCredential),
      desiredRoles(),
    );
    versions.push(core.stateVersion);
    assert.deepEqual(
      versions.map(
        (version, index) => index === 0 || version > versions[index - 1]!,
      ),
      [true, true, true],
    );
  } finally {
    cleanup(value);
  }
});

test("a first role insert is refused when a seat of that name has another kind", async () => {
  const value = await fixture();
  try {
    const { core, project: info } = value;
    const owner = info.ownerCredential;
    core.createSeat(context(core, owner), {
      seatId: "old-reviewer",
      name: "reviewer",
      role: "Developer",
    });
    const before = ledgerCounts(value.stateDirectory, info.projectId);
    assert.throws(
      () => core.syncRoleDefinitions(context(core, owner), desiredRoles()),
      MutationConflictError,
    );
    assert.deepEqual(
      ledgerCounts(value.stateDirectory, info.projectId),
      before,
    );
    assert.deepEqual(core.roleDefinitions(), []);
    const matching = desiredRoles({
      reviewer: { kind: "Developer", configHash: "7".repeat(64) },
    });
    assert.equal(
      core.syncRoleDefinitions(context(core, owner), matching).changed,
      true,
    );
  } finally {
    cleanup(value);
  }
});

test("replaying an applied kind change after a seat took the name returns the stored result", async () => {
  const value = await fixture();
  try {
    const { core, project: info } = value;
    const owner = info.ownerCredential;
    core.syncRoleDefinitions(context(core, owner), desiredRoles());
    const change = context(core, owner);
    const applied = core.syncRoleDefinitions(
      change,
      desiredRoles({
        reviewer: { kind: "Developer", configHash: "9".repeat(64) },
      }),
    );
    assert.deepEqual(applied.updated, ["reviewer"]);
    core.createSeat(context(core, owner), {
      seatId: "late-seat",
      name: "reviewer",
      role: "Developer",
    });
    const before = ledgerCounts(value.stateDirectory, info.projectId);
    assert.deepEqual(
      core.syncRoleDefinitions(
        change,
        desiredRoles({
          reviewer: { kind: "Developer", configHash: "9".repeat(64) },
        }),
      ),
      applied,
    );
    assert.deepEqual(
      ledgerCounts(value.stateDirectory, info.projectId),
      before,
    );
  } finally {
    cleanup(value);
  }
});

test("a role kind cannot change while a seat is named after the role", async () => {
  const value = await fixture();
  try {
    const { core, project: info } = value;
    const owner = info.ownerCredential;
    core.syncRoleDefinitions(context(core, owner), desiredRoles());
    core.createSeat(context(core, owner), {
      seatId: "reviewer-seat",
      name: "reviewer",
      role: "Verifier",
    });
    const before = ledgerCounts(value.stateDirectory, info.projectId);
    assert.throws(
      () =>
        core.syncRoleDefinitions(
          context(core, owner),
          desiredRoles({
            reviewer: { kind: "Developer", configHash: "d".repeat(64) },
          }),
        ),
      MutationConflictError,
    );
    assert.deepEqual(
      ledgerCounts(value.stateDirectory, info.projectId),
      before,
    );
    const changed = core.syncRoleDefinitions(
      context(core, owner),
      desiredRoles({ pm: { kind: "Supervisor", configHash: "e".repeat(64) } }),
    );
    assert.deepEqual(changed.updated, ["pm"]);
    assert.equal(core.roleKind("pm"), "Supervisor");
  } finally {
    cleanup(value);
  }
});

test("a role sync that changes nothing still authenticates and authorizes the caller", async () => {
  const value = await fixture();
  try {
    const { core, project: info } = value;
    core.syncRoleDefinitions(
      context(core, info.ownerCredential),
      desiredRoles(),
    );
    const pm = await addSeatAndActor(core, info.ownerCredential, "PM", "probe");
    assert.throws(
      () =>
        core.syncRoleDefinitions(context(core, pm.credential), desiredRoles()),
      AuthorizationError,
    );
    assert.throws(
      () =>
        core.syncRoleDefinitions(
          context(core, `${"x".repeat(40)}`),
          desiredRoles({ reviewer: { kind: "Developer" } }),
        ),
      AuthenticationError,
    );
  } finally {
    cleanup(value);
  }
});

test("replaying an applied role sync returns the original result and writes nothing", async () => {
  const value = await fixture();
  try {
    const { core, project: info } = value;
    const applied = context(core, info.ownerCredential);
    const first = core.syncRoleDefinitions(applied, desiredRoles());
    assert.equal(first.changed, true);
    const before = ledgerCounts(value.stateDirectory, info.projectId);
    assert.deepEqual(core.syncRoleDefinitions(applied, desiredRoles()), first);
    assert.deepEqual(
      ledgerCounts(value.stateDirectory, info.projectId),
      before,
    );
    assert.throws(
      () =>
        core.syncRoleDefinitions(
          applied,
          desiredRoles({ pm: { host: "codex" } }),
        ),
      IdempotencyConflictError,
    );
  } finally {
    cleanup(value);
  }
});

test("role sync rejects malformed definitions and records only the four known fields", async () => {
  const value = await fixture();
  try {
    const { core, project: info } = value;
    const owner = info.ownerCredential;
    const good = desiredRoles();
    for (const bad of [
      { ...good[0]!, name: "Upper" },
      { ...good[0]!, name: "" },
      { ...good[0]!, host: "a b" },
      { ...good[0]!, kind: "operator" as never },
      { ...good[0]!, configHash: "A".repeat(64) },
      { ...good[0]!, configHash: "abc" },
    ])
      assert.throws(
        () => core.syncRoleDefinitions(context(core, owner), [bad]),
        TypeError,
      );
    assert.deepEqual(core.roleDefinitions(), []);
    const withExtra = good.map((role) => ({
      ...role,
      secret: "must-not-be-stored",
    }));
    core.syncRoleDefinitions(context(core, owner), withExtra);
    const db = openSqlite(path.join(value.stateDirectory, "controller.sqlite"));
    try {
      const payload = (
        db
          .prepare(
            "SELECT payload_json FROM controller_events WHERE entity_type = 'role_definition'",
          )
          .get() as { payload_json: string }
      ).payload_json;
      assert.ok(!payload.includes("must-not-be-stored"));
    } finally {
      db.close();
    }
  } finally {
    cleanup(value);
  }
});

test("role sync needs the actor:manage capability and unique names", async () => {
  const value = await fixture();
  try {
    const { core, project: info } = value;
    const pm = await addSeatAndActor(core, info.ownerCredential, "PM", "sync");
    assert.throws(
      () =>
        core.syncRoleDefinitions(context(core, pm.credential), desiredRoles()),
      AuthorizationError,
    );
    assert.throws(
      () =>
        core.syncRoleDefinitions(context(core, info.ownerCredential), [
          ...desiredRoles(),
          ...desiredRoles(),
        ]),
      /role names must be unique/,
    );
    assert.deepEqual(core.roleDefinitions(), []);
  } finally {
    cleanup(value);
  }
});
