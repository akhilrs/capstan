// Runs the Rust checks: rustfmt, clippy with warnings denied and the tests, all with the committed lockfile, in the dashboard (dash/)
// and the workspace (rust/); the workspace is also built in release mode for this machine, so the host front end is at
// ${CARGO_TARGET_DIR:-rust/target}/release/cstan once the cli crate exists.
import { spawnSync } from "node:child_process";
import { accessSync, constants, readFileSync } from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const repositoryRoot = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  "..",
);
const dashDirectory = path.join(repositoryRoot, "dash");
const rustDirectory = path.join(repositoryRoot, "rust");

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
const RUST_STEPS = [...STEPS, ["build", "--release", "--locked"]];
const RUNS = [
  [dashDirectory, STEPS],
  [rustDirectory, RUST_STEPS],
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
// One toolchain for both Rust trees.
if (
  !readFileSync(path.join(dashDirectory, "rust-toolchain.toml")).equals(
    readFileSync(path.join(rustDirectory, "rust-toolchain.toml")),
  )
) {
  process.stderr.write(
    "check:dash: rust/rust-toolchain.toml differs from dash/rust-toolchain.toml; they must be identical\n",
  );
  process.exit(1);
}
for (const [cwd, steps] of RUNS) {
  for (const args of steps) {
    process.stdout.write(
      `check:dash: cargo ${args.join(" ")} (${path.basename(cwd)}/)\n`,
    );
    const result = spawnSync(cargo, args, { cwd, env, stdio: "inherit" });
    if (result.status !== 0) {
      process.stderr.write(`check:dash: cargo ${args[0]} failed\n`);
      process.exit(result.status ?? 1);
    }
  }
}
