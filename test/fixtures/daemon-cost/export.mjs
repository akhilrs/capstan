// Writes the goldens for the generated large ledger with a given build:
//   node test/fixtures/daemon-cost/export.mjs <dist dir> <out.json>
// The committed goldens were made with the code before the daemon-profile fixes (git 6881413); test/daemon-cost.test.ts
// compares the current code with them.
// The ledger is built through <dist dir>'s own ControllerCore, so an old build gets the schema it knows.
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { capture, normalise, openFor, serve } from "./capture.mjs";
import { buildLedger } from "./ledger.mjs";

const [distDir, out] = process.argv.slice(2);
if (!distDir || !out)
  throw new Error("usage: export.mjs <dist dir> <out.json> [generator dist]");
const dir = fs.mkdtempSync(path.join(os.tmpdir(), "capstan-daemon-cost-"));
try {
  const built = await buildLedger(distDir, dir);
  const core = await openFor(distDir, built);
  const served = await serve(distDir, core, built);
  const result = normalise(await capture(distDir, core, served, built), {
    [dir]: "<dir>",
  });
  await served.close();
  core.close();
  fs.writeFileSync(out, `${JSON.stringify(result, null, 1)}\n`);
  console.log(`wrote ${out}`);
} finally {
  fs.rmSync(dir, { recursive: true, force: true });
}
process.exit(0);
