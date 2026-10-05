import assert from "node:assert/strict";
import { test } from "node:test";
import { AuthenticationError, credentialHash } from "../src/controller/auth.js";
import { canonicalJson } from "../src/controller/canonical.js";

test("canonical JSON rejects accessor-backed values without invoking them", () => {
  let reads = 0;
  const value = Object.defineProperty({}, "content", {
    enumerable: true,
    get: () => {
      reads += 1;
      return reads === 1 ? "first" : "later";
    },
  });

  assert.throws(() => canonicalJson(value), /accessor properties/);
  assert.equal(reads, 0);
});
test("canonical JSON rejects ill-formed UTF-16", () => {
  assert.throws(
    () => canonicalJson("lone-surrogate:\ud800"),
    /ill-formed UTF-16/,
  );
});
test("canonical JSON rejects ill-formed UTF-16 object keys", () => {
  const invalidKey = { ["key:\ud800"]: "value" };
  assert.throws(() => canonicalJson(invalidKey), /ill-formed UTF-16/);
});
test("canonical JSON rejects non-enumerable and extra array properties", () => {
  const hidden = Object.defineProperty({ title: "visible" }, "description", {
    value: "hidden",
    enumerable: false,
  });
  const extra = Object.assign(["criterion"], { hidden: "not serialized" });
  assert.throws(() => canonicalJson(hidden), /non-enumerable properties/);
  const invalidIndex = Object.assign([], { "4294967295": "not-an-index" });
  assert.throws(() => canonicalJson(extra), /extra properties/);
  assert.throws(() => canonicalJson(invalidIndex), /extra properties/);
});

test("credential hashing rejects ill-formed UTF-16", () => {
  const prefix = "c".repeat(31);
  assert.throws(() => credentialHash(`${prefix}\ud800`), AuthenticationError);
  assert.throws(() => credentialHash(`${prefix}\udc00`), AuthenticationError);
  assert.equal(credentialHash(`${prefix}\ud83d\ude00`).length, 64);
});
