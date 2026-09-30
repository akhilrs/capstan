import assert from "node:assert/strict";
import { chmodSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { test } from "node:test";
import {
  HerdrError,
  createHerdrRunner,
  herdrEnvironment,
  runJson,
} from "../src/herdr/runner.js";

function stub(script: string): { binary: string; cleanup: () => void } {
  const directory = mkdtempSync(path.join(tmpdir(), "capstan-runner-"));
  const binary = path.join(directory, "herdr");
  writeFileSync(binary, `#!/bin/sh\n${script}\n`);
  chmodSync(binary, 0o755);
  return { binary, cleanup: () => rmSync(directory, { recursive: true }) };
}

test("the runner always names the session and drops inherited HERDR_ variables", async () => {
  const fake = stub(
    'printf "%s|%s|%s" "$*" "${HERDR_ENV-unset}" "${KEEP-unset}"',
  );
  try {
    const run = createHerdrRunner({
      session: "capstan-t1",
      binary: fake.binary,
      env: { PATH: process.env.PATH, HERDR_ENV: "leak", KEEP: "yes" },
    });
    const result = await run(["pane", "get", "w1:p1"]);
    assert.equal(
      result.stdout,
      "--session capstan-t1 pane get w1:p1|unset|yes",
    );
    for (const session of [
      "",
      "-x",
      "a b",
      "a/b",
      "x".repeat(65),
      undefined as never,
    ])
      assert.throws(
        () => createHerdrRunner({ session, binary: fake.binary }),
        TypeError,
      );
  } finally {
    fake.cleanup();
  }
});

test("herdrEnvironment removes only HERDR_ names", () => {
  assert.deepEqual(herdrEnvironment({ A: "1", HERDR_X: "2", HERDRX: "3" }), {
    A: "1",
    HERDRX: "3",
  });
});

test("the runner kills a command that exceeds its timeout or output cap", async () => {
  const slow = stub("sleep 5");
  const loud = stub("head -c 5000000 /dev/zero");
  try {
    await assert.rejects(
      createHerdrRunner({
        session: "capstan-t2",
        binary: slow.binary,
        timeoutMs: 200,
      })(["x"]),
      (error: unknown) =>
        error instanceof HerdrError && error.code === "timeout",
    );
    await assert.rejects(
      createHerdrRunner({ session: "capstan-t3", binary: loud.binary })(["x"]),
      (error: unknown) => error instanceof HerdrError,
    );
  } finally {
    slow.cleanup();
    loud.cleanup();
  }
});

test("runJson returns the result and turns an error object on stdout or stderr into a HerdrError", async () => {
  const ok = async () => ({
    code: 0,
    stdout: '{"id":"x","result":{"a":1}}',
    stderr: "",
  });
  assert.deepEqual(await runJson(ok, ["x"]), { a: 1 });
  const onStderr = async () => ({
    code: 1,
    stdout: "",
    stderr: '{"error":{"code":"agent_not_ready","message":"blocked"}}',
  });
  await assert.rejects(
    runJson(onStderr, ["agent"]),
    (error: unknown) =>
      error instanceof HerdrError && error.code === "agent_not_ready",
  );
  const onStdout = async () => ({
    code: 1,
    stdout: '{"error":{"code":"pane_not_found","message":"gone"}}',
    stderr: "",
  });
  await assert.rejects(
    runJson(onStdout, ["pane"]),
    (error: unknown) =>
      error instanceof HerdrError && error.code === "pane_not_found",
  );
  for (const stdout of [
    "not json",
    "[]",
    '{"id":"x"}',
    '{"id":"x","result":[]}',
  ])
    await assert.rejects(
      runJson(async () => ({ code: 0, stdout, stderr: "" }), ["x"]),
      (error: unknown) =>
        error instanceof HerdrError && error.code === "bad_output",
    );
});

test("a per-call timeout overrides the runner limit", async () => {
  const slow = stub("sleep 1; printf done");
  try {
    const run = createHerdrRunner({
      session: "capstan-t4",
      binary: slow.binary,
      timeoutMs: 200,
    });
    assert.equal((await run(["x"], { timeoutMs: 5000 })).stdout, "done");
    await assert.rejects(run(["x"]), HerdrError);
  } finally {
    slow.cleanup();
  }
});

test("a non-zero exit is a failure even when stdout parses as a result, and stderr wins over stdout noise", async () => {
  await assert.rejects(
    runJson(
      async () => ({
        code: 1,
        stdout: '{"id":"x","result":{"a":1}}',
        stderr: '{"error":{"code":"agent_not_ready","message":"blocked"}}',
      }),
      ["agent"],
    ),
    (error: unknown) =>
      error instanceof HerdrError && error.code === "agent_not_ready",
  );
  await assert.rejects(
    runJson(
      async () => ({ code: 2, stdout: "noise", stderr: "plain\u001b[1m text" }),
      ["agent"],
    ),
    (error: unknown) =>
      error instanceof HerdrError &&
      error.code === "exit" &&
      !/\p{Cc}/u.test(error.message),
  );
});

test("a multibyte character split across output chunks is decoded whole", async () => {
  const fake = stub("printf '\\342\\235'; sleep 0.3; printf '\\257 ok'");
  try {
    const run = createHerdrRunner({
      session: "capstan-t5",
      binary: fake.binary,
    });
    assert.equal((await run(["x"])).stdout, "❯ ok");
  } finally {
    fake.cleanup();
  }
});

test("a byte order mark does not hide a result and Herdr error text is cleaned", async () => {
  assert.deepEqual(
    await runJson(
      async () => ({
        code: 0,
        stdout: '\uFEFF{"id":"x","result":{"a":1}}',
        stderr: "",
      }),
      ["x"],
    ),
    { a: 1 },
  );
  await assert.rejects(
    runJson(
      async () => ({
        code: 1,
        stdout: "",
        stderr: JSON.stringify({
          error: { code: "bad\u001b[31m", message: "hi\u001b[31m \u202ethere" },
        }),
      }),
      ["x"],
    ),
    (error: unknown) =>
      error instanceof HerdrError &&
      error.code === "error" &&
      !/[\p{Cc}\p{Cf}]/u.test(error.message) &&
      /hi/.test(error.message),
  );
});
