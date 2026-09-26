import assert from "node:assert/strict";
import { isReceiptType, isTerminalReceipt, replayReceiptState } from "./m1-receipt-journal.mjs";

const identity = { commandId: "ended-without-reply", assignmentId: "assignment-1", attempt: 1, generation: 1 };
const accepted = { type: "accepted", ...identity, prompt: "do work" };
const submitted = { type: "submitted", ...identity };
const working = { type: "working", ...identity };
const ended = { type: "agent_end_without_assistant", ...identity };

for (const entry of [accepted, submitted, working, ended]) assert.equal(isReceiptType(entry.type), true);
assert.equal(isTerminalReceipt(ended.type), true);
assert.equal(isTerminalReceipt(working.type), false);
const recovered = replayReceiptState([accepted, submitted, working, ended]);
assert.equal(recovered.active, null, "receipt replay must release the ended dispatch");
assert.equal(recovered.commands.size, 1);
assert.equal(recovered.lastReceipt.type, "agent_end_without_assistant");

const stillWorking = replayReceiptState([accepted, submitted, working]);
assert.equal(stillWorking.active.commandId, identity.commandId, "nonterminal records must keep an uncertain dispatch active");

assert.throws(() => replayReceiptState([accepted, { ...ended, assignmentId: "other" }]), /Orphan or mismatched receipt/);
console.log("PASS receipt journal accepts and replays assistant-less terminal state without clearing nonterminal work");
