import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { existsSync, readdirSync, readFileSync } from "node:fs";
import path from "node:path";
import { test } from "node:test";
import { removeTempDir, tempDir } from "./tmp.js";

const root = path.resolve(import.meta.dirname, "..", "..");
const EXPORTER = path.join(root, "dist", "test", "daemon-transcript-export.js");
const TRANSCRIPTS = path.join(
  root,
  "rust",
  "crates",
  "daemon",
  "tests",
  "transcripts",
);
const SCENARIOS = path.join(root, "test", "daemon-scenarios");
const REGENERATE =
  "run `npm run build && node dist/test/daemon-transcript-export.js` and commit the result";

/** Runs the exporter in a child process as a developer would, with a TMPDIR of its own so a leaked scratch is seen. */
function exportTo(label: string): {
  files: Map<string, string>;
  leftInTmp: string[];
} {
  const out = tempDir(`cdt-out-${label}-`);
  const scratch = tempDir(`cdt-tmp-${label}-`);
  const run = spawnSync(process.execPath, [EXPORTER, "--out", out], {
    env: { ...process.env, TMPDIR: scratch },
    encoding: "utf8",
    timeout: 300_000,
  });
  assert.equal(run.status, 0, `exporter failed: ${run.stderr}${run.stdout}`);
  const directory = path.join(
    out,
    "rust",
    "crates",
    "daemon",
    "tests",
    "transcripts",
  );
  const files = new Map<string, string>();
  for (const name of readdirSync(directory))
    files.set(name, readFileSync(path.join(directory, name), "utf8"));
  const leftInTmp = readdirSync(scratch);
  removeTempDir(out);
  removeTempDir(scratch);
  return { files, leftInTmp };
}

test("the committed daemon transcripts equal a fresh export of the Node daemon", () => {
  const fresh = exportTo("fresh");
  assert.deepEqual(
    fresh.leftInTmp,
    [],
    "the export left scratch entries behind",
  );
  for (const [name, text] of fresh.files) {
    const file = path.join(TRANSCRIPTS, name);
    assert.ok(existsSync(file), `${name} is missing; ${REGENERATE}`);
    assert.ok(
      readFileSync(file, "utf8") === text,
      `${name} is stale; ${REGENERATE}`,
    );
  }
  const scenarios = readdirSync(SCENARIOS).filter((n) => n.endsWith(".json"));
  assert.deepEqual(
    readdirSync(TRANSCRIPTS).sort(),
    scenarios.sort(),
    `a transcript has no scenario file or the other way round; ${REGENERATE}`,
  );
});

test("core.json covers the refusals of the wire protocol and a stub route", () => {
  const document = JSON.parse(
    readFileSync(path.join(TRANSCRIPTS, "core.json"), "utf8"),
  ) as { dict: unknown[]; scenarios: Array<{ steps: unknown[] }> };
  const expand = (value: unknown): unknown => {
    if (Array.isArray(value)) return value.map(expand);
    if (value !== null && typeof value === "object") {
      const entries = Object.entries(value);
      const [only] = entries;
      if (entries.length === 1 && only![0] === "$d")
        return expand(document.dict[only![1] as number]);
      return Object.fromEntries(entries.map(([k, v]) => [k, expand(v)]));
    }
    return value;
  };
  const responses = document.scenarios
    .flatMap((scenario) => scenario.steps)
    .map((step) => (expand(step) as { response: string | null }).response ?? "")
    .join("\n");
  for (const wanted of [
    '"pong":true',
    '"projectId"',
    '"code":"unauthorized"',
    '"code":"forbidden"',
    '"code":"unknown_command"',
    '"code":"invalid_request","message":"request is not valid JSON"',
    '"code":"invalid_request","message":"malformed command request"',
    "args must be at most 16 non-empty strings",
    '"code":"not_implemented"',
  ])
    assert.ok(responses.includes(wanted), `core.json has no ${wanted}`);
  const frames = document.scenarios.flatMap((s) =>
    s.steps.map((step) => expand(step) as { layer: string }),
  );
  assert.ok(frames.some((step) => step.layer === "socket"));
  assert.ok(frames.some((step) => step.layer === "dispatch"));
});
