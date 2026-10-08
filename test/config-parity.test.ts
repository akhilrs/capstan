import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import {
  chmodSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readdirSync,
  readFileSync,
  realpathSync,
  rmSync,
  symlinkSync,
  writeFileSync,
} from "node:fs";
import os from "node:os";
import path from "node:path";
import { test } from "node:test";
import { locateFrontEnd, NODE_CLI } from "./cli-parity-harness.js";
import { STARTER_CONFIG } from "../src/config/starter.js";
import { DESIGNER_PROMPT } from "../src/roles/designer-prompt.js";
import {
  DIFFERENTIAL_COUNT,
  DIFFERENTIAL_SEED,
  PARITY_DIRECTORY,
  configCases,
  configCheckCases,
  differentialTexts,
  exportFixtures,
  promptCases,
} from "./config-parity-export.js";

const REGENERATE =
  "run `npm run build && node dist/test/config-parity-export.js` and commit rust/crates/config/tests/parity";
const SOURCES = path.join(PARITY_DIRECTORY, "..", "..", "src");

test("the committed Rust parity fixtures equal a fresh export", () => {
  const fresh = exportFixtures();
  for (const [name, text] of fresh) {
    const file = path.join(PARITY_DIRECTORY, name);
    assert.ok(existsSync(file), `${name} is missing; ${REGENERATE}`);
    assert.ok(
      readFileSync(file, "utf8") === text,
      `${name} is stale; ${REGENERATE}`,
    );
  }
  const captured = ["config-corpus.json", "prompts-corpus.json"];
  const extra = readdirSync(PARITY_DIRECTORY).filter(
    (name) => !fresh.has(name) && !captured.includes(name),
  );
  assert.deepEqual(extra, [], `unexpected fixtures; ${REGENERATE}`);
});

test("the corpora are large enough to mean something", () => {
  assert.ok(configCases().length >= 600);
  assert.ok(promptCases().length >= 150);
});

test("the Rust copies of the starter configuration and the designer prompt are the TypeScript bytes", () => {
  assert.equal(
    readFileSync(path.join(SOURCES, "starter.toml"), "utf8"),
    STARTER_CONFIG,
  );
  assert.equal(
    readFileSync(path.join(SOURCES, "designer.md"), "utf8"),
    DESIGNER_PROMPT,
  );
});

test("the differential generator makes the same cases from the same seed", () => {
  const first = differentialTexts(500, DIFFERENTIAL_SEED);
  assert.deepEqual(differentialTexts(500, DIFFERENTIAL_SEED), first);
  assert.notDeepEqual(differentialTexts(500, DIFFERENTIAL_SEED + 1), first);
  const recorded = JSON.parse(
    readFileSync(path.join(PARITY_DIRECTORY, "differential.json"), "utf8"),
  ) as { seed: number; count: number; sha256: string };
  assert.equal(recorded.seed, DIFFERENTIAL_SEED);
  assert.equal(recorded.count, DIFFERENTIAL_COUNT);
  const texts = differentialTexts(DIFFERENTIAL_COUNT, DIFFERENTIAL_SEED);
  assert.equal(texts.length, DIFFERENTIAL_COUNT);
  assert.equal(
    createHash("sha256").update(JSON.stringify(texts)).digest("hex"),
    recorded.sha256,
  );
});

// The front end binary against the Node CLI over the `config check` scenarios of the CLI transcripts: the same stdout,
// stderr and exit code, and the cases the front end must not answer go to Node (a stand-in for it records the hand-over).
let frontEnd: string | null = null;
let missing: string | undefined;
try {
  frontEnd = locateFrontEnd();
} catch (error) {
  missing = error instanceof Error ? error.message : String(error);
}

const HANDED = "handed to node:";

function scratchProject(
  source: ReturnType<typeof configCheckCases>[number],
): string {
  const scratch = realpathSync(
    mkdtempSync(path.join(os.tmpdir(), "cstan-cfgcli-")),
  );
  for (const dir of source.dirs ?? [])
    mkdirSync(path.join(scratch, dir), { recursive: true });
  for (const [file, text] of Object.entries(source.contents ?? {})) {
    mkdirSync(path.dirname(path.join(scratch, file)), { recursive: true });
    writeFileSync(path.join(scratch, file), text, { mode: 0o600 });
    chmodSync(path.join(scratch, file), source.modes?.[file] ?? 0o600);
  }
  for (const link of source.links ?? [])
    symlinkSync(path.join(scratch, link.to), path.join(scratch, link.path));
  return scratch;
}

if (missing !== undefined) {
  test("the config check CLI parity needs the Rust front end", () => {
    assert.fail(missing);
  });
} else {
  test(
    "cstan config check: the front end and the Node CLI print the same, and only the deferred cases reach Node",
    { skip: frontEnd === null ? "CSTAN_SKIP_FRONT_PARITY=1" : false },
    () => {
      assert.ok(frontEnd !== null);
      const stubDirectory = mkdtempSync(
        path.join(os.tmpdir(), "cstan-cfgstub-"),
      );
      const stub = path.join(stubDirectory, "node-stub");
      writeFileSync(stub, `#!/bin/sh\necho "${HANDED} $*"\nexit 77\n`, {
        mode: 0o755,
      });
      const env = { TZ: "UTC", PATH: process.env.PATH ?? "" };
      let natively = 0;
      let handedOver = 0;
      try {
        for (const source of configCheckCases()) {
          const scratch = scratchProject(source);
          try {
            const cwd = path.join(scratch, source.cwd);
            const run = (
              command: string,
              args: readonly string[],
              extra: Record<string, string> = {},
            ) => {
              const result = spawnSync(command, [...args], {
                cwd,
                env: { ...env, ...extra },
                encoding: "utf8",
                timeout: 60_000,
              });
              return {
                stdout: result.stdout,
                stderr: result.stderr,
                status: result.status,
              };
            };
            const viaStub = run(frontEnd as string, source.argv, {
              CSTAN_NODE_CLI: stub,
            });
            if (source.fallback === true) {
              handedOver += 1;
              assert.deepEqual(
                viaStub,
                {
                  stdout: `${HANDED} ${source.argv.join(" ")}\n`,
                  stderr: "",
                  status: 77,
                },
                `${source.name} must go to Node`,
              );
            } else {
              natively += 1;
              assert.ok(
                !viaStub.stdout.startsWith(HANDED),
                `${source.name} must be answered by the front end`,
              );
            }
            if (source.skipNode === true) continue;
            const viaNode = run(process.execPath, [NODE_CLI, ...source.argv]);
            const viaFront = run(frontEnd as string, source.argv, {
              CSTAN_NODE_CLI: NODE_CLI,
              CSTAN_NODE: process.execPath,
            });
            assert.deepEqual(
              viaFront,
              viaNode,
              `${source.name}: stdout, stderr and exit code`,
            );
            if (source.fallback !== true)
              assert.deepEqual(viaStub, viaNode, source.name);
          } finally {
            rmSync(scratch, { recursive: true, force: true });
          }
        }
      } finally {
        rmSync(stubDirectory, { recursive: true, force: true });
      }
      assert.ok(
        natively >= 25 && handedOver >= 10,
        `${natively} native, ${handedOver} handed over`,
      );
    },
  );
}
