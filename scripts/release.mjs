import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const releaseDir = path.resolve(
  root,
  process.env.CSTAN_RELEASE_DIR ?? "release",
);
const shrinkwrap = path.join(root, "npm-shrinkwrap.json");
const { name, version } = JSON.parse(
  fs.readFileSync(path.join(root, "package.json"), "utf8"),
);
const tarball = `${name}-${version}.tgz`;

function run(args) {
  const result = spawnSync("npm", args, { cwd: root, stdio: "inherit" });
  if (result.status !== 0)
    throw new Error(`npm ${args.join(" ")} failed (${result.status})`);
}

const lockChanged =
  spawnSync("git", ["diff", "--quiet", "HEAD", "--", "package-lock.json"], {
    cwd: root,
  }).status !== 0;
if (lockChanged) {
  const message =
    "package-lock.json differs from HEAD; the shrinkwrap would pin uncommitted versions";
  if (!process.argv.includes("--allow-dirty-lock")) {
    console.error(`release: ${message} (pass --allow-dirty-lock to continue)`);
    process.exit(1);
  }
  console.warn(`release: warning: ${message}`);
}

try {
  // npm pack runs the prepack script, which builds.
  fs.mkdirSync(releaseDir, { recursive: true });
  fs.copyFileSync(path.join(root, "package-lock.json"), shrinkwrap);
  run(["pack", "--pack-destination", releaseDir]);
} finally {
  fs.rmSync(shrinkwrap, { force: true });
}

// The standalone binaries. build-binary writes them to <root>/release; copy them to the release directory
// when that is a different one (CSTAN_RELEASE_DIR) and remove the originals to save disk.
const targets = ["linux-x64", "linux-arm64"];
const builtDir = path.join(root, "release");
const binaries = targets.map((target) => `cstan-${version}-${target}`);
const build = spawnSync(
  "npm",
  [
    "run",
    "build:binary",
    "--",
    ...targets.flatMap((target) => ["--target", target]),
  ],
  { cwd: root, stdio: "inherit" },
);
if (build.status !== 0)
  throw new Error(`npm run build:binary failed (${build.status})`);
for (const name of binaries) {
  const built = path.join(builtDir, name);
  const kept = path.join(releaseDir, name);
  if (built === kept) continue;
  fs.copyFileSync(built, kept);
  fs.chmodSync(kept, 0o755);
  fs.rmSync(built);
}

const assets = [tarball, ...binaries];
const sums = assets.map((name) => {
  const hash = createHash("sha256")
    .update(fs.readFileSync(path.join(releaseDir, name)))
    .digest("hex");
  return `${hash}  ${name}\n`;
});
fs.writeFileSync(path.join(releaseDir, "SHA256SUMS"), sums.join(""));

const inside = path.relative(root, releaseDir);
const relative = inside.startsWith("..") ? releaseDir : inside;
for (const name of assets)
  console.log(`asset:     ${path.join(releaseDir, name)}`);
console.log(`checksums: ${path.join(releaseDir, "SHA256SUMS")}`);
console.log("\nNothing was published. To publish, run:");
console.log(
  `  gh release create v${version} ${assets.map((name) => `${relative}/${name}`).join(" ")} ${relative}/SHA256SUMS --title "v${version}" --generate-notes`,
);
