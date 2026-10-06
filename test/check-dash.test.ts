import assert from "node:assert/strict";
import {
  chmodSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import os from "node:os";
import path from "node:path";
import { test } from "node:test";
import { spawnSync } from "node:child_process";

const script = path.resolve(
  import.meta.dirname,
  "..",
  "..",
  "scripts",
  "check-dash.mjs",
);

function run(env: Record<string, string>) {
  return spawnSync(process.execPath, [script], {
    env,
    encoding: "utf8",
  });
}

function withHome(body: (home: string) => void): void {
  const home = mkdtempSync(path.join(os.tmpdir(), "cstan-check-dash-"));
  try {
    body(home);
  } finally {
    rmSync(home, { recursive: true, force: true });
  }
}

/** A cargo that records its arguments and exits with `code`. */
function fakeCargo(directory: string, log: string, code = 0): void {
  mkdirSync(directory, { recursive: true });
  const file = path.join(directory, "cargo");
  writeFileSync(file, `#!/bin/sh\necho "$@" >> '${log}'\nexit ${code}\n`);
  chmodSync(file, 0o755);
}

test("without cargo the check fails and says how to find it", () => {
  withHome((home) => {
    const result = run({ HOME: home, PATH: "" });
    assert.notEqual(result.status, 0);
    assert.match(
      result.stderr,
      /check:dash: cargo not found; export PATH="\$HOME\/\.cargo\/bin:\$PATH"/,
    );
  });
});

test("CSTAN_SKIP_DASH_CHECK=1 turns the missing cargo into a loud skip", () => {
  withHome((home) => {
    const result = run({ HOME: home, PATH: "", CSTAN_SKIP_DASH_CHECK: "1" });
    assert.equal(result.status, 0);
    assert.equal(
      result.stderr,
      "check:dash SKIPPED: cargo not found (CSTAN_SKIP_DASH_CHECK=1); the Rust dashboard was NOT checked\n",
    );
  });
});

test("cargo under $HOME/.cargo/bin is found when PATH lacks it and runs fmt, clippy and test", () => {
  withHome((home) => {
    const log = path.join(home, "calls.log");
    fakeCargo(path.join(home, ".cargo", "bin"), log);
    const result = run({ HOME: home, PATH: "" });
    assert.equal(result.status, 0, result.stderr);
    assert.deepEqual(readFileSync(log, "utf8").trim().split("\n"), [
      "fmt --check",
      "clippy --all-targets --locked -- -D warnings",
      "test --locked",
    ]);
  });
});

test("a failing cargo step stops the check with its status", () => {
  withHome((home) => {
    const log = path.join(home, "calls.log");
    fakeCargo(path.join(home, ".cargo", "bin"), log, 3);
    const result = run({ HOME: home, PATH: "", CSTAN_SKIP_DASH_CHECK: "1" });
    assert.equal(result.status, 3);
    assert.equal(readFileSync(log, "utf8").trim().split("\n").length, 1);
  });
});
