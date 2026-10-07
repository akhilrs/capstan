import assert from "node:assert/strict";
import { describe, test } from "node:test";
import {
  loadScenarios,
  locateFrontEnd,
  runScenario,
} from "./cli-parity-harness.js";

// No test may reach a real Herdr session or a live daemon: the scratch daemons run with CAPSTAN_LAUNCH=off in their own
// projects, and no agent or operator identity of this process reaches a client (the harness builds each environment).
process.env.CAPSTAN_LAUNCH = "off";

let frontEnd: string | null = null;
let missing: string | undefined;
try {
  frontEnd = locateFrontEnd();
} catch (error) {
  missing = error instanceof Error ? error.message : String(error);
}

if (missing !== undefined) {
  // A missing binary fails the suite; only CSTAN_SKIP_FRONT_PARITY=1 skips it.
  test("CLI parity needs the Rust front end", () => {
    assert.fail(missing);
  });
} else {
  describe(
    "CLI parity: the Node CLI and the Rust front end",
    { concurrency: 4 },
    () => {
      const binary = frontEnd;
      for (const scenario of loadScenarios()) {
        test(
          scenario.name,
          { skip: binary === null ? "CSTAN_SKIP_FRONT_PARITY=1" : false },
          async () => {
            assert.ok(binary !== null);
            const [node, rust] = await Promise.all([
              runScenario("node", scenario, binary),
              runScenario("rust", scenario, binary),
            ]);
            assert.deepEqual(
              rust.steps,
              node.steps,
              "argv, stdout, stderr and exit code",
            );
            assert.deepEqual(
              rust.ledger,
              node.ledger,
              "messages and acks in the ledger",
            );
          },
        );
      }
    },
  );
}
