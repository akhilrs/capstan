import assert from "node:assert/strict";

process.env.CAPSTAN_LAUNCH = "off";
delete process.env.CAPSTAN_TOKEN;
delete process.env.CAPSTAN_SOCKET;
import { spawnSync } from "node:child_process";
import {
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import os from "node:os";
import path from "node:path";
import { test } from "node:test";
import { checkGitRequirement } from "../src/git-requirement.js";
import { LauncherError, defaultGit } from "../src/launcher.js";

const cli = path.resolve("dist/src/cli.js");
const identity = {
  GIT_AUTHOR_NAME: "t",
  GIT_AUTHOR_EMAIL: "t@example.com",
  GIT_COMMITTER_NAME: "t",
  GIT_COMMITTER_EMAIL: "t@example.com",
};

function invoke(cwd: string, ...args: string[]) {
  return spawnSync(process.execPath, [cli, ...args], {
    cwd,
    encoding: "utf8",
    env: { ...process.env, ...identity },
  });
}

function git(cwd: string, ...args: string[]) {
  const result = spawnSync("git", args, {
    cwd,
    encoding: "utf8",
    env: { ...process.env, ...identity },
  });
  assert.equal(result.status, 0, result.stderr);
  return result.stdout.trim();
}

function withDir(run: (dir: string) => void): void {
  const dir = mkdtempSync(path.join(os.tmpdir(), "cstan-gitreq-"));
  try {
    run(dir);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
}

test("a folder that is not a repository is named with the exact fix", () => {
  withDir((dir) => {
    const check = checkGitRequirement(dir);
    assert.equal(check.ok, false);
    if (check.ok) return;
    assert.equal(check.problem, "not_a_repository");
    assert.match(check.message, /git init/);
    assert.match(check.message, /git add -A/);
    assert.match(check.message, /git commit -m "chore: initial commit"/);
    assert.match(check.message, /cstan init --git/);
  });
});

test("a repository with an unborn HEAD is reported as having no commit", () => {
  withDir((dir) => {
    git(dir, "init", "--quiet");
    const check = checkGitRequirement(dir);
    assert.equal(check.ok, false);
    if (!check.ok) assert.equal(check.problem, "no_commit");
  });
});

test("a repository with a commit passes, on a branch or detached", () => {
  withDir((dir) => {
    git(dir, "init", "--quiet");
    git(dir, "commit", "--quiet", "--allow-empty", "-m", "x");
    assert.deepEqual(checkGitRequirement(dir), { ok: true, detached: false });
    git(dir, "checkout", "--quiet", "--detach");
    assert.deepEqual(checkGitRequirement(dir), { ok: true, detached: true });
  });
});

test("a project root inside a larger repository is refused", () => {
  withDir((dir) => {
    git(dir, "init", "--quiet");
    git(dir, "commit", "--quiet", "--allow-empty", "-m", "x");
    const sub = path.join(dir, "packages", "app");
    mkdirSync(sub, { recursive: true });
    const check = checkGitRequirement(sub);
    assert.equal(check.ok, false);
    if (!check.ok) {
      assert.equal(check.problem, "subdirectory");
      assert.match(check.message, /top-level folder/);
    }
  });
});

test("a bare repository is refused", () => {
  withDir((dir) => {
    git(dir, "init", "--quiet", "--bare");
    const check = checkGitRequirement(dir);
    assert.equal(check.ok, false);
    if (!check.ok) assert.equal(check.problem, "bare_repository");
  });
});

test("the launcher's git runner fails with the requirement's message", () => {
  withDir((dir) => {
    assert.throws(
      () => defaultGit(dir).headSha(),
      (error: unknown) =>
        error instanceof LauncherError &&
        error.code === "git_requirement" &&
        /git commit -m "chore: initial commit"/.test(error.message),
    );
    git(dir, "init", "--quiet");
    assert.throws(
      () => defaultGit(dir).headSha(),
      (error: unknown) =>
        error instanceof LauncherError && /no commit yet/.test(error.message),
    );
  });
});

test("cstan init outside a repository writes its files and warns", () => {
  withDir((dir) => {
    const init = invoke(dir, "init");
    assert.equal(init.status, 0, init.stderr);
    assert.match(init.stderr, /warning: Capstan needs a git repository/);
    assert.match(init.stderr, /cstan init --git/);
    assert.equal(spawnSync("git", ["-C", dir, "rev-parse"]).status !== 0, true);
  });
});

test("cstan start refuses outside a repository and with an unborn HEAD", () => {
  withDir((dir) => {
    assert.equal(invoke(dir, "init").status, 0);
    const outside = invoke(dir, "start");
    assert.equal(outside.status, 3);
    assert.match(outside.stderr, /not inside a git work tree/);
    git(dir, "init", "--quiet");
    const unborn = invoke(dir, "start");
    assert.equal(unborn.status, 3);
    assert.match(unborn.stderr, /no commit yet/);
  });
});

test("cstan init --git creates the repository and the initial commit, lists the files and keeps .capstan out", () => {
  withDir((dir) => {
    writeFileSync(path.join(dir, "keep.txt"), "x\n");
    writeFileSync(path.join(dir, "skip.log"), "x\n");
    writeFileSync(path.join(dir, ".gitignore"), "*.log\n");
    const init = invoke(dir, "init", "--git");
    assert.equal(init.status, 0, init.stderr);
    assert.doesNotMatch(init.stderr, /warning/);
    assert.match(init.stdout, /Ran git init/);
    assert.match(init.stdout, /Committing \d+ files/);
    assert.match(init.stdout, /\n {2}keep\.txt\n/);
    assert.match(init.stdout, /\n {2}capstan\.toml\n/);
    assert.doesNotMatch(init.stdout, /\n {2}(skip\.log|\.capstan)/);
    assert.match(git(dir, "log", "--format=%s"), /^chore: initial commit$/);
    const tracked = git(dir, "ls-files");
    assert.match(tracked, /keep\.txt/);
    assert.doesNotMatch(tracked, /\.capstan|skip\.log/);
    assert.equal(git(dir, "status", "--porcelain"), "");
    assert.match(
      readFileSync(path.join(dir, ".git", "info", "exclude"), "utf8"),
      /^\/\.capstan\/$/m,
    );
    assert.equal(checkGitRequirement(dir).ok, true);
  });
});

test("cstan init --git leaves an existing history alone", () => {
  withDir((dir) => {
    git(dir, "init", "--quiet");
    writeFileSync(path.join(dir, "a.txt"), "a\n");
    git(dir, "add", "-A");
    git(dir, "commit", "--quiet", "-m", "feat: mine");
    const init = invoke(dir, "init", "--git");
    assert.equal(init.status, 0, init.stderr);
    assert.match(init.stdout, /nothing to commit/);
    assert.equal(git(dir, "rev-list", "--count", "HEAD"), "1");
  });
});

test("cstan init --git on an initialized project only does the git part", () => {
  withDir((dir) => {
    assert.equal(invoke(dir, "init").status, 0);
    const again = invoke(dir, "init", "--git");
    assert.equal(again.status, 0, again.stderr);
    assert.equal(checkGitRequirement(dir).ok, true);
  });
});

test("cstan init --git refuses a subdirectory of a repository and never runs git init there", () => {
  withDir((dir) => {
    git(dir, "init", "--quiet");
    git(dir, "commit", "--quiet", "--allow-empty", "-m", "x");
    const sub = path.join(dir, "app");
    mkdirSync(sub);
    const init = invoke(sub, "init", "--git");
    assert.equal(init.status, 3);
    assert.match(init.stderr, /subdirectory/);
    assert.equal(spawnSync("test", ["-e", path.join(sub, ".git")]).status, 1);
    assert.equal(
      spawnSync("test", ["-e", path.join(sub, ".capstan")]).status,
      1,
    );
  });
});

test("cstan init never runs git without --git", () => {
  withDir((dir) => {
    assert.equal(invoke(dir, "init").status, 0);
    assert.equal(spawnSync("test", ["-e", path.join(dir, ".git")]).status, 1);
  });
});
