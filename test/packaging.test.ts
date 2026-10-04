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
    "LICENSE",
    "npm-shrinkwrap.json",
  ]);
  assert.equal(pkg.private, true);
  assert.equal(pkg.license, "MIT");
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
  git(...args: string[]): string;
  cleanup(): void;
} {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "cstan-release-"));
  fs.mkdirSync(path.join(dir, "scripts"));
  fs.mkdirSync(path.join(dir, "bin"));
  for (const file of ["release.mjs", "release-notes.mjs"])
    fs.copyFileSync(
      path.join(root, "scripts", file),
      path.join(dir, "scripts", file),
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
V=$(node -p "require('./package.json').version")
if [ "$1" = pack ]; then
  if [ -f npm-shrinkwrap.json ]; then echo present > pack-saw-shrinkwrap; else echo absent > pack-saw-shrinkwrap; fi
  if [ -d "$3" ]; then echo present > pack-saw-destination; else echo absent > pack-saw-destination; fi
  if [ -n "$STUB_PACK_OK" ] && [ -d "$3" ]; then printf tarball > "$3/stub-pkg-$V.tgz"; exit 0; fi
  exit 1
fi
if [ "$1" = run ] && [ "$2" = build:binary ]; then
  if [ -n "$STUB_BUILD_FAIL" ]; then exit 1; fi
  mkdir -p release
  for t in linux-x64 linux-arm64; do printf "binary-$t" > "release/cstan-$V-$t"; done
  exit 0
fi
exit 0
`,
    { mode: 0o755 },
  );
  const git = (...args: string[]) =>
    spawnSync("git", ["-c", "user.email=t@t", "-c", "user.name=t", ...args], {
      cwd: dir,
      encoding: "utf8",
    });
  git("init", "-q");
  git("add", "package.json", "package-lock.json", "scripts");
  git("commit", "-q", "-m", "chore: init");
  git("tag", "-a", "v1.0.0", "-m", "v1.0.0");
  fs.writeFileSync(path.join(dir, "feature.txt"), "x\n");
  git("add", "feature.txt");
  git("commit", "-q", "-m", "feat(core): add a feature");
  return {
    dir,
    git: (...args) => String(git(...args).stdout).trim(),
    run: (args, extra = {}) =>
      spawnSync(process.execPath, ["scripts/release.mjs", ...args], {
        cwd: dir,
        env: {
          ...process.env,
          GIT_AUTHOR_NAME: "t",
          GIT_AUTHOR_EMAIL: "t@t",
          GIT_COMMITTER_NAME: "t",
          GIT_COMMITTER_EMAIL: "t@t",
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
    assert.match(
      String(allowed.stderr),
      /uncommitted package-lock\.json changes will be included in the release commit/,
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

test("release leaves no commit, tag or file change when npm pack or build:binary fails", () => {
  for (const env of [{}, { STUB_PACK_OK: "1", STUB_BUILD_FAIL: "1" }]) {
    const box = releaseSandbox();
    try {
      const head = box.git("rev-parse", "HEAD");
      const result = box.run([], env);
      assert.notEqual(result.status, 0);
      assert.match(
        String(result.stderr),
        /build failed\..*restored; nothing was committed or tagged/s,
      );
      assert.equal(box.git("rev-parse", "HEAD"), head);
      assert.equal(box.git("tag", "--list", "v1.1.0"), "");
      assert.equal(
        box.git("status", "--porcelain", "--untracked-files=no"),
        "",
      );
      assert.equal(fs.existsSync(path.join(box.dir, "CHANGELOG.md")), false);
      // A rerun is not refused because of a leftover tag.
      const rerun = box.run([], { STUB_PACK_OK: "1" });
      assert.equal(rerun.status, 0, String(rerun.stderr));
      assert.equal(box.git("cat-file", "-t", "v1.1.0"), "tag");
    } finally {
      box.cleanup();
    }
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
    const sha = (content: string): string =>
      createHash("sha256").update(content).digest("hex");
    assert.equal(
      sums,
      `${sha("tarball")}  stub-pkg-1.1.0.tgz\n` +
        `${sha("binary-linux-x64")}  cstan-1.1.0-linux-x64\n` +
        `${sha("binary-linux-arm64")}  cstan-1.1.0-linux-arm64\n`,
    );
    assert.equal(
      fs.existsSync(path.join(box.dir, "release", "cstan-1.1.0-linux-x64")),
      false,
    );
    assert.equal(
      fs.existsSync(path.join(box.dir, "npm-shrinkwrap.json")),
      false,
    );
  } finally {
    box.cleanup();
  }
});

function lastCommitFiles(box: ReturnType<typeof releaseSandbox>): string[] {
  return box.git("show", "--name-only", "--format=", "HEAD").split("\n").sort();
}

test("release --dry-run prints the versions and the section and changes nothing", () => {
  const box = releaseSandbox();
  try {
    const head = box.git("rev-parse", "HEAD");
    const result = box.run(["--dry-run"]);
    assert.equal(result.status, 0, String(result.stderr));
    const out = String(result.stdout);
    assert.match(out, /current: 1\.0\.0/);
    assert.match(out, /next:\s+1\.1\.0 \(minor\)/);
    assert.match(out, /### Features\n\n- \*\*core:\*\* add a feature/);
    assert.equal(box.git("rev-parse", "HEAD"), head);
    assert.equal(box.git("tag", "--list", "v1.1.0"), "");
    assert.equal(box.git("status", "--porcelain", "--untracked-files=no"), "");
    assert.equal(fs.existsSync(path.join(box.dir, "CHANGELOG.md")), false);
    assert.equal(
      fs.existsSync(path.join(box.dir, "pack-saw-shrinkwrap")),
      false,
    );
    assert.equal(fs.existsSync(path.join(box.dir, "release")), false);
  } finally {
    box.cleanup();
  }
});

test("release bumps package.json and the lock, commits, tags and builds through the stub", () => {
  const box = releaseSandbox();
  try {
    fs.writeFileSync(
      path.join(box.dir, "package-lock.json"),
      JSON.stringify({
        name: "stub-pkg",
        version: "1.0.0",
        packages: { "": { name: "stub-pkg", version: "1.0.0" } },
      }),
    );
    box.git("commit", "-q", "-am", "chore: lock");
    box.git("tag", "-d", "v1.0.0");
    box.git("tag", "-a", "v1.0.0", "-m", "v1.0.0", "HEAD~2");
    const result = box.run([], { STUB_PACK_OK: "1" });
    assert.equal(result.status, 0, String(result.stderr));
    const pkg = JSON.parse(
      fs.readFileSync(path.join(box.dir, "package.json"), "utf8"),
    );
    assert.equal(pkg.version, "1.1.0");
    const lock = JSON.parse(
      fs.readFileSync(path.join(box.dir, "package-lock.json"), "utf8"),
    );
    assert.equal(lock.version, "1.1.0");
    assert.equal(lock.packages[""].version, "1.1.0");
    assert.equal(
      box.git("log", "-1", "--format=%B").trim(),
      "chore(release): v1.1.0",
    );
    assert.deepEqual(lastCommitFiles(box), [
      "CHANGELOG.md",
      "package-lock.json",
      "package.json",
    ]);
    assert.equal(box.git("cat-file", "-t", "v1.1.0"), "tag");
    assert.match(
      fs.readFileSync(path.join(box.dir, "CHANGELOG.md"), "utf8"),
      /^# Changelog\n\n## 1\.1\.0 \(\d{4}-\d\d-\d\d\)/,
    );
    assert.ok(
      fs.existsSync(path.join(box.dir, "release", "SHA256SUMS")),
      "pack and build:binary ran through the stub",
    );
    assert.match(
      fs.readFileSync(path.join(box.dir, "release", "SHA256SUMS"), "utf8"),
      /stub-pkg-1\.1\.0\.tgz\n.*cstan-1\.1\.0-linux-x64/s,
    );
  } finally {
    box.cleanup();
  }
});

test("release --version overrides the computed version", () => {
  const box = releaseSandbox();
  try {
    const result = box.run(["--version", "2.0.0"], { STUB_PACK_OK: "1" });
    assert.equal(result.status, 0, String(result.stderr));
    assert.equal(box.git("cat-file", "-t", "v2.0.0"), "tag");
    assert.equal(
      JSON.parse(fs.readFileSync(path.join(box.dir, "package.json"), "utf8"))
        .version,
      "2.0.0",
    );
  } finally {
    box.cleanup();
  }
});

test("release refuses nothing releasable, an existing tag and a dirty tree", () => {
  const box = releaseSandbox();
  try {
    box.git("tag", "v1.1.0");
    const exists = box.run(["--version", "1.1.0"]);
    assert.notEqual(exists.status, 0);
    assert.match(String(exists.stderr), /tag v1\.1\.0 already exists/);
    box.git("tag", "-d", "v1.1.0");

    fs.appendFileSync(path.join(box.dir, "feature.txt"), "more\n");
    const dirty = box.run([]);
    assert.notEqual(dirty.status, 0);
    assert.match(String(dirty.stderr), /uncommitted changes/);
    box.git("checkout", "--", "feature.txt");

    box.git("commit", "-q", "--allow-empty", "-m", "docs: only docs");
    box.git("tag", "-a", "-f", "v1.0.0", "-m", "v1.0.0");
    const none = box.run([]);
    assert.notEqual(none.status, 0);
    assert.match(String(none.stderr), /nothing to release/);
    assert.equal(
      fs.existsSync(path.join(box.dir, "pack-saw-shrinkwrap")),
      false,
    );
    assert.equal(box.git("tag", "--list", "v1.0.1"), "");
  } finally {
    box.cleanup();
  }
});
