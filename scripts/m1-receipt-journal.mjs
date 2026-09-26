const receiptTypes = new Set([
  "accepted", "submitted", "working", "tool_started", "tool_completed", "aborted",
  "dispatch_error", "completed", "agent_end_without_reply",
]);
const terminalReceiptTypes = new Set(["aborted", "dispatch_error", "completed"]);

export function isReceiptType(type) {
  return receiptTypes.has(type);
}

export function isTerminalReceipt(type) {
  return terminalReceiptTypes.has(type);
}

export function recoverAbortIntents(bytes, dispatched) {
  const completeEnd = bytes.lastIndexOf(0x0a) + 1;
  const authorized = new Set();
  const decoder = new TextDecoder("utf-8", { fatal: true, ignoreBOM: true });
  for (let start = 0; start < completeEnd;) {
    const end = bytes.indexOf(0x0a, start);
    let intent;
    try { intent = JSON.parse(decoder.decode(bytes.subarray(start, end))); }
    catch { throw new Error(`Malformed complete abort intent at byte ${start}`); }
    const dispatch = dispatched.get(intent?.commandId);
    if (!dispatch || intent.assignmentId !== dispatch.assignmentId || intent.attempt !== dispatch.attempt
      || intent.generation !== dispatch.generation || authorized.has(intent.commandId))
      throw new Error("Invalid durable abort intent during controller recovery");
    authorized.add(intent.commandId);
    start = end + 1;
  }
  return { authorized, completeEnd };
}

export function replayReceiptState(entries) {
  const commands = new Map();
  let active = null;
  for (const entry of entries) {
    if (entry.type === "accepted") {
      if (active || commands.has(entry.commandId) || typeof entry.prompt !== "string"
        || !Number.isSafeInteger(entry.attempt) || !Number.isSafeInteger(entry.generation)) {
        throw new Error("Invalid or overlapping accepted receipt during controller recovery");
      }
      active = { commandId: entry.commandId, assignmentId: entry.assignmentId, attempt: entry.attempt,
        generation: entry.generation, prompt: entry.prompt };
      commands.set(entry.commandId, { ...active, accepted: true });
    } else {
      const command = commands.get(entry.commandId);
      if (!active || entry.commandId !== active.commandId || entry.assignmentId !== active.assignmentId
        || entry.attempt !== active.attempt || entry.generation !== active.generation) {
        throw new Error("Orphan or mismatched receipt during controller recovery");
      }
      command.state = entry.type === "completed" ? "completed"
        : entry.type === "agent_end_without_reply" || entry.type === "aborted" || entry.type === "dispatch_error" ? "unknown" : "working";
      if (entry.type === "agent_end_without_reply") active.state = "unknown";
      if (isTerminalReceipt(entry.type)) active = null;
    }
  }
  return { commands, active, sequence: entries.length, lastReceipt: entries.at(-1) ?? null };
}
