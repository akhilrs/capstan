import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { test } from "node:test";
import { InvalidArgumentError } from "../src/herdr/adapter.js";
import {
  codexArguments,
  ompArguments,
  tomlString,
} from "../src/herdr/hosts.js";

test("a TOML string escapes backslash, quote and every control character, so it holds no newline", () => {
  assert.equal(tomlString('a"b\\c'), '"a\\"b\\\\c"');
  assert.equal(
    tomlString("l1\nl2\r\tx\u0000\u001f\u007f\u0085"),
    '"l1\\u000al2\\u000d\\u0009x\\u0000\\u001f\\u007f\\u0085"',
  );
  assert.equal(tomlString("é😀"), '"é😀"');
  assert.throws(() => tomlString("\ud800"), InvalidArgumentError);
  assert.doesNotMatch(tomlString("a\nb "), /[\n\r]/);
});

test("Codex runs with full access and no approval, the update check off, the worktree trusted and the prompt as instructions", () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "hosts-test-"));
  try {
    const real = fs.realpathSync(root);
    const link = `${root}-link`;
    fs.symlinkSync(root, link);
    const args = codexArguments(
      { model: "gpt-x" },
      'line one\nline "two"',
      link,
    );
    assert.deepEqual(args, [
      "--sandbox",
      "danger-full-access",
      "--ask-for-approval",
      "never",
      "--model",
      "gpt-x",
      "-c",
      "check_for_update_on_startup=false",
      "-c",
      `projects."${real}".trust_level="trusted"`,
      "-c",
      'developer_instructions="line one\\u000aline \\"two\\""',
    ]);
    fs.unlinkSync(link);
    assert.deepEqual(codexArguments({ model: null }, "p", undefined), [
      "--sandbox",
      "danger-full-access",
      "--ask-for-approval",
      "never",
      "-c",
      "check_for_update_on_startup=false",
      "-c",
      'developer_instructions="p"',
    ]);
  } finally {
    fs.rmSync(root, { recursive: true, force: true });
  }
});

test("Codex refuses a model that starts with a dash and a prompt too large for one argument", () => {
  assert.throws(
    () => codexArguments({ model: "-m" }, "p", undefined),
    InvalidArgumentError,
  );
  assert.throws(
    () => codexArguments({ model: null }, "x".repeat(121 * 1024), undefined),
    /too large/,
  );
});

test("OMP approves every tool call and reads the prompt from the file", () => {
  assert.deepEqual(ompArguments({ model: null }, "/tmp/p.md"), [
    "--approval-mode",
    "yolo",
    "--append-system-prompt",
    "/tmp/p.md",
  ]);
  assert.deepEqual(ompArguments({ model: "m" }, "/tmp/p.md"), [
    "--approval-mode",
    "yolo",
    "--model",
    "m",
    "--append-system-prompt",
    "/tmp/p.md",
  ]);
  assert.throws(
    () => ompArguments({ model: "--x" }, "/tmp/p.md"),
    InvalidArgumentError,
  );
});
