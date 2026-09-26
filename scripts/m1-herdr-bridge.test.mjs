import assert from "node:assert/strict";
import fs from "node:fs";
import net from "node:net";
import os from "node:os";
import path from "node:path";
import herdrBridge from "./m1-herdr-bridge.mjs";

function listen(server, socketPath) {
  return new Promise((resolve, reject) => {
    server.once("error", reject);
    server.listen(socketPath, () => { server.off("error", reject); resolve(); });
  });
}

function request(socketPath, message) {
  return new Promise((resolve, reject) => {
    const socket = net.createConnection(socketPath);
    let pending = Buffer.alloc(0);
    socket.once("connect", () => socket.write(`${JSON.stringify(message)}\n`));
    socket.on("data", (chunk) => {
      pending = Buffer.concat([pending, chunk]);
      const newline = pending.indexOf(0x0a);
      if (newline < 0) return;
      const response = JSON.parse(pending.subarray(0, newline).toString("utf8"));
      socket.end();
      resolve(response);
    });
    socket.once("error", reject);
  });
}

const temp = fs.mkdtempSync(path.join(os.tmpdir(), "m1-bridge-dispatch-error-"));
const bridgeSocket = path.join(temp, "bridge.sock");
const receiptSocket = path.join(temp, "receipt.sock");
const journal = path.join(temp, "bridge.jsonl");
const journalFd = fs.openSync(journal, "wx", 0o600);
const previousEnv = Object.fromEntries(["CAPSTAN_BRIDGE_ROLE", "CAPSTAN_BRIDGE_SOCKET", "CAPSTAN_BRIDGE_JOURNAL", "CAPSTAN_BRIDGE_RECEIPT_SOCKET"].map((key) => [key, process.env[key]]));
let receiptSequence = 0;
let sendCount = 0;
let dispatchErrorSeen;
const dispatchErrorDurable = new Promise((resolve) => { dispatchErrorSeen = resolve; });
const receiptServer = net.createServer((socket) => {
  let pending = Buffer.alloc(0);
  socket.on("data", (chunk) => {
    pending = Buffer.concat([pending, chunk]);
    const newline = pending.indexOf(0x0a);
    if (newline < 0) return;
    const entry = JSON.parse(pending.subarray(0, newline).toString("utf8"));
    assert.equal(entry.sequence, receiptSequence + 1);
    const bytes = Buffer.from(`${JSON.stringify(entry)}\n`);
    fs.writeSync(journalFd, bytes);
    fs.fsyncSync(journalFd);
    receiptSequence = entry.sequence;
    socket.end(`${JSON.stringify({ ok: true, sequence: receiptSequence })}\n`);
    if (entry.type === "dispatch_error") dispatchErrorSeen();
  });
});

const handlers = new Map();
const pi = {
  on(event, handler) { handlers.set(event, handler); },
  sendUserMessage() { sendCount++; throw new Error("injected dispatch failure"); },
};

try {
  Object.assign(process.env, {
    CAPSTAN_BRIDGE_ROLE: "PM",
    CAPSTAN_BRIDGE_SOCKET: bridgeSocket,
    CAPSTAN_BRIDGE_JOURNAL: journal,
    CAPSTAN_BRIDGE_RECEIPT_SOCKET: receiptSocket,
  });
  await listen(receiptServer, receiptSocket);
  herdrBridge(pi);
  await handlers.get("session_start")({}, { isIdle: () => true, abort() {} });
  const command = { type: "dispatch", commandId: "dispatch-fails", assignmentId: "assignment-1", attempt: 1, generation: 1, prompt: "run once" };
  assert.deepEqual(await request(bridgeSocket, command), { type: "ack", commandId: command.commandId, durable: true, state: "acknowledged" });
  await dispatchErrorDurable;
  let status;
  for (let attempt = 0; attempt < 20; attempt++) {
    status = await request(bridgeSocket, { type: "get", commandId: command.commandId });
    if (status.state === "unknown") break;
    await new Promise((resolve) => setTimeout(resolve, 5));
  }
  assert.equal(status.state, "unknown", "durably failed dispatch must not remain acknowledged");
  assert.equal((await request(bridgeSocket, command)).state, "unknown", "duplicate dispatch must reconcile, not resend");
  assert.equal((await request(bridgeSocket, { type: "abort", commandId: command.commandId })).state, "unknown");
  assert.equal(sendCount, 1, "the failed dispatch must never be blindly executed a second time");
  assert.deepEqual(fs.readFileSync(journal, "utf8").trim().split("\n").map((line) => JSON.parse(line).type), ["accepted", "dispatch_error"]);
  await handlers.get("session_shutdown")();
  console.log("PASS dispatch_error becomes unknown after durable receipt; get/retry/abort do not re-execute");
} finally {
  for (const [key, value] of Object.entries(previousEnv)) {
    if (value === undefined) delete process.env[key]; else process.env[key] = value;
  }
  await new Promise((resolve) => receiptServer.close(resolve));
  fs.closeSync(journalFd);
  fs.rmSync(temp, { recursive: true, force: true });
}
