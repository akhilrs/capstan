import assert from "node:assert/strict";
import { createReceiptFrameBuffer } from "./m1-receipt-frame.mjs";

const expected = { prompt: "café 🧪", commandId: "split-utf8" };
const bytes = Buffer.from(`${JSON.stringify(expected)}\n`);
const split = bytes.indexOf(Buffer.from("🧪")) + 2;
const frame = createReceiptFrameBuffer(1024);
assert.equal(frame.push(bytes.subarray(0, split)), null);
assert.deepEqual(JSON.parse(frame.push(bytes.subarray(split))), expected);
assert.throws(() => createReceiptFrameBuffer(32).push(Buffer.from(`${"x".repeat(33)}\n`)), /byte limit/);
assert.throws(() => {
  const malformed = createReceiptFrameBuffer(32);
  malformed.push(Buffer.from([0xc3, 0x28, 0x0a]));
}, /encoded data|encoding/i);
assert.throws(() => createReceiptFrameBuffer(32).push(Buffer.from("{}\n{}\n")), /multiple receipt frames/);
const splitFrames = createReceiptFrameBuffer(32);
assert.equal(splitFrames.push(Buffer.from("{}\n")), "{}");
assert.throws(() => splitFrames.push(Buffer.from("{}\n")), /multiple receipt frames/);
console.log("PASS receipt framing split UTF-8, malformed UTF-8, byte limit, and single-frame rules");
