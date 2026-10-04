import assert from "node:assert/strict";
import net from "node:net";
import path from "node:path";
import { removeTempDir, tempDir } from "./tmp.js";
import { callDaemon } from "../src/client.js";
import {
  startDaemonServer,
  type CommandResponse,
  type DaemonServer,
  type LogEntry,
} from "../src/daemon.js";
import {
  createCommandHandlers,
  type CommandDependencies,
} from "../src/commands.js";
import { ControllerCore } from "../src/controller/core.js";
import type {
  InitialProject,
  MutationContext,
} from "../src/controller/types.js";

const inputKinds = [
  "project_config",
  "task_brief",
  "acceptance_criteria",
  "policy",
  "plan",
] as const;

export function projectInfo(): InitialProject {
  const identity = crypto.randomUUID().replaceAll("-", "");
  return {
    projectId: `p${identity}`,
    name: "Daemon test project",
    ownerCredential: `owner-${identity}`,
    initialInputs: inputKinds.map((kind) => ({
      kind,
      content:
        kind === "acceptance_criteria" ? ["criterion"] : { kind, revision: 1 },
    })),
  };
}

export function ctx(core: ControllerCore, credential: string): MutationContext {
  const id = crypto.randomUUID();
  return {
    credential,
    requestId: `req-${id}`,
    idempotencyKey: `idem-${id}`,
    expectedVersion: core.stateVersion,
    inputRevision: core.inputRevision,
  };
}

export interface Member {
  readonly agentId: string;
  readonly credential: string;
  readonly actorId: string;
}

export interface Harness {
  readonly core: ControllerCore;
  readonly stateDirectory: string;
  readonly socketPath: string;
  readonly info: InitialProject;
  readonly owner: string;
  readonly pm: Member;
  readonly developer: Member;
  /** Registers one more agent; the names developer2 and pm2 have role definitions. */
  readonly addMember: (
    name: string,
    kind: "PM" | "Developer" | "Supervisor",
  ) => Member;
  readonly seatOnly: string;
  readonly log: LogEntry[];
  readonly shutdowns: number[];
  readonly server: DaemonServer;
}

export async function harness(
  options: {
    commands?: Partial<CommandDependencies>;
    clock?: () => Date;
  } = {},
): Promise<Harness> {
  const stateDirectory = tempDir("capstan-daemon-");
  const info = projectInfo();
  const core = await ControllerCore.open({
    stateDirectory,
    project: info,
    ...(options.clock === undefined ? {} : { clock: options.clock }),
  });
  const owner = info.ownerCredential;
  core.syncRoleDefinitions(ctx(core, owner), [
    { name: "pm", kind: "PM", host: "claude", configHash: "a".repeat(64) },
    {
      name: "developer",
      kind: "Developer",
      host: "claude",
      configHash: "b".repeat(64),
    },
    {
      name: "developer2",
      kind: "Developer",
      host: "claude",
      configHash: "c".repeat(64),
    },
    { name: "pm2", kind: "PM", host: "claude", configHash: "d".repeat(64) },
    {
      name: "supervisor",
      kind: "Supervisor",
      host: "claude",
      configHash: "e".repeat(64),
    },
  ]);
  const member = (
    name: string,
    kind: "PM" | "Developer" | "Supervisor",
  ): Member => {
    const seatId = `${name}-seat`;
    core.createSeat(ctx(core, owner), { seatId, name, role: kind });
    const actor = core.createActor(ctx(core, owner), {
      displayName: name,
      role: kind,
      seatId,
    });
    const agentId = `${name}-agent`;
    core.registerAgent(ctx(core, owner), {
      agentId,
      roleName: name,
      seatId,
      actorId: actor.actorId,
    });
    return { agentId, credential: actor.credential, actorId: actor.actorId };
  };
  const pm = member("pm", "PM");
  const developer = member("developer", "Developer");
  core.createSeat(ctx(core, owner), {
    seatId: "loose-seat",
    name: "loose",
    role: "Verifier",
  });
  const loose = core.createActor(ctx(core, owner), {
    displayName: "loose",
    role: "Verifier",
    seatId: "loose-seat",
  });
  const socketPath = path.join(stateDirectory, "control.sock");
  const log: LogEntry[] = [];
  const shutdowns: number[] = [];
  const server = await startDaemonServer({
    socketPath,
    core,
    log: (entry) => log.push(entry),
    onShutdown: () => shutdowns.push(Date.now()),
    commands: createCommandHandlers({
      core,
      controllerCredential: owner,
      ...options.commands,
    }),
  });
  return {
    core,
    stateDirectory,
    socketPath,
    info,
    owner,
    pm,
    developer,
    addMember: member,
    seatOnly: loose.credential,
    log,
    shutdowns,
    server,
  };
}

export async function close(h: Harness): Promise<void> {
  await h.server.close();
  h.core.close();
  removeTempDir(h.stateDirectory);
}

export async function call(
  h: Harness,
  credential: string,
  command: string,
  args: string[] = [],
): Promise<CommandResponse> {
  const result = await callDaemon(h.socketPath, credential, command, args);
  assert.equal(result.kind, "response");
  return (result as { response: CommandResponse }).response;
}

export function rawExchange(
  socketPath: string,
  payload: Buffer,
  timeoutMs = 3000,
): Promise<string> {
  return new Promise((resolve) => {
    const socket = net.createConnection(socketPath);
    let data = "";
    socket.on("connect", () => socket.write(payload));
    socket.on("data", (chunk) => (data += chunk.toString("utf8")));
    socket.on("close", () => resolve(data));
    socket.on("error", () => resolve(data));
    socket.setTimeout(timeoutMs, () => {
      socket.destroy();
      resolve(data);
    });
  });
}
