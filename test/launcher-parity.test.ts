import assert from "node:assert/strict";
import { existsSync, readdirSync, readFileSync } from "node:fs";
import path from "node:path";
import { test } from "node:test";
import { PARITY_DIRECTORY, exportFixtures } from "./launcher-parity-export.js";

const REGENERATE =
  "run `npm run build && node dist/test/launcher-parity-export.js` and commit rust/crates/launcher/tests/parity";

test("the committed launcher parity fixtures equal a fresh export", async () => {
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
