import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { ControllerCore } from "../src/controller/core.js";
import type {
  InitialProject,
  MutationContext,
  Role,
} from "../src/controller/types.js";

export const inputKinds = [
  "project_config",
  "task_brief",
  "acceptance_criteria",
  "policy",
  "plan",
] as const;

export function project(): InitialProject {
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

export interface Fixture {
  readonly core: ControllerCore;
  readonly stateDirectory: string;
  readonly project: InitialProject;
}

export async function fixture(runtimeWorkspacePath?: string): Promise<Fixture> {
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

export function context(
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

export function cleanup(value: Fixture): void {
  value.core.close();
  rmSync(value.stateDirectory, { recursive: true, force: true });
}

export async function addSeatAndActor(
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
