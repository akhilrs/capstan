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
  const fd = fs.openSync(file, fs.constants.O_RDONLY | fs.constants.O_NOFOLLOW);
  let bytes;
  try {
    if (!fs.fstatSync(fd).isFile()) throw new Error("Bridge journal is not a regular file");
    bytes = fs.readFileSync(fd);
  } finally { fs.closeSync(fd); }
  const rows = new Map();
  let offset = 0;
  let sequence = 0;
  let currentActive = null;
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
      if (currentActive || rows.has(entry.commandId) || typeof entry.assignmentId !== "string"
        || !Number.isSafeInteger(entry.attempt) || !Number.isSafeInteger(entry.generation) || typeof entry.prompt !== "string") {
        throw new Error(`invalid accepted journal record at byte ${offset}`);
      }
      rows.set(entry.commandId, { accepted: entry, state: "acknowledged" });
      currentActive = { commandId: entry.commandId, assignmentId: entry.assignmentId, attempt: entry.attempt,
        generation: entry.generation, prompt: entry.prompt };
    } else {
      const row = rows.get(entry.commandId);
      if (!row || !currentActive || currentActive.commandId !== entry.commandId
        || currentActive.assignmentId !== entry.assignmentId || currentActive.attempt !== entry.attempt
        || currentActive.generation !== entry.generation) {
        throw new Error(`orphan or mismatched journal record at byte ${offset}`);
      }
      if (entry.type === "completed") {
        if (typeof entry.reply !== "string" || !entry.evidenceRef) throw new Error(`invalid completion journal record at byte ${offset}`);
        row.state = "completed";
        row.result = entry.reply;
        row.evidenceRef = entry.evidenceRef;
        currentActive = null;
      } else if (entry.type === "submitted" || entry.type === "working" || entry.type === "tool_started" || entry.type === "tool_completed"
        || entry.type === "aborted" || entry.type === "dispatch_error" || entry.type === "agent_end_without_reply") {
        row.state = entry.type === "aborted" || entry.type === "dispatch_error" || entry.type === "agent_end_without_reply"
          ? "unknown" : "working";
        if (entry.type === "agent_end_without_reply" || entry.type === "aborted" || entry.type === "dispatch_error") currentActive.state = "unknown";
        if (entry.type === "aborted") row.aborted = true;
      } else {
        throw new Error(`unknown journal record type at byte ${offset}`);
      }
    }
    offset = newline + 1;
  }
  // The controller owns and repairs its journal. A partial tail is ignored on
  // replay; a worker must never truncate or otherwise mutate this read-only file.
  return { rows, sequence, active: currentActive };
}

export default function herdrBridge(pi) {
  // The factory is registration-only: no socket, filesystem, or runtime actions here.
  let server;
  let ctxRef;
  let journalPath;
  let socketPath;
  let receiptSocketPath;
  let role;
  let sequence = 0;
  let rows = new Map();
  let active = null;
  let agentStarted = false;
  let lastTurnMessage = null;
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

  async function append(type, fields) {
    const entry = { sequence: sequence + 1, timestamp: new Date().toISOString(), type, role, ...fields };
    const line = `${JSON.stringify(entry)}\n`;
    const deadline = Date.now() + 12_000;
    while (true) {
      const socket = net.createConnection(receiptSocketPath);
      let response;
      try {
        response = await new Promise((resolve, reject) => {
          let pending = "";
          const timer = setTimeout(() => { socket.destroy(); reject(new Error("Controller receipt acknowledgement timed out")); }, 2_000);
          socket.once("connect", () => socket.end(line));
          socket.on("data", (chunk) => {
            pending += chunk.toString("utf8");
            const end = pending.indexOf("\n");
            if (end < 0) return;
            clearTimeout(timer);
            try { resolve(JSON.parse(pending.slice(0, end))); } catch (error) { reject(error); }
            socket.end();
          });
          socket.once("error", (error) => { clearTimeout(timer); reject(error); });
          socket.once("close", () => { clearTimeout(timer); reject(new Error("Controller receipt socket closed without durable acknowledgement")); });
        });
      } catch (error) {
        socket.destroy();
        if (Date.now() >= deadline || shuttingDown) throw new Error(`Controller receipt unavailable; assignment state unknown: ${error.message}`);
        await new Promise((resolve) => setTimeout(resolve, 100));
        continue;
      }
      if (response?.ok !== true || response.sequence !== entry.sequence)
        throw new Error(`Controller did not durably acknowledge bridge receipt: ${String(response?.error ?? "sequence mismatch").slice(0, 256)}`);
      sequence = entry.sequence;
      return entry;
    }
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
      if (row.state === "completed" || (row.state === "unknown" && active?.commandId !== request.commandId)) return snapshot(request.commandId);
      if (active?.commandId === request.commandId && !row.aborted) {
        await append("aborted", { ...identity(active) });
        row.state = "unknown";
        row.aborted = true;
        active.dispatchFailed = true;
        agentStarted = false;
        try { await ctxRef.abort(); } catch (error) { console.error(`[m1-herdr-bridge] abort failed; assignment remains locked: ${error?.message ?? error}`); }
      }
      return snapshot(request.commandId);
    }
    if (row) {
      if (!sameCommand(row, request)) fail("commandId payload mismatch");
      return snapshot(request.commandId);
    }
    if (active) fail("another assignment is active");
    if (!ctxRef?.isIdle?.()) fail("OMP session is not idle for a new assignment");
    const accepted = await append("accepted", { commandId: request.commandId, assignmentId: request.assignmentId, attempt: request.attempt, generation: request.generation, prompt: request.prompt });
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
      if (shuttingDown || active?.commandId !== payload.commandId || active.dispatchFailed) return;
      const command = active;
      try {
        pi.sendUserMessage(command.prompt);
      } catch (error) {
        command.dispatchFailed = true;
        agentStarted = false;
        rows.get(command.commandId).state = "unknown";
        try {
          await append("dispatch_error", { ...identity(command), error: String(error?.message ?? error).slice(0, 2048) });
        } catch (receiptError) {
          console.error(`[m1-herdr-bridge] failed dispatch remains unknown; dispatch_error receipt unavailable: ${receiptError?.message ?? receiptError}`);
        }
        const unknown = { type: "unknown", ...identity(command) };
        for (const subscriber of commandSockets.get(command.commandId) ?? []) if (!subscriber.destroyed) subscriber.write(`${JSON.stringify(unknown)}\n`);
        commandSockets.delete(command.commandId);
        console.error(`[m1-herdr-bridge] dispatch failed: ${error?.message ?? error}`);
        return;
      }
      try {
        await append("submitted", { ...identity(command) });
      } catch (error) {
        command.dispatchFailed = true;
        agentStarted = false;
        rows.get(command.commandId).state = "unknown";
        console.error(`[m1-herdr-bridge] submitted receipt unavailable; dispatch may be running, assignment remains unknown: ${error?.message ?? error}`);
        const unknown = { type: "unknown", ...identity(command) };
        for (const subscriber of commandSockets.get(command.commandId) ?? []) if (!subscriber.destroyed) subscriber.write(`${JSON.stringify(unknown)}\n`);
        commandSockets.delete(command.commandId);
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
    receiptSocketPath = process.env.CAPSTAN_BRIDGE_RECEIPT_SOCKET;
    if (!ROLES.has(role)) throw new Error("CAPSTAN_BRIDGE_ROLE must be PM, Developer, Verifier, or Supervisor");
    if (typeof socketPath !== "string" || !path.isAbsolute(socketPath) || typeof journalPath !== "string" || !path.isAbsolute(journalPath)
      || typeof receiptSocketPath !== "string" || !path.isAbsolute(receiptSocketPath)) throw new Error("Bridge socket, receipt socket, and journal paths must be absolute");
    const recovered = recoverJournal(journalPath, role);
    rows = recovered.rows;
    sequence = recovered.sequence;
    active = recovered.active;
    // An accepted command without durable completion remains locked until external recovery contains its runtime.
    if (active) active.dispatchFailed = true;
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

  pi.on("agent_start", async () => enqueueEvent(async () => {
    if (active?.dispatchFailed) return;
    lastTurnMessage = null;
    agentStarted = true;
    if (!active) return;
    const command = active;
    try {
      await append("working", { ...identity(command) });
    } catch (error) {
      command.dispatchFailed = true;
      agentStarted = false;
      rows.get(command.commandId).state = "unknown";
      const unknown = { type: "unknown", ...identity(command) };
      for (const socket of commandSockets.get(command.commandId) ?? []) if (!socket.destroyed) socket.write(`${JSON.stringify(unknown)}\n`);
      commandSockets.delete(command.commandId);
      console.error(`[m1-herdr-bridge] working receipt unavailable; assignment remains unknown: ${error?.message ?? error}`);
      return;
    }
    rows.get(command.commandId).state = "working";
    const workingAck = { type: "ack", commandId: command.commandId, durable: true, state: "working" };
    for (const socket of commandSockets.get(command.commandId) ?? []) if (!socket.destroyed) socket.write(`${JSON.stringify(workingAck)}\n`);
  }));

  pi.on("tool_execution_start", async (event) => enqueueEvent(async () => {
    if (!active || !agentStarted) return;
    const toolName = typeof event?.toolName === "string" ? event.toolName : "unknown";
    const toolCallId = typeof event?.toolCallId === "string" ? event.toolCallId : "unknown";
    const entry = await append("tool_started", { ...identity(active), toolName, toolCallId, evidenceRef: { journal: journalPath, sequence: sequence + 1 } });
    active.toolCalls ??= new Map();
    active.toolCalls.set(toolCallId, { toolName, startSequence: entry.sequence });
    const progress = { type: "tool_started", ...identity(active), toolName, toolCallId, evidenceRef: evidence(entry) };
    for (const socket of commandSockets.get(active.commandId) ?? []) if (!socket.destroyed) socket.write(`${JSON.stringify(progress)}\n`);
  }));

  pi.on("tool_execution_end", async (event) => enqueueEvent(async () => {
    if (!active || !agentStarted) return;
    const toolName = typeof event?.toolName === "string" ? event.toolName : "unknown";
    const toolCallId = typeof event?.toolCallId === "string" ? event.toolCallId : "unknown";
    const prior = active.toolCalls?.get(toolCallId);
    if (!prior || prior.toolName !== toolName) return;
    const entry = await append("tool_completed", { ...identity(active), toolName, toolCallId, startSequence: prior.startSequence, isError: event?.isError === true, evidenceRef: { journal: journalPath, sequence: sequence + 1 } });
    active.toolCalls.delete(toolCallId);
    // Store evidence identity on the corresponding durable row for audit consumers.
    rows.get(active.commandId).lastToolEvidenceRef = evidence(entry);
    const progress = { type: "tool_completed", ...identity(active), toolName, toolCallId, isError: event?.isError === true, evidenceRef: evidence(entry) };
    for (const socket of commandSockets.get(active.commandId) ?? []) if (!socket.destroyed) socket.write(`${JSON.stringify(progress)}\n`);
  }));
  pi.on("turn_end", async (event) => enqueueEvent(async () => {
    if (!active || !agentStarted) return;
    lastTurnMessage = event?.message?.role === "assistant" ? event.message : null;
  }));


  pi.on("agent_end", async (event) => enqueueEvent(async () => {
    if (event?.willContinue === true) {
      lastTurnMessage = null;
      return;
    }
    const completedRun = agentStarted;
    agentStarted = false;
    if (!active || !completedRun) return;
    const command = active;
    const assistant = lastTurnMessage;
    lastTurnMessage = null;
    const content = Array.isArray(assistant?.content) ? assistant.content : [];
    const reply = content.filter((item) => item?.type === "text" && typeof item.text === "string").map((item) => item.text).join("");
    if (!assistant || reply.trim().length === 0) {
      // Unknown is not proof of containment; keep the slot reserved until a terminal receipt is durable.
      command.dispatchFailed = true;
      const row = rows.get(command.commandId);
      row.state = "unknown";
      const entry = await append("agent_end_without_reply", { ...identity(command) });
      row.evidenceRef = evidence(entry);
      const unknown = { type: "unknown", ...identity(command), evidenceRef: row.evidenceRef };
      for (const socket of commandSockets.get(command.commandId) ?? []) {
        if (!socket.destroyed) socket.write(`${JSON.stringify(unknown)}\n`);
      }
      commandSockets.delete(command.commandId);
      return;
    }
    let entry;
    try {
      entry = await append("completed", { ...identity(command), reply, evidenceRef: { journal: journalPath, sequence: sequence + 1 } });
    } catch (error) {
      command.dispatchFailed = true;
      rows.get(command.commandId).state = "unknown";
      const unknown = { type: "unknown", ...identity(command) };
      for (const socket of commandSockets.get(command.commandId) ?? []) {
        if (!socket.destroyed) socket.write(`${JSON.stringify(unknown)}\n`);
      }
      commandSockets.delete(command.commandId);
      console.error(`[m1-herdr-bridge] completion receipt unavailable; result remains unknown: ${error?.message ?? error}`);
      return;
    }
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

