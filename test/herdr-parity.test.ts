import assert from "node:assert/strict";
import { existsSync, readdirSync, readFileSync } from "node:fs";
import path from "node:path";
import { test } from "node:test";
import { PARITY_DIRECTORY, exportFixtures } from "./herdr-parity-export.js";

const REGENERATE =
  "run `npm run build && node dist/test/herdr-parity-export.js` and commit rust/crates/herdr/tests/parity";

test("the committed herdr parity fixtures equal a fresh export", async () => {
  const fresh = await exportFixtures();
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
