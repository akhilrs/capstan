import net from "node:net";
import fs from "node:fs";
import path from "node:path";
import { TextDecoder } from "node:util";

const MAX_LINE_BYTES = 1_048_576;
const MAX_PROMPT_BYTES = 262_144;
const ID_PATTERN = /^[A-Za-z0-9][A-Za-z0-9._:-]{0,127}$/;
const ROLES = new Set(["PM", "Developer", "Verifier", "Supervisor"]);
const decoder = new TextDecoder("utf-8", { fatal: true, ignoreBOM: true });

function fail(message) {
  const error = new Error(message);
  error.name = "BridgeProtocolError";
  throw error;
}

function validId(value, name) {
  if (typeof value !== "string" || !ID_PATTERN.test(value)) fail(`invalid ${name}`);
  return value;
}

function positiveInteger(value, name) {
  if (!Number.isSafeInteger(value) || value < 1) fail(`invalid ${name}`);
  return value;
}

function byteLength(value) {
  return Buffer.byteLength(value, "utf8");
}

function syncDirectory(directory) {
  const fd = fs.openSync(directory, "r");
  try { fs.fsyncSync(fd); } finally { fs.closeSync(fd); }
}
async function clearStaleSocket(socketPath) {
  let stat;
  try { stat = fs.lstatSync(socketPath); } catch (error) { if (error?.code === "ENOENT") return; throw error; }
  if (!stat.isSocket()) throw new Error("bridge socket path exists and is not a Unix socket");
  await new Promise((resolve, reject) => {
    const probe = net.createConnection(socketPath);
    probe.once("connect", () => {
      probe.destroy();
      reject(new Error("bridge socket is already served by another process"));
    });
    probe.once("error", (error) => {
      if (error.code !== "ECONNREFUSED" && error.code !== "ENOENT") { reject(error); return; }
      try {
        if (fs.lstatSync(socketPath).isSocket()) fs.unlinkSync(socketPath);
        resolve();
      } catch (unlinkError) { if (unlinkError?.code === "ENOENT") resolve(); else reject(unlinkError); }
    });
  });
}

function recoverJournal(file, role) {
  const existed = fs.existsSync(file);
  fs.mkdirSync(path.dirname(file), { recursive: true, mode: 0o700 });
  let fd = fs.openSync(file, "a+", 0o600);
  if (!existed) syncDirectory(path.dirname(file));
  let bytes;
  try { bytes = fs.readFileSync(fd); } finally { fs.closeSync(fd); }
  const rows = new Map();
  let offset = 0;
  let goodEnd = 0;
  let sequence = 0;
  while (offset < bytes.length) {
    const newline = bytes.indexOf(0x0a, offset);
    if (newline < 0) break;
    const line = bytes.subarray(offset, newline);
    let entry;
    try {
      entry = JSON.parse(decoder.decode(line));
    } catch {
      throw new Error(`invalid complete journal record at byte ${offset}`);
    }
    if (!entry || typeof entry !== "object" || Array.isArray(entry) || !Number.isSafeInteger(entry.sequence) || entry.sequence !== sequence + 1 || typeof entry.type !== "string" || entry.role !== role) {
      throw new Error(`invalid journal record at byte ${offset}`);
    }
    sequence = entry.sequence;
    if (entry.type === "accepted") {
      validId(entry.commandId, "journal commandId");
      if (rows.has(entry.commandId) || typeof entry.assignmentId !== "string" || !Number.isSafeInteger(entry.attempt) || !Number.isSafeInteger(entry.generation) || typeof entry.prompt !== "string") throw new Error(`invalid accepted journal record at byte ${offset}`);
      rows.set(entry.commandId, { accepted: entry, state: "acknowledged" });
    } else if (entry.type === "submitted" || entry.type === "working" || entry.type === "tool_started" || entry.type === "tool_completed" || entry.type === "aborted" || entry.type === "dispatch_error") {
      const row = rows.get(entry.commandId);
      if (!row) throw new Error(`orphan journal record at byte ${offset}`);
      if (row.state !== "completed") row.state = entry.type === "aborted" || entry.type === "dispatch_error" ? "unknown" : "working";
    } else if (entry.type === "completed") {
      const row = rows.get(entry.commandId);
      if (!row || typeof entry.reply !== "string" || !entry.evidenceRef) throw new Error(`invalid completion journal record at byte ${offset}`);
      row.state = "completed";
      row.result = entry.reply;
      row.evidenceRef = entry.evidenceRef;
    } else {
      throw new Error(`unknown journal record type at byte ${offset}`);
    }
    goodEnd = newline + 1;
    offset = newline + 1;
  }
  if (goodEnd !== bytes.length) {
    fd = fs.openSync(file, "r+");
    try { fs.ftruncateSync(fd, goodEnd); fs.fsyncSync(fd); } finally { fs.closeSync(fd); }
  }
  return { rows, sequence };
}

export default function herdrBridge(pi) {
  // The factory is registration-only: no socket, filesystem, or runtime actions here.
  let server;
  let ctxRef;
  let journalPath;
  let socketPath;
  let role;
  let sequence = 0;
  let rows = new Map();
  let active = null;
  let agentStarted = false;
  let shuttingDown = false;
  let queue = Promise.resolve();
  let clients = new Set();
  let commandSockets = new Map();

  const enqueue = (fn) => {
    const next = queue.then(fn, fn);
    queue = next.catch((error) => {
      console.error(`[m1-herdr-bridge] ${error?.message ?? error}`);
    });
    return next;
  };

  function append(type, fields) {
    const entry = { sequence: ++sequence, timestamp: new Date().toISOString(), type, role, ...fields };
    const line = `${JSON.stringify(entry)}\n`;
    const fd = fs.openSync(journalPath, "a", 0o600);
    try {
      const bytes = Buffer.from(line);
      for (let offset = 0; offset < bytes.length;) {
        const written = fs.writeSync(fd, bytes, offset, bytes.length - offset);
        if (written <= 0) throw new Error("Bridge journal write made no progress");
        offset += written;
      }
      fs.fsyncSync(fd);
    } finally { fs.closeSync(fd); }
    return entry;
  }

  function evidence(entry) {
    return Object.freeze({ journal: journalPath, sequence: entry.sequence });
  }

  function snapshot(commandId) {
    const row = rows.get(commandId);
    if (!row) return { type: "ack", commandId, durable: true, state: "unknown" };
    if (row.state === "completed") return { type: "completed", ...identity(row.accepted), reply: row.result, evidenceRef: row.evidenceRef };
    return { type: "ack", commandId, durable: true, state: row.state };
  }

  function sameCommand(row, request) {
    const old = row.accepted;
    return old.assignmentId === request.assignmentId && old.attempt === request.attempt && old.generation === request.generation && old.prompt === request.prompt;
  }

  function requestShape(value) {
    if (!value || typeof value !== "object" || Array.isArray(value)) fail("request must be an object");
    if (value.type === "dispatch") {
      if (Object.keys(value).some((key) => !["type", "commandId", "assignmentId", "attempt", "generation", "prompt"].includes(key))) fail("unexpected dispatch field");
      const commandId = validId(value.commandId, "commandId");
      const assignmentId = validId(value.assignmentId, "assignmentId");
      const attempt = positiveInteger(value.attempt, "attempt");
      const generation = positiveInteger(value.generation, "generation");
      if (typeof value.prompt !== "string" || !value.prompt.trim() || byteLength(value.prompt) > MAX_PROMPT_BYTES || Buffer.from(value.prompt, "utf8").toString("utf8") !== value.prompt) fail("invalid prompt");
      return { type: "dispatch", commandId, assignmentId, attempt, generation, prompt: value.prompt };
    }
    if (value.type === "get" || value.type === "abort") {
      if (Object.keys(value).some((key) => !["type", "commandId"].includes(key))) fail("unexpected request field");
      return { type: value.type, commandId: validId(value.commandId, "commandId") };
    }
    fail("unsupported request type");
  }

  async function handle(request) {
    if (request.type === "get") return snapshot(request.commandId);
    const row = rows.get(request.commandId);
    if (request.type === "abort") {
      if (!row) return { type: "ack", commandId: request.commandId, durable: true, state: "unknown" };
      if (row.state === "completed" || row.state === "unknown") return snapshot(request.commandId);
      if (active?.commandId === request.commandId) {
        append("aborted", { ...identity(active) });
        row.state = "unknown";
        active = null;
        try { ctxRef.abort(); } catch (error) { console.error(`[m1-herdr-bridge] abort failed: ${error?.message ?? error}`); }
      }
      return snapshot(request.commandId);
    }
    if (row) {
      if (!sameCommand(row, request)) fail("commandId payload mismatch");
      return snapshot(request.commandId);
    }
    if (active) fail("another assignment is active");
    if (!ctxRef?.isIdle?.()) fail("OMP session is not idle for a new assignment");
    const accepted = append("accepted", { commandId: request.commandId, assignmentId: request.assignmentId, attempt: request.attempt, generation: request.generation, prompt: request.prompt });
    const newRow = { accepted, state: "acknowledged" };
    rows.set(request.commandId, newRow);
    active = { commandId: request.commandId, assignmentId: request.assignmentId, attempt: request.attempt, generation: request.generation, prompt: request.prompt };
    // Return only after durable receipt. Dispatch is initiated after response is queued by caller.
    return { type: "ack", commandId: request.commandId, durable: true, state: "acknowledged", _dispatch: true };
  }

  function identity(command) {
    return { commandId: command.commandId, assignmentId: command.assignmentId, attempt: command.attempt, generation: command.generation };
  }

  function reply(socket, payload) {
    const { _dispatch, ...wire } = payload;
    if (!socket.destroyed) socket.write(`${JSON.stringify(wire)}\n`);
    if (_dispatch) enqueue(async () => {
      if (shuttingDown || active?.commandId !== payload.commandId) return;
      try {
        pi.sendUserMessage(active.prompt);
        append("submitted", { ...identity(active) });
      } catch (error) {
        // Preserve the accepted identity: a restart/retry must reconcile, never submit it blindly.
        append("dispatch_error", { ...identity(active), error: String(error?.message ?? error).slice(0, 2048) });
        rows.get(active.commandId).state = "unknown";
        active = null;
        console.error(`[m1-herdr-bridge] dispatch failed: ${error?.message ?? error}`);
      }
    }).catch((error) => {
      console.error(`[m1-herdr-bridge] dispatch failure: ${error?.message ?? error}`);
    });
  }

  function serveSocket(socket) {
    clients.add(socket);
    socket.once("close", () => clients.delete(socket));
    let pending = Buffer.alloc(0);
    let failed = false;
    socket.on("data", (chunk) => {
      if (failed) return;
      pending = pending.length ? Buffer.concat([pending, chunk]) : chunk;
      let at;
      while ((at = pending.indexOf(0x0a)) !== -1) {
        const line = pending.subarray(0, at);
        pending = pending.subarray(at + 1);
        if (!line.length || line.length > MAX_LINE_BYTES) {
          failed = true; socket.end(`${JSON.stringify({ type: "error", error: "invalid request line" })}\n`); return;
        }
        try {
          const parsed = JSON.parse(decoder.decode(line));
          const normalized = requestShape(parsed);
          enqueue(() => handle(normalized)).then((response) => {
            if (normalized.type === "dispatch") {
              let subscribers = commandSockets.get(normalized.commandId);
              if (!subscribers) commandSockets.set(normalized.commandId, (subscribers = new Set()));
              subscribers.add(socket);
              socket.once("close", () => subscribers.delete(socket));
            }
            reply(socket, response);
          }).catch((error) => {
            if (!socket.destroyed) socket.write(`${JSON.stringify({ type: "error", error: String(error?.message ?? error).slice(0, 2048) })}\n`);
          });
        } catch (error) {
          failed = true;
          socket.end(`${JSON.stringify({ type: "error", error: String(error?.message ?? error).slice(0, 2048) })}\n`);
          return;
        }
      }
      if (pending.length > MAX_LINE_BYTES) {
        failed = true; socket.end(`${JSON.stringify({ type: "error", error: "request line too large" })}\n`);
      }
    });
    socket.on("error", (error) => console.error(`[m1-herdr-bridge] client socket: ${error.message}`));
  }

  const enqueueEvent = (fn) => enqueue(fn).catch((error) => {
    console.error(`[m1-herdr-bridge] event failed: ${error?.message ?? error}`);
  });

  pi.on("session_start", async (_event, ctx) => {
    ctxRef = ctx;
    role = process.env.CAPSTAN_BRIDGE_ROLE;
    socketPath = process.env.CAPSTAN_BRIDGE_SOCKET;
    journalPath = process.env.CAPSTAN_BRIDGE_JOURNAL;
    if (!ROLES.has(role)) throw new Error("CAPSTAN_BRIDGE_ROLE must be PM, Developer, Verifier, or Supervisor");
    if (typeof socketPath !== "string" || !path.isAbsolute(socketPath) || typeof journalPath !== "string" || !path.isAbsolute(journalPath)) throw new Error("CAPSTAN_BRIDGE_SOCKET and CAPSTAN_BRIDGE_JOURNAL must be absolute paths");
    const recovered = recoverJournal(journalPath, role);
    rows = recovered.rows;
    sequence = recovered.sequence;
    // Anything accepted without durable completion before this process came up is unknown.
    for (const item of rows.values()) if (item.state !== "completed") item.state = "unknown";
    await clearStaleSocket(socketPath);
    server = net.createServer(serveSocket);
    await new Promise((resolve, reject) => {
      const onError = (error) => { server.off("listening", onListening); reject(error); };
      const onListening = () => { server.off("error", onError); resolve(); };
      server.once("error", onError);
      server.once("listening", onListening);
      server.listen(socketPath);
    });
    server.on("error", (error) => console.error(`[m1-herdr-bridge] server socket: ${error.message}`));
    fs.chmodSync(socketPath, 0o600);
  });

  pi.on("agent_start", async () => enqueueEvent(() => {
    agentStarted = true;
    if (!active) return;
    append("working", { ...identity(active) });
    rows.get(active.commandId).state = "working";
    const workingAck = { type: "ack", commandId: active.commandId, durable: true, state: "working" };
    for (const socket of commandSockets.get(active.commandId) ?? []) if (!socket.destroyed) socket.write(`${JSON.stringify(workingAck)}\n`);
  }));

  pi.on("tool_execution_start", async (event) => enqueueEvent(() => {
    if (!active || !agentStarted) return;
    const toolName = typeof event?.toolName === "string" ? event.toolName : "unknown";
    const toolCallId = typeof event?.toolCallId === "string" ? event.toolCallId : "unknown";
    const entry = append("tool_started", { ...identity(active), toolName, toolCallId, evidenceRef: { journal: journalPath, sequence: sequence + 1 } });
    active.toolCalls ??= new Map();
    active.toolCalls.set(toolCallId, { toolName, startSequence: entry.sequence });
    const progress = { type: "tool_started", ...identity(active), toolName, toolCallId, evidenceRef: evidence(entry) };
    for (const socket of commandSockets.get(active.commandId) ?? []) if (!socket.destroyed) socket.write(`${JSON.stringify(progress)}\n`);
  }));

  pi.on("tool_execution_end", async (event) => enqueueEvent(() => {
    if (!active || !agentStarted) return;
    const toolName = typeof event?.toolName === "string" ? event.toolName : "unknown";
    const toolCallId = typeof event?.toolCallId === "string" ? event.toolCallId : "unknown";
    const prior = active.toolCalls?.get(toolCallId);
    if (!prior || prior.toolName !== toolName) return;
    const entry = append("tool_completed", { ...identity(active), toolName, toolCallId, startSequence: prior.startSequence, isError: event?.isError === true, evidenceRef: { journal: journalPath, sequence: sequence + 1 } });
    active.toolCalls.delete(toolCallId);
    // Store evidence identity on the corresponding durable row for audit consumers.
    rows.get(active.commandId).lastToolEvidenceRef = evidence(entry);
    const progress = { type: "tool_completed", ...identity(active), toolName, toolCallId, isError: event?.isError === true, evidenceRef: evidence(entry) };
    for (const socket of commandSockets.get(active.commandId) ?? []) if (!socket.destroyed) socket.write(`${JSON.stringify(progress)}\n`);
  }));

  pi.on("agent_end", async (event) => enqueueEvent(() => {
    if (event?.willContinue === true) return;
    const completedRun = agentStarted;
    agentStarted = false;
    if (!active || !completedRun) return;
    const messages = Array.isArray(event?.messages) ? event.messages : [];
    let assistant;
    for (let i = messages.length - 1; i >= 0; i--) {
      if (messages[i]?.role === "assistant") { assistant = messages[i]; break; }
    }
    if (!assistant) return;
    const content = Array.isArray(assistant.content) ? assistant.content : [];
    const reply = content.filter((item) => item?.type === "text" && typeof item.text === "string").map((item) => item.text).join("");
    const command = active;
    const entry = append("completed", { ...identity(command), reply, evidenceRef: { journal: journalPath, sequence: sequence + 1 } });
    const row = rows.get(command.commandId);
    row.state = "completed";
    row.result = reply;
    row.evidenceRef = evidence(entry);
    const completion = { type: "completed", ...identity(command), reply, evidenceRef: row.evidenceRef };
    for (const socket of commandSockets.get(command.commandId) ?? []) {
      if (!socket.destroyed) socket.write(`${JSON.stringify(completion)}\n`);
    }
    commandSockets.delete(command.commandId);
    active = null;
  }));

  pi.on("session_shutdown", async () => {
    shuttingDown = true;
    for (const client of clients) client.destroy();
    if (server) {
      await new Promise((resolve) => server.close(() => resolve()));
      server = undefined;
    }
    if (socketPath) {
      try { if (fs.lstatSync(socketPath).isSocket()) fs.unlinkSync(socketPath); } catch (error) { if (error?.code !== "ENOENT") console.error(`[m1-herdr-bridge] socket cleanup: ${error.message}`); }
    }
  });
}

