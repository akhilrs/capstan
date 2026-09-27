import { createHash, randomBytes, randomUUID } from "node:crypto";
import type Database from "better-sqlite3";
import { roles, type Capability, type Role } from "./types.js";

export class AuthenticationError extends Error {
  override readonly name = "AuthenticationError";
}

export class AuthorizationError extends Error {
  override readonly name = "AuthorizationError";
}

export interface AuthenticatedActor {
  readonly actorId: string;
  readonly projectId: string;
  readonly role: Role;
  readonly seatId?: string;
  readonly capabilities: ReadonlySet<string>;
}

export function issueCredential(): string {
  return randomBytes(32).toString("base64url");
}

export function newActorId(): string {
  return randomUUID();
}

export function credentialHash(credential: string): string {
  if (
    typeof credential !== "string" ||
    credential.length < 32 ||
    credential.length > 256
  ) {
    throw new AuthenticationError("invalid credential");
  }
  return createHash("sha256").update(credential, "utf8").digest("hex");
}

export function authenticateActor(
  database: Database.Database,
  projectId: string,
  credential: string,
): AuthenticatedActor {
  const digest = credentialHash(credential);
  const rows = database
    .prepare(
      `
    SELECT a.actor_id, a.project_id, a.role, a.seat_id, rc.capability
    FROM actors a
    JOIN capability_grants cg
      ON cg.project_id = a.project_id AND cg.actor_id = a.actor_id AND cg.revoked_at IS NULL
    JOIN role_capabilities rc
      ON rc.role = a.role AND rc.capability = cg.capability
    WHERE a.project_id = ? AND a.credential_hash = ? AND a.active = 1 AND a.revoked_at IS NULL
    ORDER BY rc.capability
  `,
    )
    .all(projectId, digest) as Array<{
    actor_id: string;
    project_id: string;
    role: string;
    seat_id: string | null;
    capability: string;
  }>;
  const actor = rows[0];
  if (!actor || !roles.includes(actor.role as Role))
    throw new AuthenticationError("credential is not active for this project");
  return {
    actorId: actor.actor_id,
    projectId: actor.project_id,
    role: actor.role as Role,
    ...(actor.seat_id ? { seatId: actor.seat_id } : {}),
    capabilities: new Set(rows.map((row) => row.capability)),
  };
}

export function requireCapability(
  actor: AuthenticatedActor,
  capability: Capability,
): void {
  if (!actor.capabilities.has(capability))
    throw new AuthorizationError(`actor lacks ${capability}`);
}
