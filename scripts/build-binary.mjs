#!/usr/bin/env node
// Builds the standalone cstan binary: an esbuild bundle of src/cli.ts packed into the official Node
// binary as a single-executable application. See docs/binary.md.
import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { build } from "esbuild";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const releaseDir = path.join(root, "release");
const buildDir = path.join(releaseDir, "build");
const cacheDir = path.join(releaseDir, "node-cache");
const version = JSON.parse(
  fs.readFileSync(path.join(root, "package.json"), "utf8"),
).version;

const TARGETS = {
  "linux-x64": { os: "linux", arch: "x64" },
  "linux-arm64": { os: "linux", arch: "arm64" },
  "darwin-x64": { os: "darwin", arch: "x64" },
  "darwin-arm64": { os: "darwin", arch: "arm64" },
};

function parseTargets(argv) {
  const requested = [];
  for (let i = 0; i < argv.length; i += 1) {
    if (argv[i] === "--target") requested.push(argv[(i += 1)]);
    else if (argv[i].startsWith("--target="))
      requested.push(argv[i].slice("--target=".length));
    else throw new Error(`unknown argument ${argv[i]}`);
  }
  const names = requested.length > 0 ? requested : ["linux-x64", "linux-arm64"];
  for (const name of names)
    if (!(name in TARGETS))
      throw new Error(
        `unknown target ${name}; use one of ${Object.keys(TARGETS).join(", ")}`,
      );
  return names;
}

function run(command, args, options = {}) {
  const result = spawnSync(command, args, { stdio: "inherit", ...options });
  if (result.status !== 0)
    throw new Error(`${command} ${args.join(" ")} exited ${result.status}`);
}

async function download(url) {
  const response = await fetch(url);
  if (!response.ok) throw new Error(`GET ${url}: HTTP ${response.status}`);
  return Buffer.from(await response.arrayBuffer());
}

/** The node binary for a target, from the official archive verified against SHASUMS256.txt. */
async function nodeBinary(target) {
  const { os: platform, arch } = TARGETS[target];
  const nodeVersion = process.version;
  const ext = platform === "darwin" ? "tar.gz" : "tar.xz";
  const archive = `node-${nodeVersion}-${platform}-${arch}.${ext}`;
  const out = path.join(cacheDir, `node-${nodeVersion}-${platform}-${arch}`);
  if (fs.existsSync(out)) return out;
  fs.mkdirSync(cacheDir, { recursive: true });
  const base = `https://nodejs.org/dist/${nodeVersion}`;
  const sums = (await download(`${base}/SHASUMS256.txt`)).toString("utf8");
  const line = sums.split("\n").find((l) => l.endsWith(`  ${archive}`));
  if (line === undefined) throw new Error(`${archive} not in SHASUMS256.txt`);
  const expected = line.split(/\s+/)[0];
  console.log(`downloading ${archive}`);
  const bytes = await download(`${base}/${archive}`);
  const actual = createHash("sha256").update(bytes).digest("hex");
  if (actual !== expected)
    throw new Error(`${archive} checksum mismatch: ${actual} != ${expected}`);
  console.log(`verified ${archive} sha256 ${actual}`);
  const tmp = fs.mkdtempSync(path.join(cacheDir, "extract-"));
  try {
    const archivePath = path.join(tmp, archive);
    fs.writeFileSync(archivePath, bytes);
    run("tar", [
      "-xf",
      archivePath,
      "-C",
      tmp,
      `${archive.replace(`.${ext}`, "")}/bin/node`,
    ]);
    fs.renameSync(
      path.join(tmp, archive.replace(`.${ext}`, ""), "bin", "node"),
      out,
    );
  } finally {
    fs.rmSync(tmp, { recursive: true, force: true });
  }
  return out;
}

/** The CommonJS SEA main: writes the ESM bundle asset to a private cache once and imports it. */
const MAIN = `"use strict";
const crypto = require("node:crypto");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const { pathToFileURL } = require("node:url");
const sea = require("node:sea");

function cacheRoot() {
  const xdg = process.env.XDG_CACHE_HOME;
  const base = xdg && path.isAbsolute(xdg) ? xdg : path.join(os.homedir(), ".cache");
  return path.join(base, "capstan");
}

function privateDir(dir) {
  fs.mkdirSync(dir, { recursive: true, mode: 0o700 });
  const stat = fs.lstatSync(dir);
  if (!stat.isDirectory() || stat.isSymbolicLink() || (process.getuid && stat.uid !== process.getuid()))
    throw new Error("cache directory " + dir + " is not a private directory of the current user");
  if ((stat.mode & 0o077) !== 0) fs.chmodSync(dir, 0o700);
}

function extractBundle() {
  const bytes = Buffer.from(sea.getAsset("bundle.mjs"));
  const hash = crypto.createHash("sha256").update(bytes).digest("hex");
  const root = path.join(cacheRoot(), "sea");
  const dir = path.join(root, hash);
  const file = path.join(dir, "cstan.mjs");
  try {
    // Never trust stale content: the cached copy must equal the embedded bytes.
    if (fs.readFileSync(file).equals(bytes)) return file;
  } catch {}
  try {
    privateDir(root);
    privateDir(dir);
    const tmp = path.join(dir, "cstan.mjs.tmp-" + process.pid + "-" + crypto.randomBytes(6).toString("hex"));
    const fd = fs.openSync(tmp, "wx", 0o600);
    try {
      fs.writeFileSync(fd, bytes);
      fs.fsyncSync(fd);
    } finally {
      fs.closeSync(fd);
    }
    fs.renameSync(tmp, file);
  } catch (error) {
    throw new Error("cannot write the bundle cache " + dir + ": " + (error && error.message ? error.message : String(error)));
  }
  return file;
}

let bundle;
try {
  bundle = extractBundle();
} catch (error) {
  process.stderr.write("cstan: " + error.message + "\\n");
  process.exit(1);
}
import(pathToFileURL(bundle).href).catch((error) => {
  process.stderr.write("cstan: " + (error && error.stack ? error.stack : String(error)) + "\\n");
  process.exitCode = 1;
});
`;

async function bundle() {
  fs.rmSync(buildDir, { recursive: true, force: true });
  fs.mkdirSync(buildDir, { recursive: true });
  await build({
    entryPoints: [path.join(root, "src", "cli.ts")],
    outfile: path.join(buildDir, "bundle.mjs"),
    bundle: true,
    platform: "node",
    target: "node24",
    format: "esm",
    // CommonJS packages in the bundle call require(); give them one.
    banner: {
      js: [
        'import { createRequire as __createRequire } from "node:module";',
        'import { pathToFileURL as __pathToFileURL } from "node:url";',
        "const require = __createRequire(process.execPath);",
        "const __importMetaUrl = __pathToFileURL(process.execPath).href;",
      ].join("\n"),
    },
    define: {
      "import.meta.url": "__importMetaUrl",
      __CAPSTAN_VERSION__: JSON.stringify(version),
      "process.env.NODE_ENV": '"production"',
    },
    plugins: [
      {
        // ink loads this optional peer dependency only when DEV=true; the binary never does.
        name: "stub-react-devtools-core",
        setup(b) {
          b.onResolve({ filter: /^react-devtools-core$/ }, () => ({
            path: "react-devtools-core",
            namespace: "stub",
          }));
          b.onLoad({ filter: /.*/, namespace: "stub" }, () => ({
            contents: "export default { connectToDevTools() {} };",
            loader: "js",
          }));
        },
      },
    ],
    logLevel: "warning",
    legalComments: "none",
  });
  fs.writeFileSync(path.join(buildDir, "main.cjs"), MAIN);
  // The blob lists every migration as an asset.
  const config = JSON.parse(
    fs.readFileSync(path.join(root, "scripts", "sea-config.json"), "utf8"),
  );
  for (const name of fs.readdirSync(path.join(root, "migrations")).sort())
    if (name.endsWith(".sql"))
      config.assets[`migrations/${name}`] = `migrations/${name}`;
  // Node resolves relative config paths against the working directory; absolute ones are unambiguous.
  config.main = path.join(root, config.main);
  config.output = path.join(root, config.output);
  for (const [name, file] of Object.entries(config.assets))
    config.assets[name] = path.resolve(root, file);
  const configPath = path.join(buildDir, "sea-config.json");
  fs.writeFileSync(configPath, JSON.stringify(config, null, 2));
  run(process.execPath, ["--experimental-sea-config", configPath], {
    cwd: root,
  });
  return path.join(buildDir, "sea-prep.blob");
}

async function main() {
  const targets = parseTargets(process.argv.slice(2));
  const blob = await bundle();
  fs.mkdirSync(releaseDir, { recursive: true });
  const postject = path.join(root, "node_modules", ".bin", "postject");
  for (const target of targets) {
    const node = await nodeBinary(target);
    const out = path.join(releaseDir, `cstan-${version}-${target}`);
    fs.rmSync(out, { force: true });
    fs.copyFileSync(node, out);
    fs.chmodSync(out, 0o755);
    const darwin = TARGETS[target].os === "darwin";
    if (darwin) {
      if (os.platform() !== "darwin")
        throw new Error("macOS targets can only be built on macOS (codesign)");
      run("codesign", ["--remove-signature", out]);
    }
    run(postject, [
      out,
      "NODE_SEA_BLOB",
      blob,
      "--sentinel-fuse",
      "NODE_SEA_FUSE_fce680ab2cc467b6e072b8b5df1996b2",
      ...(darwin ? ["--macho-segment-name", "NODE_SEA"] : []),
    ]);
    if (darwin) run("codesign", ["--sign", "-", out]);
    console.log(`built ${path.relative(root, out)}`);
  }
  fs.rmSync(buildDir, { recursive: true, force: true });
}

main().catch((error) => {
  console.error(`build-binary: ${error.message}`);
  process.exit(1);
});
