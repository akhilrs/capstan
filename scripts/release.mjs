import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import {
  compareVersions,
  dedupeCommits,
  lastReleaseTag,
  nextVersion,
  parseCommit,
  renderChangelogSection,
} from "./release-notes.mjs";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const releaseDir = path.resolve(
  root,
  process.env.CSTAN_RELEASE_DIR ?? "release",
);
const shrinkwrap = path.join(root, "npm-shrinkwrap.json");
const argv = process.argv.slice(2);
const dryRun = argv.includes("--dry-run");
const withDash = !argv.includes("--no-dash");

function fail(message) {
  console.error(`release: ${message}`);
  process.exit(1);
}

function git(args, options = {}) {
  const result = spawnSync("git", args, {
    cwd: root,
    encoding: "utf8",
    ...options,
  });
  if (result.status !== 0 && !options.allowFailure)
    throw new Error(
      `git ${args.join(" ")} failed (${result.status}): ${result.stderr}`,
    );
  return result;
}

function run(args) {
  const result = spawnSync("npm", args, { cwd: root, stdio: "inherit" });
  if (result.status !== 0)
    throw new Error(`npm ${args.join(" ")} failed (${result.status})`);
}

const lockChanged =
  git(["diff", "--quiet", "HEAD", "--", "package-lock.json"], {
    allowFailure: true,
  }).status !== 0;
if (lockChanged) {
  const message =
    "package-lock.json differs from HEAD; the shrinkwrap would pin uncommitted versions";
  if (!argv.includes("--allow-dirty-lock"))
    fail(`${message} (pass --allow-dirty-lock to continue)`);
  console.warn(
    `release: warning: ${message}; with --allow-dirty-lock those uncommitted package-lock.json changes will be included in the release commit`,
  );
}

const dirty = git(["status", "--porcelain", "--untracked-files=no"])
  .stdout.split("\n")
  .filter(
    (line) =>
      line !== "" && !(lockChanged && line.endsWith(" package-lock.json")),
  );
if (dirty.length > 0) {
  const message = `tracked files have uncommitted changes:\n${dirty.join("\n")}`;
  if (!dryRun) fail(message);
  console.warn(`release: warning: ${message}`);
}

// Work out the next version from the commits since the last release tag.
const pkgPath = path.join(root, "package.json");
const current = JSON.parse(fs.readFileSync(pkgPath, "utf8")).version;
const lastTag = lastReleaseTag(root);
const log = git([
  "log",
  "--format=%H%x1f%P%x1f%B%x1e",
  lastTag ? `${lastTag}..HEAD` : "HEAD",
]).stdout;
const commits = [];
const ignored = [];
for (const entry of log.split("\x1e")) {
  const [sha, parents, message] = entry.replace(/^\n/, "").split("\x1f");
  if (!sha) continue;
  const subject = message.split("\n")[0];
  if (parents.trim().split(/\s+/).length > 1) {
    ignored.push(`${sha.slice(0, 7)} ${subject} (merge)`);
  } else if (!parseCommit(message)) {
    ignored.push(`${sha.slice(0, 7)} ${subject}`);
  } else if (!subject.startsWith("chore(release):")) {
    commits.push({ sha, message });
  }
}
if (ignored.length > 0)
  console.warn(
    `release: warning: ignoring ${ignored.length} merge or non-conforming commit(s):\n  ${ignored.join("\n  ")}`,
  );

let next = nextVersion(current, commits);
const override = argv.indexOf("--version");
if (override !== -1) {
  const wanted = argv[override + 1] ?? "";
  if (!/^\d+\.\d+\.\d+$/.test(wanted))
    fail(`--version needs X.Y.Z, got '${wanted}'`);
  if (compareVersions(wanted, current) <= 0)
    fail(`--version ${wanted} must be higher than the current ${current}`);
  next = { version: wanted, level: "override" };
}
if (!next)
  fail(
    `nothing to release since ${lastTag ?? "the first commit"}: no feat, fix, perf, revert or breaking commits`,
  );
const version = next.version;
const tagName = `v${version}`;
if (
  git(["rev-parse", "-q", "--verify", `refs/tags/${tagName}`], {
    allowFailure: true,
  }).status === 0
)
  fail(`tag ${tagName} already exists`);

const date = new Date().toISOString().slice(0, 10);
const section = renderChangelogSection(version, date, commits);
console.log(`current: ${current}`);
console.log(`next:    ${version} (${next.level})`);
const listed = dedupeCommits(commits).length;
console.log(
  `commits: ${commits.length} counted, ${listed} listed (${commits.length - listed} duplicate(s) collapsed)`,
);
console.log(`\n${section}`);

const targets = ["linux-x64", "linux-arm64"];
const dashTargets = targets;
if (dryRun) {
  console.log("Assets that would be built:");
  console.log(`  capstan-controller-${version}.tgz`);
  for (const target of targets) console.log(`  cstan-${version}-${target}`);
  if (withDash)
    for (const target of dashTargets)
      console.log(`  cstan-dash-${version}-${target}`);
  else console.log("  (cstan-dash skipped: --no-dash)");
  console.log("Dry run: nothing was changed.");
  process.exit(0);
}

// Write the version into package.json and the lockfile, prepend the changelog and build every asset.
// Nothing is committed or tagged until all of that succeeded; a failure restores the files it touched.
const changelogPath = path.join(root, "CHANGELOG.md");
const lockPath = path.join(root, "package-lock.json");
if (
  withDash &&
  spawnSync("cargo", ["--version"], { stdio: "ignore" }).status !== 0
)
  fail(
    "cargo not found, so cstan-dash cannot be built; install Rust (https://rustup.rs) or pass --no-dash to release without it",
  );
const originals = new Map(
  [pkgPath, lockPath, changelogPath].map((file) => [
    file,
    fs.existsSync(file) ? fs.readFileSync(file) : null,
  ]),
);
function restoreFiles() {
  for (const [file, content] of originals) {
    if (content === null) fs.rmSync(file, { force: true });
    else fs.writeFileSync(file, content);
  }
}
function setVersion(file) {
  const data = JSON.parse(fs.readFileSync(file, "utf8"));
  if (data.version === undefined) return;
  data.version = version;
  if (data.packages?.[""]) data.packages[""].version = version;
  fs.writeFileSync(file, `${JSON.stringify(data, null, 2)}\n`);
}

// The standalone binaries. build-binary writes them to <root>/release; copy them to the release directory
// when that is a different one (CSTAN_RELEASE_DIR) and remove the originals to save disk.
const builtDir = path.join(root, "release");
const binaries = targets.map((target) => `cstan-${version}-${target}`);

function buildAssets() {
  setVersion(pkgPath);
  setVersion(lockPath);
  const existing =
    originals.get(changelogPath)?.toString("utf8") ?? "# Changelog\n";
  const at = existing.search(/^## /m);
  fs.writeFileSync(
    changelogPath,
    at === -1
      ? `${existing.trimEnd()}\n\n${section}`
      : `${existing.slice(0, at)}${section}\n${existing.slice(at)}`,
  );

  // Read after the bump: the tarball name depends on the new version.
  const { name } = JSON.parse(fs.readFileSync(pkgPath, "utf8"));
  const tarball = `${name}-${version}.tgz`;
  try {
    // npm pack runs the prepack script, which builds.
    fs.mkdirSync(releaseDir, { recursive: true });
    fs.copyFileSync(lockPath, shrinkwrap);
    run(["pack", "--pack-destination", releaseDir]);
  } finally {
    fs.rmSync(shrinkwrap, { force: true });
  }

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
  const moveToReleaseDir = (names) => {
    for (const name of names) {
      const built = path.join(builtDir, name);
      const kept = path.join(releaseDir, name);
      if (built === kept) continue;
      fs.copyFileSync(built, kept);
      fs.chmodSync(kept, 0o755);
      fs.rmSync(built);
    }
  };
  moveToReleaseDir(binaries);
  const dashes = [];
  if (withDash) {
    const dash = spawnSync(
      "npm",
      [
        "run",
        "build:dash",
        "--",
        ...dashTargets.flatMap((target) => ["--target", target]),
      ],
      { cwd: root, stdio: "inherit" },
    );
    if (dash.status !== 0)
      throw new Error(`npm run build:dash failed (${dash.status})`);
    for (const target of dashTargets)
      dashes.push(`cstan-dash-${version}-${target}`);
    moveToReleaseDir(dashes);
  }
  return [tarball, ...binaries, ...dashes];
}

let assets;
try {
  assets = buildAssets();
} catch (error) {
  restoreFiles();
  console.error(`release: ${error.message}`);
  console.error(
    "release: build failed. package.json, package-lock.json and CHANGELOG.md were restored; nothing was committed or tagged.",
  );
  process.exit(1);
}

// Every asset is built: commit the bump and tag it.
try {
  git(["add", "package.json", "package-lock.json", "CHANGELOG.md"]);
  git(["commit", "-q", "-m", `chore(release): ${tagName}`]);
  git(["tag", "-a", tagName, "-m", tagName]);
} catch (error) {
  console.error(`release: ${error.message}`);
  console.error(
    `release: committing or tagging failed. To undo it, run:\n  git tag -d ${tagName}\n  git reset --hard HEAD~1   # only if the chore(release) commit was created`,
  );
  process.exit(1);
}
console.log(
  `committed and tagged ${tagName} (local only; nothing was pushed)\n`,
);

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
