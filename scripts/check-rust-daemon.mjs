// Runs the black-box test suites of test/rust-daemon-suites.ts with CSTAN_DAEMON=rust, so `cstan start` and every routed
// command talk to the Rust daemon: the release cstan-daemon of ${CARGO_TARGET_DIR:-rust/target}, which check:dash builds
// (CSTAN_DAEMON_BIN names another). Together with check:dash (the Rust replays) and the staleness tests of npm test this is
// the parity gate that keeps the Node and Rust daemons in step. A shadow run (scripts/shadow-daemon.mjs --self-test) ends it.
//
// Skippable like check:dash: CSTAN_SKIP_RUST_DAEMON_CHECK=1 (or CSTAN_SKIP_DASH_CHECK=1 when cargo is missing) turns a
// missing binary into a loud skip instead of a failure.
import { spawnSync } from "node:child_process";
import { accessSync, constants } from "node:fs";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const repositoryRoot = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  "..",
);

function executable(file) {
  try {
    accessSync(file, constants.X_OK);
    return true;
  } catch {
    return false;
  }
}

const targetDirectory = path.resolve(
  repositoryRoot,
  "rust",
  process.env.CARGO_TARGET_DIR ?? "target",
);
const binary =
  process.env.CSTAN_DAEMON_BIN ??
  path.join(targetDirectory, "release", "cstan-daemon");

if (!path.isAbsolute(binary) || !executable(binary)) {
  if (
    process.env.CSTAN_SKIP_RUST_DAEMON_CHECK === "1" ||
    process.env.CSTAN_SKIP_DASH_CHECK === "1"
  ) {
    process.stderr.write(
      `check:rust-daemon SKIPPED: ${binary} is not an executable (CSTAN_SKIP_RUST_DAEMON_CHECK=1); the Rust daemon was NOT checked\n`,
    );
    process.exit(0);
  }
  process.stderr.write(
    `check:rust-daemon: ${binary} is not an executable; run npm run check:dash first (it builds the release binaries) or set CSTAN_DAEMON_BIN\n`,
  );
  process.exit(1);
}

const run = (command, args, env = process.env) =>
  spawnSync(command, args, { cwd: repositoryRoot, env, stdio: "inherit" });

process.stdout.write("check:rust-daemon: npm run build\n");
const built = run("npm", ["run", "build"]);
if (built.status !== 0) {
  process.stderr.write("check:rust-daemon: the build failed\n");
  process.exit(built.status ?? 1);
}

const { RUST_DAEMON_SUITES, RUST_DAEMON_SKIPS } = await import(
  pathToFileURL(
    path.join(repositoryRoot, "dist", "test", "rust-daemon-suites.js"),
  ).href
);
for (const [suite, reason] of Object.entries(RUST_DAEMON_SKIPS))
  process.stdout.write(`check:rust-daemon: skipping ${suite}: ${reason}\n`);
const files = RUST_DAEMON_SUITES.filter(
  (name) => !(name in RUST_DAEMON_SKIPS),
).map((name) => path.join("dist", "test", `${name}.js`));

process.stdout.write(
  `check:rust-daemon: CSTAN_DAEMON=rust ${binary}\n  ${files.join("\n  ")}\n`,
);
const tested = run(process.execPath, ["--test", ...files], {
  ...process.env,
  CSTAN_DAEMON: "rust",
  CSTAN_DAEMON_BIN: binary,
});
if (tested.status !== 0) {
  process.stderr.write(
    "check:rust-daemon: a suite failed with the Rust daemon\n",
  );
  process.exit(tested.status ?? 1);
}

// The shadow run on a small generated ledger: the Node daemon and the Rust daemon answer the same wire requests over copies
// of it and no response or table may differ (scripts/shadow-daemon.mjs; the large run is a test of cstan-daemon, run by hand).
process.stdout.write("check:rust-daemon: shadow run (--self-test)\n");
const shadowed = run(
  process.execPath,
  [path.join("scripts", "shadow-daemon.mjs"), "--self-test"],
  { ...process.env, CSTAN_DAEMON_BIN: binary },
);
if (shadowed.status !== 0) {
  process.stderr.write(
    "check:rust-daemon: the shadow run found a difference\n",
  );
  process.exit(shadowed.status ?? 1);
}
