import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { existsSync, readdirSync, readFileSync, statSync } from "node:fs";
import path from "node:path";
import { test } from "node:test";
import { removeTempDir, tempDir } from "./tmp.js";

const root = path.resolve(import.meta.dirname, "..", "..");
const EXPORTER = path.join(root, "dist", "test", "kernel-parity-export.js");
const REGENERATE =
  "run `npm run build && node dist/test/kernel-parity-export.js` and commit the result";

function filesUnder(directory: string): string[] {
  const out: string[] = [];
  for (const entry of readdirSync(directory, { withFileTypes: true })) {
    const full = path.join(directory, entry.name);
    if (entry.isDirectory()) out.push(...filesUnder(full));
    else out.push(full);
  }
  return out.sort();
}

/**
 * Runs the exporter in a child process exactly as a developer would (no flags, no hooks of this process), with a
 * TMPDIR of its own so that a scratch directory it leaves behind is seen. Returns the exported tree and what the
 * child left in its TMPDIR.
 */
function exportTo(label: string): {
  files: Map<string, string>;
  leftInTmp: string[];
  out: string;
} {
  const out = tempDir(`capstan-kernel-out-${label}-`);
  const scratch = tempDir(`capstan-kernel-tmp-${label}-`);
  const run = spawnSync(process.execPath, [EXPORTER, "--out", out], {
    env: { ...process.env, TMPDIR: scratch },
    encoding: "utf8",
    timeout: 300_000,
  });
  assert.equal(run.status, 0, `exporter failed: ${run.stderr}${run.stdout}`);
  const files = new Map<string, string>();
  for (const file of filesUnder(out))
    files.set(
      path.relative(out, file).split(path.sep).join("/"),
      readFileSync(file, "utf8"),
    );
  const leftInTmp = readdirSync(scratch);
  removeTempDir(out);
  removeTempDir(scratch);
  return { files, leftInTmp, out };
}

test("two exports of the kernel parity fixtures are byte-identical, and the export leaves no scratch directory behind", () => {
  const first = exportTo("first");
  const second = exportTo("second");
  assert.deepEqual(
    first.leftInTmp,
    [],
    "the export left scratch entries in its TMPDIR",
  );
  assert.deepEqual(
    second.leftInTmp,
    [],
    "the export left scratch entries in its TMPDIR",
  );
  assert.deepEqual(
    [...second.files.keys()],
    [...first.files.keys()],
    "the exports wrote different files",
  );
  for (const [name, text] of first.files)
    assert.ok(
      second.files.get(name) === text,
      `${name} differs between two exports: the run is not deterministic`,
    );
});

test("the committed kernel parity fixtures equal a fresh export", () => {
  const fresh = exportTo("fresh");
  assert.deepEqual(
    fresh.leftInTmp,
    [],
    "the export left scratch entries in its TMPDIR",
  );
  assert.ok(fresh.files.size > 0, "the export wrote nothing");
  for (const [name, text] of fresh.files) {
    const committed = path.join(root, name);
    assert.ok(existsSync(committed), `${name} is missing; ${REGENERATE}`);
    assert.ok(
      readFileSync(committed, "utf8") === text,
      `${name} is stale; ${REGENERATE}`,
    );
  }
  // Nothing generated may be committed that the export no longer writes.
  for (const directory of [
    "rust/crates/kernel/tests/parity",
    "test/fixtures/plan-bodies",
    "test/fixtures/kernel-text",
  ]) {
    const committed = filesUnder(path.join(root, directory)).map((file) =>
      path.relative(root, file).split(path.sep).join("/"),
    );
    const extra = committed.filter((name) => !fresh.files.has(name));
    assert.deepEqual(
      extra,
      [],
      `unexpected files in ${directory}; ${REGENERATE}`,
    );
  }
});

test("every sequence file has a parity group, found by glob", () => {
  const sequences = readdirSync(path.join(root, "test", "kernel-sequences"))
    .filter((name) => name.endsWith(".json"))
    .map((name) => name.slice(0, -".json".length))
    .sort();
  const groups = readdirSync(
    path.join(root, "rust", "crates", "kernel", "tests", "parity"),
  )
    .filter((name) => name.endsWith(".json"))
    .map((name) => name.slice(0, -".json".length))
    .sort();
  assert.deepEqual(groups, sequences);
  assert.ok(sequences.includes("core"));
  for (const group of groups) {
    const file = path.join(
      root,
      "rust",
      "crates",
      "kernel",
      "tests",
      "parity",
      `${group}.json`,
    );
    assert.ok(statSync(file).size > 0);
  }
});
