import assert from "node:assert/strict";
import { isReceiptType, isTerminalReceipt, replayReceiptState } from "./m1-receipt-journal.mjs";

const identity = { commandId: "ended-without-reply", assignmentId: "assignment-1", attempt: 1, generation: 1 };
const accepted = { type: "accepted", ...identity, prompt: "do work" };
const submitted = { type: "submitted", ...identity };
const working = { type: "working", ...identity };
const ended = { type: "agent_end_without_reply", ...identity };

for (const entry of [accepted, submitted, working, ended]) assert.equal(isReceiptType(entry.type), true);
assert.equal(isTerminalReceipt(ended.type), false);
assert.equal(isTerminalReceipt(working.type), false);
const recovered = replayReceiptState([accepted, submitted, working, ended]);
assert.equal(recovered.active.commandId, identity.commandId, "assistant-less completion must retain the unresolved dispatch");
assert.equal(recovered.active.state, "unknown");
assert.equal(recovered.commands.get(identity.commandId).state, "unknown");
assert.equal(recovered.lastReceipt.type, "agent_end_without_reply");

const stillWorking = replayReceiptState([accepted, submitted, working]);
assert.equal(stillWorking.active.commandId, identity.commandId, "nonterminal records must keep an uncertain dispatch active");

const completed = replayReceiptState([accepted, submitted, working, { type: "completed", ...identity, reply: "done", evidenceRef: {} }]);
assert.equal(completed.active, null, "only durable terminal outcomes release the dispatch");
assert.equal(completed.commands.get(identity.commandId).state, "completed");
assert.throws(() => replayReceiptState([accepted, { ...ended, assignmentId: "other" }]), /Orphan or mismatched receipt/);
console.log("PASS receipt journal keeps ambiguous completions locked through recovery");
