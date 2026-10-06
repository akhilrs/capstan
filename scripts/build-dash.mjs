#!/usr/bin/env node
// Builds cstan-dash, the Rust dashboard, with cargo. Without --target it builds for this machine into
// dash/target/release/cstan-dash (where `cstan dash` finds it in a source checkout). With --target
// linux-x64 or linux-arm64 it builds the static musl binary and copies it to
// release/cstan-dash-<version>-<target>. See docs/binary.md.
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const releaseDir = path.join(root, "release");
const dashDir = path.join(root, "dash");
const targetDir = path.resolve(
  dashDir,
  process.env.CARGO_TARGET_DIR ?? "target",
);
const version = JSON.parse(
  fs.readFileSync(path.join(root, "package.json"), "utf8"),
).version;

const TARGETS = {
  "linux-x64": "x86_64-unknown-linux-musl",
  "linux-arm64": "aarch64-unknown-linux-musl",
};

function parseTargets(argv) {
  const requested = [];
  for (let i = 0; i < argv.length; i += 1) {
    if (argv[i] === "--target") requested.push(argv[(i += 1)]);
    else if (argv[i].startsWith("--target="))
      requested.push(argv[i].slice("--target=".length));
    else throw new Error(`unknown argument ${argv[i]}`);
  }
  for (const name of requested)
    if (!(name in TARGETS))
      throw new Error(
        `unknown target ${name}; use one of ${Object.keys(TARGETS).join(", ")}`,
      );
  return requested;
}

function cargo(args) {
  const result = spawnSync(
    "cargo",
    ["build", "--release", "--locked", ...args],
    {
      cwd: dashDir,
      stdio: "inherit",
      env: { ...process.env, CSTAN_VERSION: version },
    },
  );
  if (result.error?.code === "ENOENT")
    throw new Error("cargo not found; install Rust (https://rustup.rs)");
  if (result.status !== 0)
    throw new Error(`cargo build exited ${result.status}`);
}

try {
  const targets = parseTargets(process.argv.slice(2));
  if (targets.length === 0) {
    cargo([]);
    console.log(`built ${path.join(targetDir, "release", "cstan-dash")}`);
  }
  for (const target of targets) {
    const triple = TARGETS[target];
    cargo(["--target", triple]);
    fs.mkdirSync(releaseDir, { recursive: true });
    const out = path.join(releaseDir, `cstan-dash-${version}-${target}`);
    fs.copyFileSync(path.join(targetDir, triple, "release", "cstan-dash"), out);
    fs.chmodSync(out, 0o755);
    console.log(`built ${out}`);
  }
} catch (error) {
  console.error(`build-dash: ${error.message}`);
  process.exit(1);
}
