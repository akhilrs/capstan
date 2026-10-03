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

const hash = createHash("sha256")
  .update(fs.readFileSync(path.join(releaseDir, tarball)))
  .digest("hex");
fs.writeFileSync(path.join(releaseDir, "SHA256SUMS"), `${hash}  ${tarball}\n`);

console.log(`tarball:   ${path.join(releaseDir, tarball)}`);
console.log(`checksums: ${path.join(releaseDir, "SHA256SUMS")}`);
console.log("\nTo publish, run:");
console.log(
  `  gh release create v${version} ${path.relative(root, releaseDir)}/${tarball} ${path.relative(root, releaseDir)}/SHA256SUMS --title "v${version}" --generate-notes`,
);
