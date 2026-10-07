import assert from "node:assert/strict";
import { existsSync, readdirSync, readFileSync } from "node:fs";
import path from "node:path";
import { test } from "node:test";
import {
  PARITY_DIRECTORY,
  RENDER_SOURCE,
  exportFixtures,
} from "./wire-parity-export.js";

const REGENERATE =
  "run `npm run build && node dist/test/wire-parity-export.js` and commit rust/crates/wire/tests/parity";

test("the committed wire parity fixtures equal a fresh export", () => {
  const fresh = exportFixtures();
  for (const [name, text] of fresh) {
    const file = path.join(PARITY_DIRECTORY, name);
    assert.ok(existsSync(file), `${name} is missing; ${REGENERATE}`);
    assert.ok(
      readFileSync(file, "utf8") === text,
      `${name} is stale; ${REGENERATE}`,
    );
  }
  const extra = existsSync(PARITY_DIRECTORY)
    ? readdirSync(PARITY_DIRECTORY).filter((name) => !fresh.has(name))
    : [];
  assert.deepEqual(extra, [], `unexpected fixtures; ${REGENERATE}`);
});

test("the exporter's render() is the one in src/cli.ts", () => {
  const cli = readFileSync(
    path.resolve(import.meta.dirname, "..", "..", "src", "cli.ts"),
    "utf8",
  );
  assert.ok(
    cli.includes(RENDER_SOURCE),
    "render() in src/cli.ts changed; update RENDER_SOURCE and render() in test/wire-parity-export.ts",
  );
});
