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
async function waitForJournal(file, expectedTypes) {
  for (let attempt = 0; attempt < 100; attempt++) {
    if (fs.existsSync(file)) {
      const rows = fs.readFileSync(file, "utf8").trim().split("\n").filter(Boolean).map((line) => JSON.parse(line));
      if (rows.map((row) => row.type).join(",") === expectedTypes.join(",")) return rows;
    }
    await new Promise((resolve) => setTimeout(resolve, 5));
  }
  assert.fail(`timed out waiting for durable journal records: ${expectedTypes.join(",")}`);
}

const temp = fs.mkdtempSync(path.join(os.tmpdir(), "m1-bridge-dispatch-error-"));
const bridgeSocket = path.join(temp, "bridge.sock");
const receiptSocket = path.join(temp, "receipt.sock");
const journal = path.join(temp, "bridge.jsonl");
let journalFd = fs.openSync(journal, "wx", 0o600);
const previousEnv = Object.fromEntries(["CAPSTAN_BRIDGE_ROLE", "CAPSTAN_BRIDGE_SOCKET", "CAPSTAN_BRIDGE_JOURNAL", "CAPSTAN_BRIDGE_RECEIPT_SOCKET"].map((key) => [key, process.env[key]]));
let receiptSequence = 0;
let sendCount = 0;
let failDispatchError = false;
let failSubmitted = false;
let failWorking = false;
let failCompleted = false;
let authorizeAbort = false;
let failSend = true;
let dispatchErrorSeen;
let dispatchErrorDurable = new Promise((resolve) => { dispatchErrorSeen = resolve; });
const receiptServer = net.createServer((socket) => {
  let pending = Buffer.alloc(0);
  socket.on("data", (chunk) => {
    pending = Buffer.concat([pending, chunk]);
    const newline = pending.indexOf(0x0a);
    if (newline < 0) return;
    const entry = JSON.parse(pending.subarray(0, newline).toString("utf8"));
    assert.equal(entry.sequence, receiptSequence + 1);
    if (entry.type === "dispatch_error" && failDispatchError) {
      socket.end(`${JSON.stringify({ ok: false, sequence: receiptSequence, error: "injected durable receipt failure" })}\n`);
      dispatchErrorSeen();
      return;
    }
    if ((entry.type === "working" && failWorking) || (entry.type === "submitted" && failSubmitted) || (entry.type === "completed" && failCompleted)) {
      socket.end(`${JSON.stringify({ ok: false, sequence: receiptSequence, error: `injected ${entry.type} receipt failure` })}\n`);
      return;
    }
    if (entry.type === "aborted" && !authorizeAbort) {
      socket.end(`${JSON.stringify({ ok: false, sequence: receiptSequence, error: "abort lacks durable controller authorization" })}\n`);
      return;
    }
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
  sendUserMessage() {
    sendCount++;
    if (failSend) throw new Error("injected dispatch failure");
  },
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
  const failedBridgeSocket = path.join(temp, "bridge-receipt-fails.sock");
  const failedJournal = path.join(temp, "bridge-receipt-fails.jsonl");
  fs.closeSync(journalFd);
  journalFd = fs.openSync(failedJournal, "wx", 0o600);
  receiptSequence = 0;
  failDispatchError = true;
  dispatchErrorDurable = new Promise((resolve) => { dispatchErrorSeen = resolve; });
  Object.assign(process.env, { CAPSTAN_BRIDGE_SOCKET: failedBridgeSocket, CAPSTAN_BRIDGE_JOURNAL: failedJournal });
  handlers.clear();
  herdrBridge(pi);
  await handlers.get("session_start")({}, { isIdle: () => true, abort() {} });
  const failedReceiptCommand = { ...command, commandId: "dispatch-receipt-fails", assignmentId: "assignment-2" };
  assert.equal((await request(failedBridgeSocket, failedReceiptCommand)).state, "acknowledged");
  await dispatchErrorDurable;
  assert.equal((await request(failedBridgeSocket, { type: "get", commandId: failedReceiptCommand.commandId })).state, "unknown");
  assert.equal((await request(failedBridgeSocket, failedReceiptCommand)).state, "unknown");
  const nextCommand = { ...failedReceiptCommand, commandId: "new-command", assignmentId: "assignment-3" };
  assert.equal((await request(failedBridgeSocket, nextCommand)).type, "error", "an unresolved failed dispatch must block replacement work");
  assert.equal(sendCount, 2, "neither failed command may be submitted twice");
  assert.deepEqual(fs.readFileSync(failedJournal, "utf8").trim().split("\n").map((line) => JSON.parse(line).type), ["accepted"]);
  await handlers.get("session_shutdown")();
  const recoveredFailedBridgeSocket = path.join(temp, "bridge-receipt-fails-recovered.sock");
  fs.closeSync(journalFd);
  journalFd = fs.openSync(failedJournal, "a", 0o600);
  Object.assign(process.env, { CAPSTAN_BRIDGE_SOCKET: recoveredFailedBridgeSocket });
  handlers.clear();
  herdrBridge(pi);
  await handlers.get("session_start")({}, { isIdle: () => true, abort() {} });
  const sendsBeforeRecoveryRetry = sendCount;
  assert.equal((await request(recoveredFailedBridgeSocket, { type: "get", commandId: failedReceiptCommand.commandId })).state, "unknown");
  assert.equal((await request(recoveredFailedBridgeSocket, failedReceiptCommand)).state, "unknown");
  assert.equal((await request(recoveredFailedBridgeSocket, nextCommand)).type, "error", "recovered accepted-without-terminal work must keep the slot locked");
  assert.equal(sendCount, sendsBeforeRecoveryRetry, "recovered accepted work must never be blindly resent");
  await handlers.get("session_shutdown")();
  const workingBridgeSocket = path.join(temp, "bridge-working.sock");
  const workingJournal = path.join(temp, "bridge-working.jsonl");
  fs.closeSync(journalFd);
  journalFd = fs.openSync(workingJournal, "wx", 0o600);
  receiptSequence = 0;
  failDispatchError = false;
  failSend = false;
  Object.assign(process.env, { CAPSTAN_BRIDGE_SOCKET: workingBridgeSocket, CAPSTAN_BRIDGE_JOURNAL: workingJournal });
  handlers.clear();
  herdrBridge(pi);
  await handlers.get("session_start")({}, { isIdle: () => true, abort() {} });
  const workingCommand = { ...command, commandId: "dispatch-working", assignmentId: "assignment-4" };
  await request(workingBridgeSocket, workingCommand);
  await handlers.get("agent_start")();
  const workingRows = await waitForJournal(workingJournal, ["accepted", "submitted", "working"]);
  assert.equal(workingRows.at(-1).commandId, workingCommand.commandId);
  assert.equal((await request(workingBridgeSocket, { type: "get", commandId: workingCommand.commandId })).state, "working");
  const sendsWhileWorking = sendCount;
  assert.equal((await request(workingBridgeSocket, workingCommand)).state, "working", "duplicate dispatch must expose working state");
  assert.equal(sendCount, sendsWhileWorking, "duplicate dispatch while working must not resend");
  handlers.get("turn_end")({ message: { role: "assistant", content: [{ type: "text", text: "CURRENT_RESULT" }] } });
  await handlers.get("agent_end")({ willContinue: false, messages: [{ role: "assistant", content: [{ type: "text", text: "STALE_RESULT" }] }] });
  const completedRows = await waitForJournal(workingJournal, ["accepted", "submitted", "working", "completed"]);
  assert.equal(completedRows.at(-1).reply, "CURRENT_RESULT", "completion must use this run's turn_end, not stale history");
  assert.equal((await request(workingBridgeSocket, { type: "get", commandId: workingCommand.commandId })).reply, "CURRENT_RESULT");
  await handlers.get("session_shutdown")();

  const unknownBridgeSocket = path.join(temp, "bridge-unknown.sock");
  const unknownJournal = path.join(temp, "bridge-unknown.jsonl");
  fs.closeSync(journalFd);
  journalFd = fs.openSync(unknownJournal, "wx", 0o600);
  receiptSequence = 0;
  Object.assign(process.env, { CAPSTAN_BRIDGE_SOCKET: unknownBridgeSocket, CAPSTAN_BRIDGE_JOURNAL: unknownJournal });
  handlers.clear();
  herdrBridge(pi);
  await handlers.get("session_start")({}, { isIdle: () => true, abort() {} });
  const unknownCommand = { ...command, commandId: "dispatch-no-current-reply", assignmentId: "assignment-5" };
  await request(unknownBridgeSocket, unknownCommand);
  await handlers.get("agent_start")();
  handlers.get("turn_end")({ message: { role: "assistant", content: [{ type: "text", text: " \t\n " }] } });
  await handlers.get("agent_end")({ willContinue: false, messages: [{ role: "assistant", content: [{ type: "text", text: "STALE_RESULT" }] }] });
  const unknownRows = await waitForJournal(unknownJournal, ["accepted", "submitted", "working", "agent_end_without_reply"]);
  assert.equal(unknownRows.at(-1).commandId, unknownCommand.commandId);
  assert.equal((await request(unknownBridgeSocket, { type: "abort", commandId: unknownCommand.commandId })).type, "error",
    "an untrusted abort cannot terminalize an uncertain assignment");
  assert.equal((await request(unknownBridgeSocket, unknownCommand)).state, "unknown");
  const afterUnknown = { ...unknownCommand, commandId: "dispatch-after-unknown", assignmentId: "assignment-6" };
  assert.equal((await request(unknownBridgeSocket, afterUnknown)).type, "error", "unknown work must retain the active slot");
  await handlers.get("session_shutdown")();

  const recoveredBridgeSocket = path.join(temp, "bridge-unknown-recovered.sock");
  fs.closeSync(journalFd);
  journalFd = fs.openSync(unknownJournal, "a", 0o600);
  Object.assign(process.env, { CAPSTAN_BRIDGE_SOCKET: recoveredBridgeSocket });
  handlers.clear();
  herdrBridge(pi);
  await handlers.get("session_start")({}, { isIdle: () => true, abort() { throw new Error("injected abort failure"); } });
  assert.equal((await request(recoveredBridgeSocket, { type: "get", commandId: unknownCommand.commandId })).state, "unknown");
  assert.equal((await request(recoveredBridgeSocket, afterUnknown)).type, "error", "recovered unknown work must remain locked");
  assert.equal((await request(recoveredBridgeSocket, { type: "abort", commandId: unknownCommand.commandId })).type, "error",
    "recovered unknown work requires durable controller authorization before release");
  authorizeAbort = true;
  assert.equal((await request(recoveredBridgeSocket, { type: "abort", commandId: unknownCommand.commandId })).state, "unknown");
  await waitForJournal(unknownJournal, ["accepted", "submitted", "working", "agent_end_without_reply", "aborted"]);
  assert.equal((await request(recoveredBridgeSocket, afterUnknown)).type, "error", "abort acknowledgement cannot establish worker containment");
  assert.equal((await request(recoveredBridgeSocket, { type: "abort", commandId: unknownCommand.commandId })).state, "unknown");
  await handlers.get("session_shutdown")();

  const abortedBridgeSocket = path.join(temp, "bridge-aborted-recovered.sock");
  Object.assign(process.env, { CAPSTAN_BRIDGE_SOCKET: abortedBridgeSocket });
  handlers.clear();
  herdrBridge(pi);
  await handlers.get("session_start")({}, { isIdle: () => true, abort() {} });
  assert.equal((await request(abortedBridgeSocket, afterUnknown)).type, "error", "restart cannot reuse a seat after abort without containment");
  await handlers.get("session_shutdown")();

  async function isolated(label) {
    const socket = path.join(temp, `${label}.sock`);
    const file = path.join(temp, `${label}.jsonl`);
    fs.closeSync(journalFd);
    journalFd = fs.openSync(file, "wx", 0o600);
    receiptSequence = 0;
    Object.assign(process.env, { CAPSTAN_BRIDGE_SOCKET: socket, CAPSTAN_BRIDGE_JOURNAL: file });
    handlers.clear();
    herdrBridge(pi);
    await handlers.get("session_start")({}, { isIdle: () => true, abort() {} });
    return { socket, file };
  }

  const workingFailure = await isolated("bridge-working-failure");
  const workingUnknown = { ...unknownCommand, commandId: "dispatch-working-receipt-fails", assignmentId: "assignment-6" };
  await request(workingFailure.socket, workingUnknown);
  await waitForJournal(workingFailure.file, ["accepted", "submitted"]);
  failWorking = true;
  await handlers.get("agent_start")();
  assert.equal((await request(workingFailure.socket, { type: "get", commandId: workingUnknown.commandId })).state, "unknown");
  assert.equal((await request(workingFailure.socket, afterUnknown)).type, "error");
  await handlers.get("session_shutdown")();
  failWorking = false;
  const recoveredWorkingSocket = path.join(temp, "bridge-working-recovered.sock");
  Object.assign(process.env, { CAPSTAN_BRIDGE_SOCKET: recoveredWorkingSocket });
  handlers.clear();
  herdrBridge(pi);
  await handlers.get("session_start")({}, { isIdle: () => true, abort() {} });
  assert.equal((await request(recoveredWorkingSocket, afterUnknown)).type, "error", "failed working receipt keeps replay locked");
  await handlers.get("session_shutdown")();

  const completionFailure = await isolated("bridge-completion-failure");
  const completedUnknown = { ...unknownCommand, commandId: "dispatch-completion-receipt-fails", assignmentId: "assignment-7" };
  await request(completionFailure.socket, completedUnknown);
  await handlers.get("agent_start")();
  await waitForJournal(completionFailure.file, ["accepted", "submitted", "working"]);
  failCompleted = true;
  handlers.get("turn_end")({ message: { role: "assistant", content: [{ type: "text", text: "UNPERSISTED_RESULT" }] } });
  await handlers.get("agent_end")({ willContinue: false, messages: [] });
  assert.equal((await request(completionFailure.socket, { type: "get", commandId: completedUnknown.commandId })).state, "unknown");
  const afterCompletionFailure = { ...completedUnknown, commandId: "dispatch-after-completion-failure", assignmentId: "assignment-8" };
  assert.equal((await request(completionFailure.socket, afterCompletionFailure)).type, "error");
  await handlers.get("session_shutdown")();
  failCompleted = false;
  const recoveredCompletionBridgeSocket = path.join(temp, "bridge-completion-recovered.sock");
  Object.assign(process.env, { CAPSTAN_BRIDGE_SOCKET: recoveredCompletionBridgeSocket });
  handlers.clear();
  herdrBridge(pi);
  await handlers.get("session_start")({}, { isIdle: () => true, abort() {} });
  assert.equal((await request(recoveredCompletionBridgeSocket, afterCompletionFailure)).type, "error");
  await handlers.get("session_shutdown")();

  const submittedFailure = await isolated("bridge-submitted-failure");
  failSubmitted = true;
  const submittedUnknown = { ...unknownCommand, commandId: "dispatch-submitted-receipt-fails", assignmentId: "assignment-9" };
  const sendsBeforeSubmittedFailure = sendCount;
  assert.equal((await request(submittedFailure.socket, submittedUnknown)).state, "acknowledged");
  await waitForJournal(submittedFailure.file, ["accepted"]);
  assert.equal(sendCount, sendsBeforeSubmittedFailure + 1);
  assert.equal((await request(submittedFailure.socket, submittedUnknown)).state, "unknown");
  const afterSubmittedFailure = { ...submittedUnknown, commandId: "dispatch-after-submitted-failure", assignmentId: "assignment-10" };
  assert.equal((await request(submittedFailure.socket, afterSubmittedFailure)).type, "error");
  await handlers.get("session_shutdown")();
  failSubmitted = false;
  const recoveredSubmittedBridgeSocket = path.join(temp, "bridge-submitted-recovered.sock");
  Object.assign(process.env, { CAPSTAN_BRIDGE_SOCKET: recoveredSubmittedBridgeSocket });
  handlers.clear();
  herdrBridge(pi);
  await handlers.get("session_start")({}, { isIdle: () => true, abort() {} });
  const sendsBeforeRestartQuery = sendCount;
  assert.equal((await request(recoveredSubmittedBridgeSocket, submittedUnknown)).state, "unknown");
  assert.equal((await request(recoveredSubmittedBridgeSocket, afterSubmittedFailure)).type, "error");
  assert.equal(sendCount, sendsBeforeRestartQuery, "recovered ambiguous send must never be blindly resubmitted");
  await handlers.get("session_shutdown")();
  console.log("PASS current-turn binding, ambiguous receipt failures, and fail-closed recovery");
} finally {
  for (const [key, value] of Object.entries(previousEnv)) {
    if (value === undefined) delete process.env[key]; else process.env[key] = value;
  }
  await new Promise((resolve) => receiptServer.close(resolve));
  fs.closeSync(journalFd);
  fs.rmSync(temp, { recursive: true, force: true });
}
