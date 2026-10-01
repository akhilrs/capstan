import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { test } from "node:test";
import {
  GitCheckError,
  cleanGitEnvironment,
  inspectCommit,
} from "../src/git.js";

function git(root: string, ...args: string[]): string {
  return execFileSync("git", ["-C", root, ...args], {
    encoding: "utf8",
    env: {
      ...process.env,
      GIT_AUTHOR_NAME: "t",
      GIT_AUTHOR_EMAIL: "t@e.c",
      GIT_COMMITTER_NAME: "t",
      GIT_COMMITTER_EMAIL: "t@e.c",
    },
  }).trim();
}

interface Repo {
  readonly root: string;
  readonly base: string;
  readonly onBranch: string;
  readonly onMain: string;
  readonly branch: string;
}

function makeRepo(): Repo {
  const root = mkdtempSync(path.join(tmpdir(), "capstan-git-"));
  git(root, "init", "-q", "-b", "main");
  const commit = (file: string, message: string): string => {
    writeFileSync(path.join(root, file), message);
    git(root, "add", file);
    git(root, "commit", "-q", "-m", message);
    return git(root, "rev-parse", "HEAD");
  };
  const base = commit("base.txt", "base");
  const branch = "capstan/developer-1-g1";
  git(root, "checkout", "-q", "-b", branch);
  const onBranch = commit("work.txt", "work on the branch");
  git(root, "checkout", "-q", "main");
  const onMain = commit("main.txt", "later main work");
  return { root, base, onBranch, onMain, branch };
}

test("a commit made on the branch is on it, and the base and main-only commits are not new work", async () => {
  const r = makeRepo();
  try {
    const input = { branch: r.branch, baseSha: r.base };
    const mine = await inspectCommit(r.root, { ...input, sha: r.onBranch });
    assert.deepEqual(mine, {
      commitExists: true,
      branchTip: r.onBranch,
      isAncestorOfTip: true,
      isAncestorOfBase: false,
    });
    const base = await inspectCommit(r.root, { ...input, sha: r.base });
    assert.equal(base.isAncestorOfTip, true);
    assert.equal(base.isAncestorOfBase, true, "the base is not new work");
    const main = await inspectCommit(r.root, { ...input, sha: r.onMain });
    assert.equal(main.commitExists, true);
    assert.equal(
      main.isAncestorOfTip,
      false,
      "a commit only on main is not on the branch",
    );
    const missing = await inspectCommit(r.root, {
      ...input,
      sha: "1".repeat(40),
    });
    assert.equal(missing.commitExists, false);
    assert.equal(missing.isAncestorOfTip, false);
    const tree = git(r.root, "rev-parse", `${r.onBranch}^{tree}`);
    assert.equal(
      (await inspectCommit(r.root, { ...input, sha: tree })).commitExists,
      false,
      "a tree is not a commit",
    );
    const noBranch = await inspectCommit(r.root, {
      branch: "capstan/none-g1",
      baseSha: r.base,
      sha: r.onBranch,
    });
    assert.equal(noBranch.branchTip, null);
    assert.equal(noBranch.isAncestorOfTip, false);
    const noBase = await inspectCommit(r.root, {
      branch: r.branch,
      baseSha: null,
      sha: r.onBranch,
    });
    assert.equal(noBase.isAncestorOfBase, false);
  } finally {
    rmSync(r.root, { recursive: true, force: true });
  }
});

test("bad inputs and unsupported repositories are errors, not verdicts", async () => {
  const r = makeRepo();
  try {
    for (const bad of [
      { branch: r.branch, baseSha: r.base, sha: "B".repeat(40) },
      { branch: r.branch, baseSha: r.base, sha: "abc" },
      { branch: r.branch, baseSha: "abc", sha: r.onBranch },
      { branch: "bad..name", baseSha: r.base, sha: r.onBranch },
      { branch: "a b", baseSha: r.base, sha: r.onBranch },
    ])
      await assert.rejects(
        inspectCommit(r.root, bad),
        GitCheckError,
        JSON.stringify(bad),
      );
    const flagLike = await inspectCommit(r.root, {
      branch: "-flag",
      baseSha: r.base,
      sha: r.onBranch,
    });
    assert.equal(
      flagLike.branchTip,
      null,
      "a name that looks like an option is only a ref name here",
    );
    await assert.rejects(
      inspectCommit(path.join(r.root, "nowhere"), {
        branch: r.branch,
        baseSha: r.base,
        sha: r.onBranch,
      }),
      GitCheckError,
    );
    const sha256Root = mkdtempSync(path.join(tmpdir(), "capstan-git256-"));
    try {
      git(sha256Root, "init", "-q", "--object-format=sha256");
      await assert.rejects(
        inspectCommit(sha256Root, {
          branch: r.branch,
          baseSha: null,
          sha: "1".repeat(40),
        }),
        /only sha1 repositories/,
      );
    } finally {
      rmSync(sha256Root, { recursive: true, force: true });
    }
  } finally {
    rmSync(r.root, { recursive: true, force: true });
  }
});

test("the daemon's own git variables and a replace ref cannot steer the check", async () => {
  const r = makeRepo();
  const other = makeRepo();
  const saved = { dir: process.env.GIT_DIR, tree: process.env.GIT_WORK_TREE };
  try {
    process.env.GIT_DIR = path.join(other.root, ".git");
    process.env.GIT_WORK_TREE = other.root;
    const found = await inspectCommit(r.root, {
      branch: r.branch,
      baseSha: r.base,
      sha: r.onBranch,
    });
    assert.equal(
      found.commitExists,
      true,
      "GIT_DIR pointing at another repository is ignored",
    );
    assert.equal(found.isAncestorOfTip, true);
    assert.deepEqual(
      Object.keys(cleanGitEnvironment())
        .filter((k) => k.startsWith("GIT_"))
        .sort(),
      ["GIT_CONFIG_GLOBAL", "GIT_CONFIG_NOSYSTEM", "GIT_TERMINAL_PROMPT"],
    );
  } finally {
    if (saved.dir === undefined) delete process.env.GIT_DIR;
    else process.env.GIT_DIR = saved.dir;
    if (saved.tree === undefined) delete process.env.GIT_WORK_TREE;
    else process.env.GIT_WORK_TREE = saved.tree;
  }
  try {
    // A replace ref that makes the main-only commit look like the branch's commit.
    git(r.root, "replace", r.onMain, r.onBranch);
    const outcome = await inspectCommit(r.root, {
      branch: r.branch,
      baseSha: r.base,
      sha: r.onMain,
    });
    assert.equal(
      outcome.isAncestorOfTip,
      false,
      "replace refs are switched off",
    );
  } finally {
    rmSync(r.root, { recursive: true, force: true });
    rmSync(other.root, { recursive: true, force: true });
  }
});
