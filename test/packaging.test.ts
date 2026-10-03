import assert from "node:assert/strict";
import { createHash } from "node:crypto";
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
    "npm-shrinkwrap.json",
  ]);
  assert.equal(pkg.private, true);
  assert.deepEqual(pkg.bin, { cstan: "./dist/src/cli.js" });
  assert.equal(pkg.scripts.prepack, "npm run build");
  assert.equal(pkg.scripts.release, "node scripts/release.mjs");
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

/** A temp git repo holding a copy of the release script, a committed lockfile and a stub npm. */
function releaseSandbox(): {
  dir: string;
  run(
    args: string[],
    env?: Record<string, string>,
  ): ReturnType<typeof spawnSync>;
  cleanup(): void;
} {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "cstan-release-"));
  fs.mkdirSync(path.join(dir, "scripts"));
  fs.mkdirSync(path.join(dir, "bin"));
  fs.copyFileSync(
    path.join(root, "scripts", "release.mjs"),
    path.join(dir, "scripts", "release.mjs"),
  );
  fs.writeFileSync(
    path.join(dir, "package.json"),
    JSON.stringify({ name: "stub-pkg", version: "1.0.0" }),
  );
  fs.writeFileSync(path.join(dir, "package-lock.json"), '{"lock":1}\n');
  // Fails only for `pack`, recording whether the shrinkwrap exists at that moment.
  fs.writeFileSync(
    path.join(dir, "bin", "npm"),
    `#!/bin/sh
if [ "$1" = pack ]; then
  if [ -f npm-shrinkwrap.json ]; then echo present > pack-saw-shrinkwrap; else echo absent > pack-saw-shrinkwrap; fi
  if [ -d "$3" ]; then echo present > pack-saw-destination; else echo absent > pack-saw-destination; fi
  if [ -n "$STUB_PACK_OK" ] && [ -d "$3" ]; then printf tarball > "$3/stub-pkg-1.0.0.tgz"; exit 0; fi
  exit 1
fi
exit 0
`,
    { mode: 0o755 },
  );
  const git = (...args: string[]) =>
    spawnSync("git", ["-c", "user.email=t@t", "-c", "user.name=t", ...args], {
      cwd: dir,
    });
  git("init", "-q");
  git("add", "package.json", "package-lock.json", "scripts");
  git("commit", "-q", "-m", "init");
  return {
    dir,
    run: (args, extra = {}) =>
      spawnSync(process.execPath, ["scripts/release.mjs", ...args], {
        cwd: dir,
        env: {
          ...process.env,
          ...extra,
          PATH: `${path.join(dir, "bin")}:${process.env.PATH}`,
        },
        encoding: "utf8",
      }),
    cleanup: () => fs.rmSync(dir, { recursive: true, force: true }),
  };
}

test("release script removes npm-shrinkwrap.json after npm pack fails", () => {
  const box = releaseSandbox();
  try {
    const result = box.run([]);
    assert.notEqual(result.status, 0);
    assert.equal(
      fs.readFileSync(path.join(box.dir, "pack-saw-shrinkwrap"), "utf8"),
      "present\n",
    );
    assert.equal(
      fs.readFileSync(path.join(box.dir, "pack-saw-destination"), "utf8"),
      "present\n",
    );
    assert.equal(
      fs.existsSync(path.join(box.dir, "npm-shrinkwrap.json")),
      false,
    );
  } finally {
    box.cleanup();
  }
});

test("release script refuses a modified package-lock.json unless --allow-dirty-lock", () => {
  const box = releaseSandbox();
  try {
    fs.writeFileSync(path.join(box.dir, "package-lock.json"), '{"lock":2}\n');
    const refused = box.run([]);
    assert.notEqual(refused.status, 0);
    assert.match(
      String(refused.stderr),
      /package-lock\.json differs from HEAD/,
    );
    assert.equal(
      fs.existsSync(path.join(box.dir, "pack-saw-shrinkwrap")),
      false,
    );

    const allowed = box.run(["--allow-dirty-lock"]);
    assert.match(
      String(allowed.stderr),
      /warning: package-lock\.json differs from HEAD/,
    );
    // It went on to npm pack (the stub then fails it).
    assert.equal(
      fs.existsSync(path.join(box.dir, "pack-saw-shrinkwrap")),
      true,
    );
    assert.equal(
      fs.existsSync(path.join(box.dir, "npm-shrinkwrap.json")),
      false,
    );
  } finally {
    box.cleanup();
  }
});

test("release script builds only through prepack", () => {
  const script = fs.readFileSync(
    path.join(root, "scripts", "release.mjs"),
    "utf8",
  );
  assert.doesNotMatch(script, /"build"/);
});

test("release script creates a missing nested release dir and writes SHA256SUMS", () => {
  const box = releaseSandbox();
  try {
    const result = box.run([], {
      STUB_PACK_OK: "1",
      CSTAN_RELEASE_DIR: "out/nested/release",
    });
    assert.equal(result.status, 0, String(result.stderr));
    const sums = fs.readFileSync(
      path.join(box.dir, "out", "nested", "release", "SHA256SUMS"),
      "utf8",
    );
    const hash = createHash("sha256").update("tarball").digest("hex");
    assert.equal(sums, `${hash}  stub-pkg-1.0.0.tgz\n`);
    assert.equal(
      fs.existsSync(path.join(box.dir, "npm-shrinkwrap.json")),
      false,
    );
  } finally {
    box.cleanup();
  }
});
