import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const root = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  "..",
  "..",
);
const cli = path.join(root, "dist", "src", "cli.js");
const pkg = JSON.parse(
  fs.readFileSync(path.join(root, "package.json"), "utf8"),
);

function cstan(args: string[]) {
  const home = fs.mkdtempSync(path.join(os.tmpdir(), "cstan-pkg-"));
  try {
    return spawnSync(process.execPath, [cli, ...args], {
      cwd: "/",
      env: { PATH: process.env.PATH, HOME: home },
      encoding: "utf8",
    });
  } finally {
    fs.rmSync(home, { recursive: true, force: true });
  }
}

for (const flag of ["--version", "-V", "version"])
  test(`cstan ${flag} prints the package version`, () => {
    const result = cstan([flag]);
    assert.equal(result.status, 0);
    assert.equal(result.stdout, `cstan ${pkg.version}\n`);
  });

for (const flag of ["--help", "-h", "help"])
  test(`cstan ${flag} prints usage to stdout`, () => {
    const result = cstan([flag]);
    assert.equal(result.status, 0);
    assert.match(result.stdout, /^usage: cstan init/);
  });

test("bare cstan and unknown commands keep the usage failure", () => {
  for (const args of [[], ["nonsense"]]) {
    const result = cstan(args);
    assert.equal(result.status, 2);
    assert.equal(result.stdout, "");
    assert.match(result.stderr, /^usage: cstan init/);
  }
});

test("package.json packaging invariants", () => {
  assert.deepEqual(pkg.files, [
    "dist/src",
    "dist/migrations",
    "README.md",
    "LICENSE",
    "npm-shrinkwrap.json",
  ]);
  assert.equal(pkg.private, true);
  assert.equal(pkg.license, "MIT");
  assert.deepEqual(pkg.bin, { cstan: "./dist/src/cli.js" });
  assert.equal(pkg.scripts.prepack, "npm run build");
  for (const hook of ["prepare", "postinstall", "preinstall", "install"])
    assert.equal(pkg.scripts[hook], undefined, hook);
  assert.match(pkg.repository.url, /akhilrs\/capstan/);
  assert.match(pkg.homepage, /akhilrs\/capstan/);
  for (const name of fs.readdirSync(path.join(root, "migrations")))
    assert.ok(fs.existsSync(path.join(root, "dist", "migrations", name)), name);
  assert.match(
    fs.readFileSync(path.join(root, ".gitignore"), "utf8"),
    /^\/release\/$/m,
  );
});

test("package.json builds the front end with scripts/build-cli.mjs", () => {
  assert.equal(pkg.scripts["build:cli"], "node scripts/build-cli.mjs");
  assert.ok(fs.existsSync(path.join(root, "scripts", "build-cli.mjs")));
  assert.match(pkg.scripts["format:check"], /scripts\/build-cli\.mjs/);
});

test("VERSION is the version package.json and the lockfile carry", () => {
  const version = fs.readFileSync(path.join(root, "VERSION"), "utf8").trim();
  const lock = JSON.parse(
    fs.readFileSync(path.join(root, "package-lock.json"), "utf8"),
  );
  assert.equal(pkg.version, version);
  assert.equal(lock.version, version);
  assert.equal(lock.packages[""].version, version);
});
