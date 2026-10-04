// Runs the compiled test suite with a private TMPDIR so every temp directory
// the suite (and the daemons it spawns) creates lives under one per-run root.
// After the run the root is inspected: anything still inside it is a leaked
// temp directory. The leak is reported (and fails the run unless
// CAPSTAN_TEST_TMP_LEAK=warn) and the root is then removed. Only this run's
// root is examined, so a suite running in parallel cannot disturb the count.
import { spawn } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";

const testDirectory = path.resolve("dist/test");
const files = fs
  .readdirSync(testDirectory)
  .filter((name) => name.endsWith(".test.js"))
  .sort()
  .map((name) => path.join(testDirectory, name));

const root = fs.realpathSync(
  fs.mkdtempSync(path.join(os.tmpdir(), "capstan-testrun-")),
);
const env = { ...process.env, TMPDIR: root, CAPSTAN_TEST_TMP_ROOT: root };
const warnOnly = process.env.CAPSTAN_TEST_TMP_LEAK === "warn";

const child = spawn(
  process.execPath,
  ["--test", ...process.argv.slice(2), ...files],
  { env, stdio: "inherit" },
);

for (const signal of ["SIGINT", "SIGTERM"]) {
  process.on(signal, () => child.kill(signal));
}

child.on("close", (code, signal) => {
  // node and the host tool create their own entries (compile cache, sockets).
  const leaked = fs
    .readdirSync(root)
    .filter((name) => /^(capstan|cstan|cph)-/.test(name));
  fs.rmSync(root, { recursive: true, force: true, maxRetries: 3 });
  let status = code ?? (signal ? 1 : 0);
  if (leaked.length > 0) {
    process.stderr.write(
      `\ntemp-dir guard: the suite left ${leaked.length} temp director${leaked.length === 1 ? "y" : "ies"} behind (removed by the wrapper):\n`,
    );
    const counts = new Map();
    for (const name of leaked) {
      const prefix = name.replace(/[-.][A-Za-z0-9]{6}$/, "");
      counts.set(prefix, (counts.get(prefix) ?? 0) + 1);
    }
    for (const [prefix, count] of counts) {
      process.stderr.write(`  ${count} x ${prefix}-*\n`);
    }
    if (!warnOnly && status === 0) status = 1;
  } else {
    process.stderr.write("temp-dir guard: no leaked temp directories\n");
  }
  process.exit(status);
});
