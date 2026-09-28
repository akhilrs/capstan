import fs from "node:fs";
import net from "node:net";
import path from "node:path";
import { randomUUID, timingSafeEqual } from "node:crypto";
import type { ControllerCore } from "./controller/core.js";

const MAX_FRAME = 16_384;

type Request = { token: string; action: "status" | "inspect"; id?: string };

function sameSecret(actual: unknown, expected: string): boolean {
  if (typeof actual !== "string") return false;
  const left = Buffer.from(actual);
  const right = Buffer.from(expected);
  return left.length === right.length && timingSafeEqual(left, right);
}

function privateSocketDirectory(socketPath: string): void {
  if (!path.isAbsolute(socketPath))
    throw new TypeError("control socket path must be absolute");
  const directory = fs.lstatSync(path.dirname(socketPath));
  if (
    !directory.isDirectory() ||
    directory.isSymbolicLink() ||
    (process.getuid && directory.uid !== process.getuid()) ||
    (directory.mode & 0o077) !== 0
  )
    throw new Error(
      "control socket directory must be private and owned by the current user",
    );
}

export async function listenControl(
  socketPath: string,
  token: string,
  core: ControllerCore,
): Promise<() => Promise<void>> {
  privateSocketDirectory(socketPath);
  try {
    const stat = fs.lstatSync(socketPath);
    if (!stat.isSocket())
      throw new Error("control socket path already exists and is not a socket");
    const probe = net.createConnection(socketPath);
    await new Promise<void>((resolve, reject) => {
      probe.once("connect", () => {
        probe.destroy();
        reject(new Error("another cstan control server is active"));
      });
      probe.once("error", (error: NodeJS.ErrnoException) => {
        if (error.code === "ECONNREFUSED") {
          probe.destroy();
          fs.unlinkSync(socketPath);
          resolve();
        } else reject(error);
      });
    });
  } catch (error) {
    if (
      !(error instanceof Error) ||
      !("code" in error) ||
      error.code !== "ENOENT"
    )
      throw error;
  }
  const server = net.createServer((socket) => {
    let bytes = Buffer.alloc(0);
    socket.on("data", (chunk: Buffer) => {
      bytes = Buffer.concat([bytes, chunk]);
      if (bytes.length > MAX_FRAME) {
        socket.destroy();
        return;
      }
      const newline = bytes.indexOf(10);
      if (newline < 0) return;
      const frame = bytes.subarray(0, newline);
      socket.removeAllListeners("data");
      try {
        const request = JSON.parse(frame.toString("utf8")) as Request;
        if (
          !request ||
          typeof request !== "object" ||
          !sameSecret(request.token, token)
        )
          throw new Error("unauthorized");
        const result =
          request.action === "status" && request.id === undefined
            ? core.statusSnapshot()
            : request.action === "inspect" &&
                typeof request.id === "string" &&
                request.id.length > 0
              ? core.inspect(request.id)
              : (() => {
                  throw new Error("invalid control request");
                })();
        socket.end(`${JSON.stringify({ requestId: randomUUID(), result })}\n`);
      } catch (error) {
        socket.end(
          `${JSON.stringify({ error: error instanceof Error ? error.message : "invalid request" })}\n`,
        );
      }
    });
    socket.setTimeout(5_000, () => socket.destroy());
  });
  const ready = new Promise<void>((resolve, reject) => {
    server.once("error", reject);
    server.listen(socketPath, resolve);
  });
  await ready;
  fs.chmodSync(socketPath, 0o600);
  const identity = fs.lstatSync(socketPath);
  if (!identity.isSocket()) {
    server.close();
    throw new Error("control socket was not created");
  }
  return async () => {
    if (server.listening) {
      const closed = new Promise<void>((resolve, reject) =>
        server.close((error) => (error ? reject(error) : resolve())),
      );
      await closed;
    }
    try {
      const current = fs.lstatSync(socketPath);
      if (
        current.isSocket() &&
        current.dev === identity.dev &&
        current.ino === identity.ino
      )
        fs.unlinkSync(socketPath);
    } catch (error) {
      if (
        !(error instanceof Error) ||
        !("code" in error) ||
        error.code !== "ENOENT"
      )
        throw error;
    }
  };
}

export async function requestControl(
  socketPath: string,
  token: string,
  action: Request["action"],
  id?: string,
): Promise<unknown> {
  return await new Promise((resolve, reject) => {
    const socket = net.createConnection(socketPath);
    let bytes = Buffer.alloc(0);
    socket.once("connect", () =>
      socket.write(
        `${JSON.stringify({ token, action, ...(id ? { id } : {}) })}\n`,
      ),
    );
    socket.on("data", (chunk: Buffer) => {
      bytes = Buffer.concat([bytes, chunk]);
      if (bytes.length > MAX_FRAME) {
        socket.destroy();
        reject(new Error("control response exceeds limit"));
        return;
      }
      const newline = bytes.indexOf(10);
      if (newline < 0) return;
      try {
        const response = JSON.parse(
          bytes.subarray(0, newline).toString("utf8"),
        ) as { result?: unknown; error?: string };
        socket.end();
        if (response.error) reject(new Error(response.error));
        else resolve(response.result);
      } catch (error) {
        reject(error);
      }
    });
    socket.once("error", reject);
    socket.setTimeout(5_000, () => {
      socket.destroy();
      reject(new Error("control request timed out"));
    });
  });
}
