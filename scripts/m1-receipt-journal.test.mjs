import assert from "node:assert/strict";
import { isReceiptType, isTerminalReceipt, recoverAbortIntents, replayReceiptState } from "./m1-receipt-journal.mjs";

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

const aborted = replayReceiptState([accepted, submitted, working, ended, { type: "aborted", ...identity }]);
assert.equal(aborted.active.commandId, identity.commandId, "abort intent alone cannot prove process containment");
assert.equal(aborted.active.state, "unknown");
assert.equal(aborted.commands.get(identity.commandId).state, "unknown");

const completed = replayReceiptState([accepted, submitted, working, { type: "completed", ...identity, reply: "done", evidenceRef: {} }]);
assert.equal(completed.active, null, "only durable terminal outcomes release the dispatch");
assert.equal(completed.commands.get(identity.commandId).state, "completed");
assert.throws(() => replayReceiptState([accepted, { ...ended, assignmentId: "other" }]), /Orphan or mismatched receipt/);

const intent = { commandId: identity.commandId, assignmentId: identity.assignmentId, attempt: identity.attempt, generation: identity.generation };
const intentLine = Buffer.from(`${JSON.stringify(intent)}\n`);
const dispatches = new Map([[identity.commandId, intent]]);
assert.deepEqual(recoverAbortIntents(Buffer.alloc(0), dispatches), { authorized: new Set(), completeEnd: 0 });
assert.deepEqual(recoverAbortIntents(Buffer.from('{"commandId":'), dispatches), { authorized: new Set(), completeEnd: 0 });
const tornTail = Buffer.concat([intentLine, Buffer.from('{"commandId":')]);
assert.deepEqual(recoverAbortIntents(tornTail, dispatches), { authorized: new Set([identity.commandId]), completeEnd: intentLine.length });
assert.throws(() => recoverAbortIntents(Buffer.from('{"commandId":\n'), dispatches), /Malformed complete abort intent/);
console.log("PASS receipt journal keeps ambiguous completions locked through recovery");
