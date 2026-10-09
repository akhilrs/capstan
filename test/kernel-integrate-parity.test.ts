import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { existsSync, readdirSync, readFileSync } from "node:fs";
import path from "node:path";
import { test } from "node:test";
import { removeTempDir, tempDir } from "./tmp.js";

const root = path.resolve(import.meta.dirname, "..", "..");
const EXPORTER = path.join(root, "dist", "test", "kernel-integrate-export.js");
const DIRECTORY = "rust/crates/kernel/tests/parity-integrate";
const REGENERATE =
  "run `npm run build && node dist/test/kernel-integrate-export.js` and commit the result";

function filesUnder(directory: string): string[] {
  if (!existsSync(directory)) return [];
  const out: string[] = [];
  for (const entry of readdirSync(directory, { withFileTypes: true })) {
    const full = path.join(directory, entry.name);
    if (entry.isDirectory()) out.push(...filesUnder(full));
    else out.push(full);
  }
  return out.sort();
}

/**
 * Runs the exporter in a child process exactly as a developer would, with a TMPDIR of its own so that a scratch
 * repository or state directory it leaves behind is seen.
 */
function exportTo(label: string): {
  files: Map<string, string>;
  leftInTmp: string[];
} {
  const out = tempDir(`capstan-integrate-out-${label}-`);
  const scratch = tempDir(`capstan-integrate-tmp-${label}-`);
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
  return { files, leftInTmp };
}

test("two exports of the integrate fixtures are byte-identical, and the export leaves no scratch directory behind", () => {
  const first = exportTo("first");
  const second = exportTo("second");
  assert.deepEqual(first.leftInTmp, [], "the export left scratch entries");
  assert.deepEqual(second.leftInTmp, [], "the export left scratch entries");
  assert.deepEqual([...second.files.keys()], [...first.files.keys()]);
  for (const [name, content] of first.files)
    assert.ok(
      second.files.get(name) === content,
      `${name} differs between two exports: the run is not deterministic`,
    );
});

test("the committed integrate fixtures equal a fresh export", () => {
  const fresh = exportTo("fresh");
  assert.deepEqual(fresh.leftInTmp, [], "the export left scratch entries");
  assert.ok(fresh.files.size > 0, "the export wrote nothing");
  for (const [name, content] of fresh.files) {
    const committed = path.join(root, name);
    assert.ok(existsSync(committed), `${name} is missing; ${REGENERATE}`);
    assert.ok(
      readFileSync(committed, "utf8") === content,
      `${name} is stale; ${REGENERATE}`,
    );
  }
  // Nothing generated may be committed that the export no longer writes.
  const extra = filesUnder(path.join(root, DIRECTORY))
    .map((file) => path.relative(root, file).split(path.sep).join("/"))
    .filter((name) => !fresh.files.has(name));
  assert.deepEqual(
    extra,
    [],
    `unexpected files in ${DIRECTORY}; ${REGENERATE}`,
  );
});
