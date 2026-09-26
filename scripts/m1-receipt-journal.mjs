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
