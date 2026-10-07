import assert from "node:assert/strict";
import { existsSync, readdirSync, readFileSync } from "node:fs";
import path from "node:path";
import { test } from "node:test";
import {
  TRANSCRIPT_DIRECTORY,
  exportTranscripts,
} from "./cli-transcript-export.js";

const REGENERATE =
  "run `npm run build && node dist/test/cli-transcript-export.js` and commit rust/crates/cstan/tests/transcripts";

test("the committed CLI transcripts equal a fresh export from the Node CLI", async () => {
  const fresh = await exportTranscripts();
  const stale: string[] = [];
  for (const [name, text] of fresh) {
    const file = path.join(TRANSCRIPT_DIRECTORY, name);
    if (!existsSync(file) || readFileSync(file, "utf8") !== text)
      stale.push(name);
  }
  assert.deepEqual(stale, [], `stale or missing transcripts; ${REGENERATE}`);
  const extra = existsSync(TRANSCRIPT_DIRECTORY)
    ? readdirSync(TRANSCRIPT_DIRECTORY).filter((name) => !fresh.has(name))
    : [];
  assert.deepEqual(extra, [], `unexpected transcripts; ${REGENERATE}`);
});
