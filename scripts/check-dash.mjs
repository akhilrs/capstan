// Runs the Rust dashboard's checks: rustfmt, clippy with warnings denied, and the tests, all with the committed lockfile.
import { spawnSync } from "node:child_process";
import { accessSync, constants } from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const dashDirectory = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  "..",
  "dash",
);

function executable(file) {
  try {
    accessSync(file, constants.X_OK);
    return true;
  } catch {
    return false;
  }
}

/** The cargo on PATH, else the one rustup installs under the home directory. */
function findCargo(env) {
  const candidates = (env.PATH ?? "")
    .split(path.delimiter)
    .filter((entry) => entry !== "")
    .map((entry) => path.join(entry, "cargo"));
  candidates.push(
    path.join(env.HOME ?? os.homedir(), ".cargo", "bin", "cargo"),
  );
  return candidates.find(executable);
}

const STEPS = [
  ["fmt", "--check"],
  ["clippy", "--all-targets", "--locked", "--", "-D", "warnings"],
  ["test", "--locked"],
];

const cargo = findCargo(process.env);
if (cargo === undefined) {
  if (process.env.CSTAN_SKIP_DASH_CHECK === "1") {
    process.stderr.write(
      "check:dash SKIPPED: cargo not found (CSTAN_SKIP_DASH_CHECK=1); the Rust dashboard was NOT checked\n",
    );
    process.exit(0);
  }
  process.stderr.write(
    'check:dash: cargo not found; export PATH="$HOME/.cargo/bin:$PATH"\n',
  );
  process.exit(1);
}

// cargo finds cargo-fmt and cargo-clippy next to itself or on PATH; make sure its own directory is on PATH.
const env = {
  ...process.env,
  PATH: [path.dirname(cargo), process.env.PATH ?? ""]
    .filter((entry) => entry !== "")
    .join(path.delimiter),
};
for (const args of STEPS) {
  process.stdout.write(`check:dash: cargo ${args.join(" ")}\n`);
  const result = spawnSync(cargo, args, {
    cwd: dashDirectory,
    env,
    stdio: "inherit",
  });
  if (result.status !== 0) {
    process.stderr.write(`check:dash: cargo ${args[0]} failed\n`);
    process.exit(result.status ?? 1);
  }
}
