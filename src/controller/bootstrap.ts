import { randomBytes, randomUUID } from "node:crypto";
import type { Database } from "./sqlite.js";
import { credentialHash } from "./auth.js";
import { canonicalJson, sha256 } from "./canonical.js";
import type { InitialProject, InputKind } from "./types.js";
import { acceptanceCriteriaFromContent } from "./helpers.js";

export function initializeProject(
  database: Database,
  project: InitialProject,
): void {
  const initialInputs = project.initialInputs.map((input) => ({
    kind: input.kind,
    content: canonicalJson(input.content),
  }));
  const name = project.name;
  const requiredKinds: readonly InputKind[] = [
    "project_config",
    "task_brief",
    "acceptance_criteria",
    "policy",
    "plan",
  ];
  if (
    initialInputs.length !== requiredKinds.length ||
    requiredKinds.some(
      (kind) =>
        initialInputs.filter((input) => input.kind === kind).length !== 1,
    )
  ) {
    throw new TypeError(
      "initial project requires exactly one revision of each durable input kind",
    );
  }
  if (name.trim().length === 0)
    throw new TypeError("project name must not be empty");
  canonicalJson(name);
  const ownerHash = credentialHash(project.ownerCredential);
  const internalHash = credentialHash(randomBytes(32).toString("base64url"));
  const now = new Date().toISOString();
  const ownerId = randomUUID();
  const internalId = randomUUID();
  database.exec("BEGIN IMMEDIATE");
  try {
    database
      .prepare(
        "INSERT INTO projects(project_id, name, current_input_revision, state_version, created_at) VALUES (?, ?, 1, 1, ?)",
      )
      .run(project.projectId, name, now);
    database
      .prepare(
        "INSERT INTO supervision_control(project_id, enabled, health, target_epoch, updated_at) VALUES (?, 0, 'degraded', 0, ?)",
      )
      .run(project.projectId, now);
    const insertActor = database.prepare(`
      INSERT INTO actors(actor_id, project_id, display_name, role, seat_id, credential_hash, active, is_internal, created_at)
      VALUES (?, ?, ?, ?, NULL, ?, 1, ?, ?)
    `);
    insertActor.run(
      ownerId,
      project.projectId,
      "project operator",
      "operator",
      ownerHash,
      0,
      now,
    );
    insertActor.run(
      internalId,
      project.projectId,
      "capstan internal controller",
      "controller",
      internalHash,
      1,
      now,
    );
    const grantRole = database.prepare(`
      INSERT INTO capability_grants(project_id, actor_id, capability, granted_by, granted_at)
      SELECT ?, ?, capability, ?, ? FROM role_capabilities WHERE role = ?
    `);
    grantRole.run(project.projectId, ownerId, ownerId, now, "operator");
    grantRole.run(project.projectId, internalId, ownerId, now, "controller");
    const insertRevision = database.prepare(`
      INSERT INTO project_revisions(project_id, revision, kind, content_json, content_hash, created_by, request_id, created_at)
      VALUES (?, 1, ?, ?, ?, ?, ?, ?)
    `);
    for (const input of initialInputs) {
      if (input.kind === "acceptance_criteria")
        acceptanceCriteriaFromContent(JSON.parse(input.content));
      insertRevision.run(
        project.projectId,
        input.kind,
        input.content,
        sha256(input.content),
        ownerId,
        `bootstrap:${project.projectId}`,
        now,
      );
    }
    database
      .prepare(
        "INSERT INTO run_controls(project_id, state, state_version, updated_at) VALUES (?, 'active', 0, ?)",
      )
      .run(project.projectId, now);
    database
      .prepare(
        `
      INSERT INTO controller_events(project_id, sequence, event_id, entity_type, entity_id, from_state, to_state,
        state_version, actor_id, request_id, input_revision, payload_json, created_at)
      VALUES (?, 1, ?, 'project', ?, NULL, 'initialized', 1, ?, ?, 1, ?, ?)
    `,
      )
      .run(
        project.projectId,
        randomUUID(),
        project.projectId,
        ownerId,
        `bootstrap:${project.projectId}`,
        canonicalJson({ inputKinds: requiredKinds }),
        now,
      );
    database.exec("COMMIT");
  } catch (error) {
    database.exec("ROLLBACK");
    throw error;
  }
}
