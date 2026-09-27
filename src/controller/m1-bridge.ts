import fs from "node:fs";
import net from "node:net";
import path from "node:path";
import { randomUUID } from "node:crypto";
import { ControllerError, type ControllerCore } from "./core.js";
import {
  M1_MAX_FRAME_BYTES,
  M1ResponseFrameParser,
  parseM1Frame,
  type M1Frame,
} from "./m1-protocol.js";
import type { BridgeReceipt, MutationContext } from "./types.js";

type BridgeResponse = M1Frame;

function protocolError(message: string): Error {
  return new ControllerError(`M1 bridge protocol: ${message}`);
}

function sameSocket(
  stat: fs.Stats,
  identity: { dev: number; ino: number },
): boolean {
  return (
    stat.isSocket() && stat.dev === identity.dev && stat.ino === identity.ino
  );
}

async function clearStaleSocket(socketPath: string): Promise<void> {
  let initial: fs.Stats;
  try {
    initial = fs.lstatSync(socketPath);
  } catch (error) {
    if (error instanceof Error && "code" in error && error.code === "ENOENT")
      return;
    throw error;
  }
  if (!initial.isSocket())
    throw new ControllerError(
      "M1 bridge socket path exists and is not a Unix socket",
    );
  const completion = Promise.withResolvers<void>();
  const probe = net.createConnection(socketPath);
  probe.once("connect", () => {
    probe.destroy();
    completion.reject(
      new ControllerError("M1 bridge receipt socket is already served"),
    );
  });
  probe.once("error", (error: NodeJS.ErrnoException) => {
    if (error.code !== "ECONNREFUSED" && error.code !== "ENOENT") {
      completion.reject(error);
      return;
    }
    try {
      const current = fs.lstatSync(socketPath);
      if (!sameSocket(current, { dev: initial.dev, ino: initial.ino })) {
        completion.reject(
          new ControllerError(
            "M1 bridge socket path changed during stale-socket check",
          ),
        );
        return;
      }
      fs.unlinkSync(socketPath);
      completion.resolve();
    } catch (unlinkError) {
      if (
        unlinkError instanceof Error &&
        "code" in unlinkError &&
        unlinkError.code === "ENOENT"
      )
        completion.resolve();
      else completion.reject(unlinkError);
    }
  });
  await completion.promise;
}

function validateSocketPath(socketPath: string): string {
  if (!path.isAbsolute(socketPath) || Buffer.byteLength(socketPath) >= 104) {
    throw new TypeError(
      "Unix socket path must be absolute and shorter than 104 bytes",
    );
  }
  return path.resolve(socketPath);
}

function frameObject(buffer: Buffer): BridgeResponse {
  try {
    return parseM1Frame(buffer);
  } catch (error) {
    throw protocolError(
      error instanceof Error ? error.message : "invalid M1 frame",
    );
  }
}

function requestBridge(
  socketPath: string,
  request: BridgeResponse,
): Promise<BridgeResponse> {
  const line = `${JSON.stringify(request)}\n`;
  if (Buffer.byteLength(line) > M1_MAX_FRAME_BYTES)
    return Promise.reject(
      protocolError("bridge request exceeds the frame limit"),
    );
  const completion = Promise.withResolvers<BridgeResponse>();
  const { promise, resolve, reject } = completion;
  const socket = net.createConnection(socketPath);
  const parser = new M1ResponseFrameParser();
  let response: BridgeResponse | undefined;
  let promiseSettled = false;
  const fail = (error: Error): void => {
    socket.destroy();
    if (promiseSettled) return;
    promiseSettled = true;
    clearTimeout(timer);
    reject(error);
  };
  const timer = setTimeout(
    () => fail(protocolError("bridge response timed out")),
    15_000,
  );
  socket.once("connect", () => socket.write(line));
  socket.on("data", (chunk: Buffer) => {
    try {
      const frames = parser.push(chunk);
      response ??= frames[0];
    } catch (error) {
      fail(
        error instanceof Error
          ? protocolError(error.message)
          : protocolError("invalid bridge response"),
      );
    }
  });
  socket.once("error", (error) => fail(error));
  socket.once("end", () => {
    try {
      parser.finish();
      if (!response)
        throw protocolError(
          `bridge closed without a ${String(request.type)} response`,
        );
      if (promiseSettled) return;
      promiseSettled = true;
      clearTimeout(timer);
      resolve(response);
    } catch (error) {
      fail(
        error instanceof Error
          ? protocolError(error.message)
          : protocolError("incomplete bridge frame"),
      );
    }
  });
  socket.once("close", () => {
    if (!promiseSettled)
      fail(
        protocolError(
          `bridge closed without a ${String(request.type)} response`,
        ),
      );
  });
  return promise;
}

export class M1BridgeAdapter {
  static readonly #verifiedSnapshots = new WeakSet<object>();

  static isVerifiedSnapshot(value: unknown): boolean {
    return (
      value !== null &&
      typeof value === "object" &&
      M1BridgeAdapter.#verifiedSnapshots.has(value)
    );
  }

  readonly #core: ControllerCore;
  readonly #receiptSocketPath: string;
  readonly #bridgeSocketPath: string;
  #server: net.Server | undefined;
  #socketIdentity: { dev: number; ino: number } | undefined;

  constructor(
    core: ControllerCore,
    receiptSocketPath: string,
    bridgeSocketPath: string,
  ) {
    this.#core = core;
    this.#receiptSocketPath = validateSocketPath(receiptSocketPath);
    this.#bridgeSocketPath = validateSocketPath(bridgeSocketPath);
    if (this.#receiptSocketPath === this.#bridgeSocketPath)
      throw new TypeError("receipt and command sockets must differ");
  }

  async listen(): Promise<void> {
    if (this.#server)
      throw new ControllerError(
        "M1 bridge receipt server is already listening",
      );
    const directory = fs.lstatSync(path.dirname(this.#receiptSocketPath));
    if (
      !directory.isDirectory() ||
      directory.isSymbolicLink() ||
      (process.getuid && directory.uid !== process.getuid()) ||
      (directory.mode & 0o077) !== 0
    ) {
      throw new ControllerError(
        "M1 bridge receipt socket requires a private directory owned by the current user",
      );
    }
    await clearStaleSocket(this.#receiptSocketPath);
    const server = net.createServer((socket) => this.#receiveReceipt(socket));
    this.#server = server;
    try {
      const listening = Promise.withResolvers<void>();
      server.once("error", listening.reject);
      server.listen(this.#receiptSocketPath, listening.resolve);
      await listening.promise;
      fs.chmodSync(this.#receiptSocketPath, 0o600);
      const stat = fs.lstatSync(this.#receiptSocketPath);
      if (!stat.isSocket())
        throw new ControllerError(
          "M1 bridge receipt path did not create a Unix socket",
        );
      this.#socketIdentity = { dev: stat.dev, ino: stat.ino };
    } catch (error) {
      this.#server = undefined;
      server.close();
      throw error;
    }
  }

  async close(): Promise<void> {
    const server = this.#server;
    this.#server = undefined;
    if (server) {
      const closed = Promise.withResolvers<void>();
      server.close((error) =>
        error ? closed.reject(error) : closed.resolve(),
      );
      await closed.promise;
    }
    const identity = this.#socketIdentity;
    this.#socketIdentity = undefined;
    if (!identity) return;
    try {
      if (sameSocket(fs.lstatSync(this.#receiptSocketPath), identity))
        fs.unlinkSync(this.#receiptSocketPath);
    } catch (error) {
      if (
        !(error instanceof Error) ||
        !("code" in error) ||
        error.code !== "ENOENT"
      )
        throw error;
    }
  }

  async dispatchAndStart(
    context: MutationContext,
    commandId: string,
  ): Promise<{ readonly commandId: string; readonly state: string }> {
    let state = this.#core.commandState(commandId);
    let dispatchReplyVerified = false;
    if (state === "queued" || state === "attempting") {
      const delivery = this.#core.beginCommandDelivery(context, commandId);
      const payload = delivery.payload;
      if (!payload || typeof payload !== "object" || Array.isArray(payload))
        throw protocolError("durable command payload is not an object");
      const response = await requestBridge(
        this.#bridgeSocketPath,
        payload as BridgeResponse,
      );
      if (
        response.type !== "ack" ||
        response.commandId !== commandId ||
        response.durable !== true ||
        response.state !== "acknowledged" ||
        this.#core.commandState(commandId) !== "acknowledged"
      ) {
        throw protocolError(
          "dispatch was not durably acknowledged by both bridge and controller",
        );
      }
      state = "acknowledged";
      dispatchReplyVerified = true;
    }
    if (state === "started" || state === "completed")
      return { commandId, state };
    if (state !== "acknowledged")
      throw protocolError(
        `command is ${state ?? "missing"}; automatic dispatch is unsafe`,
      );
    if (!dispatchReplyVerified) {
      const snapshot = await requestBridge(this.#bridgeSocketPath, {
        type: "get",
        commandId,
      });
      if (
        snapshot.type !== "ack" ||
        snapshot.commandId !== commandId ||
        snapshot.durable !== true ||
        snapshot.state !== "acknowledged"
      )
        throw protocolError(
          "acknowledged command requires a valid same-command M1 get before start",
        );
    }
    const startContextId = randomUUID();
    this.#core.beginCommandStart(
      {
        credential: context.credential,
        requestId: `m1-start-${startContextId}`,
        idempotencyKey: `m1-start-${startContextId}`,
        expectedVersion: this.#core.stateVersion,
        inputRevision: this.#core.inputRevision,
      },
      commandId,
    );
    const response = await requestBridge(this.#bridgeSocketPath, {
      type: "start",
      commandId,
      singleResponse: true,
    });
    if (
      response.type !== "started" ||
      response.commandId !== commandId ||
      response.durable !== true ||
      response.state !== "acknowledged"
    ) {
      throw protocolError(
        "bridge did not durably accept the explicit start request",
      );
    }
    return {
      commandId,
      state: this.#core.commandState(commandId) ?? "unknown",
    };
  }
  async inspectUncertainCommand(commandId: string): Promise<{
    readonly commandId: string;
    readonly bridgeState: string;
    readonly durable: boolean;
  }> {
    if (this.#core.commandState(commandId) !== "unknown")
      throw new ControllerError(
        "only uncertain commands can be inspected without new dispatch",
      );
    const response = await requestBridge(this.#bridgeSocketPath, {
      type: "get",
      commandId,
    });
    if (response.commandId !== commandId)
      throw protocolError("bridge returned a different command identity");
    let snapshot: {
      readonly commandId: string;
      readonly bridgeState: string;
      readonly durable: boolean;
    };
    if (response.type === "completed") {
      snapshot = Object.freeze({
        commandId,
        bridgeState: "completed",
        durable: true,
      });
    } else {
      if (
        response.type !== "ack" ||
        typeof response.state !== "string" ||
        typeof response.durable !== "boolean"
      )
        throw protocolError("bridge returned an invalid command snapshot");
      snapshot = Object.freeze({
        commandId,
        bridgeState: response.state,
        durable: response.durable,
      });
    }
    return snapshot;
  }

  async reconcilePrestartAndContain(
    context: MutationContext,
    assignmentId: string,
    commandId: string,
    proofRef: string,
  ): Promise<{ readonly contained: true }> {
    const snapshot = await this.inspectUncertainCommand(commandId);
    M1BridgeAdapter.#verifiedSnapshots.add(snapshot);
    try {
      return this.#core.confirmContainment(
        context,
        assignmentId,
        proofRef,
        snapshot,
      );
    } finally {
      M1BridgeAdapter.#verifiedSnapshots.delete(snapshot);
    }
  }

  #receiveReceipt(socket: net.Socket): void {
    const chunks: Buffer[] = [];
    let size = 0;
    let settled = false;
    socket.setTimeout(15_000, () => socket.destroy());
    const finish = (): void => {
      if (settled) return;
      settled = true;
      try {
        const value = frameObject(Buffer.concat(chunks, size));
        const result = this.#core.recordBridgeReceipt(value as BridgeReceipt);
        if (!socket.destroyed)
          socket.end(
            `${JSON.stringify({ ok: true, sequence: (value as BridgeReceipt).sequence, duplicate: result.duplicate, ...(result.fenced ? { fenced: true } : {}) })}\n`,
          );
      } catch (error) {
        if (!socket.destroyed)
          socket.end(
            `${JSON.stringify({ ok: false, error: String(error instanceof Error ? error.message : error).slice(0, 2048) })}\n`,
          );
      }
    };
    socket.on("data", (chunk: Buffer) => {
      if (settled) return;
      size += chunk.length;
      if (size > M1_MAX_FRAME_BYTES) {
        socket.destroy();
        return;
      }
      chunks.push(chunk);
      if (chunk.includes(0x0a)) finish();
    });
    socket.once("end", finish);
    socket.on("error", () => socket.destroy());
  }
}
