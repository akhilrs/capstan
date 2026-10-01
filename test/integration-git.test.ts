import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { existsSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { test } from "node:test";
import {
  branchTip,
  deleteBranchAt,
  headCommit,
  isInHead,
  mergeIntoBranch,
  printablePath,
} from "../src/git.js";

const IDENTITY = {
  GIT_AUTHOR_NAME: "t",
  GIT_AUTHOR_EMAIL: "t@e.c",
  GIT_COMMITTER_NAME: "t",
  GIT_COMMITTER_EMAIL: "t@e.c",
};

function git(root: string, ...args: string[]): string {
  return execFileSync("git", ["-C", root, ...args], {
    encoding: "utf8",
    env: { ...process.env, ...IDENTITY },
  }).trim();
}

interface Repo {
  readonly root: string;
  readonly base: string;
  /** Edits shared.txt on one branch each; `a` and `b` conflict, `c` touches only its own file. */
  readonly a: string;
  readonly b: string;
  readonly c: string;
}

function makeRepo(): Repo {
  const root = mkdtempSync(path.join(tmpdir(), "capstan-igit-"));
  git(root, "init", "-q", "-b", "main");
  const commit = (file: string, text: string, message: string): string => {
    writeFileSync(path.join(root, file), text);
    git(root, "add", file);
    git(root, "commit", "-q", "-m", message);
    return git(root, "rev-parse", "HEAD");
  };
  const base = commit("shared.txt", "base\n", "base");
  const onBranch = (name: string, file: string, text: string): string => {
    git(root, "checkout", "-q", "-b", name, base);
    const sha = commit(file, text, name);
    git(root, "checkout", "-q", "main");
    return sha;
  };
  return {
    root,
    base,
    a: onBranch("a", "shared.txt", "from a\n"),
    b: onBranch("b", "shared.txt", "from b\n"),
    c: onBranch("c", "own.txt", "own\n"),
  };
}

function input(repo: Repo, branch: string, shas: [string, string][]) {
  return {
    baseSha: repo.base,
    branch,
    merges: shas.map(([reportId, sha]) => ({
      reportId,
      sha,
      message: `Merge report ${reportId}`,
    })),
  };
}

function worktrees(repo: Repo): string[] {
  return git(repo.root, "worktree", "list", "--porcelain")
    .split("\n")
    .filter((line) => line.startsWith("worktree "));
}

test("reports merge in order, each as a merge commit, onto a new branch, and nothing is checked out", async () => {
  const repo = makeRepo();
  try {
    const request = input(repo, "capstan/integration/ok", [
      ["r-a", repo.a],
      ["r-c", repo.c],
    ]);
    const result = await mergeIntoBranch(repo.root, request);
    assert.equal(result.kind, "merged");
    if (result.kind !== "merged") return;
    assert.equal(
      await branchTip(repo.root, "capstan/integration/ok"),
      result.headSha,
    );
    const subjects = git(
      repo.root,
      "log",
      "--first-parent",
      "--format=%s",
      `${repo.base}..${result.headSha}`,
    ).split("\n");
    assert.deepEqual(subjects, ["Merge report r-c", "Merge report r-a"]);
    assert.equal(git(repo.root, "show", `${result.headSha}:own.txt`), "own");
    assert.equal(
      git(repo.root, "show", `${result.headSha}:shared.txt`),
      "from a",
    );
    assert.equal(worktrees(repo).length, 1, "only the main worktree remains");
    assert.equal(await isInHead(repo.root, result.headSha), false);
    git(repo.root, "merge", "-q", "--ff-only", "capstan/integration/ok");
    assert.equal(await isInHead(repo.root, result.headSha), true);
    assert.equal(
      await deleteBranchAt(repo.root, "capstan/integration/ok", "f".repeat(40)),
      false,
      "a branch that moved is not deleted",
    );
    assert.equal(
      await deleteBranchAt(repo.root, "capstan/integration/ok", result.headSha),
      true,
    );
    assert.equal(await branchTip(repo.root, "capstan/integration/ok"), null);
  } finally {
    rmSync(repo.root, { recursive: true, force: true });
  }
});

test("a conflict aborts, names the report and the files, and leaves no worktree, branch or changed file", async () => {
  const repo = makeRepo();
  try {
    const request = input(repo, "capstan/integration/clash", [
      ["r-c", repo.c],
      ["r-a", repo.a],
      ["r-b", repo.b],
    ]);
    const result = await mergeIntoBranch(repo.root, request);
    assert.deepEqual(result, {
      kind: "conflicted",
      reportId: "r-b",
      files: ["shared.txt"],
    });
    assert.equal(worktrees(repo).length, 1);
    assert.equal(await branchTip(repo.root, "capstan/integration/clash"), null);
    assert.equal(git(repo.root, "status", "--porcelain"), "");
    assert.equal(git(repo.root, "rev-parse", "HEAD"), repo.base);
    assert.equal(await headCommit(repo.root), repo.base);
  } finally {
    rmSync(repo.root, { recursive: true, force: true });
  }
});

test("conflict paths are escaped, kept distinct and capped", async () => {
  const root = mkdtempSync(path.join(tmpdir(), "capstan-igit-"));
  try {
    git(root, "init", "-q", "-b", "main");
    const names = [
      "0-a\nb.txt",
      "0-caf\u00e9.txt",
      '0-q"uote.txt',
      "0-back\\slash.txt",
    ];
    for (let i = 0; i < 52; i += 1) names.push(`n${i}.txt`);
    const write = (text: string): string => {
      for (const name of names) writeFileSync(path.join(root, name), text);
      git(root, "add", "-A");
      git(root, "commit", "-q", "-m", text);
      return git(root, "rev-parse", "HEAD");
    };
    const base = write("base\n");
    git(root, "checkout", "-q", "-b", "x");
    const x = write("x\n");
    git(root, "checkout", "-q", "-b", "y", base);
    const y = write("y\n");
    git(root, "checkout", "-q", "main");
    const result = await mergeIntoBranch(root, {
      baseSha: x,
      branch: "capstan/integration/paths",
      merges: [{ reportId: "r", sha: y, message: "m" }],
    });
    assert.equal(result.kind, "conflicted");
    if (result.kind !== "conflicted") return;
    assert.equal(result.files.length, 51);
    assert.equal(result.files.at(-1), "(and 6 more)");
    assert.ok(result.files.every((f) => /^[\x20-\x7e]+$/.test(f)));
    assert.ok(result.files.includes("0-a\\x0ab.txt"));
    assert.ok(result.files.includes("0-caf\\xc3\\xa9.txt"));
    assert.ok(result.files.includes("0-q\\x22uote.txt"));
    assert.ok(result.files.includes("0-back\\x5cslash.txt"));
    assert.equal(printablePath("x".repeat(300)).length, 203);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("repository hooks, fsmonitor, signing and rerere settings do not run during an integration", async () => {
  const repo = makeRepo();
  try {
    const marker = path.join(repo.root, "hook-ran");
    for (const hook of ["post-checkout", "pre-merge-commit", "commit-msg"]) {
      const file = path.join(repo.root, ".git", "hooks", hook);
      writeFileSync(file, `#!/bin/sh\ntouch ${marker}\nexit 1\n`, {
        mode: 0o755,
      });
    }
    const program = path.join(
      repo.root,
      "..",
      `capstan-prog-${path.basename(repo.root)}`,
    );
    writeFileSync(program, `#!/bin/sh\ntouch ${marker}\nexit 1\n`, {
      mode: 0o755,
    });
    git(repo.root, "config", "core.fsmonitor", program);
    git(repo.root, "config", "commit.gpgSign", "true");
    git(repo.root, "config", "gpg.program", program);
    git(repo.root, "config", "rerere.enabled", "true");
    git(repo.root, "config", "rerere.autoUpdate", "true");
    const result = await mergeIntoBranch(
      repo.root,
      input(repo, "capstan/integration/hooks", [
        ["r-a", repo.a],
        ["r-c", repo.c],
      ]),
    );
    rmSync(program, { force: true });
    assert.equal(result.kind, "merged");
    assert.equal(existsSync(marker), false);
  } finally {
    rmSync(repo.root, { recursive: true, force: true });
  }
});

test("a branch that a worktree has checked out is never deleted", async () => {
  const repo = makeRepo();
  try {
    const result = await mergeIntoBranch(
      repo.root,
      input(repo, "capstan/integration/live", [["r-c", repo.c]]),
    );
    assert.equal(result.kind, "merged");
    if (result.kind !== "merged") return;
    git(repo.root, "checkout", "-q", "capstan/integration/live");
    assert.equal(
      await deleteBranchAt(
        repo.root,
        "capstan/integration/live",
        result.headSha,
      ),
      false,
    );
    assert.equal(
      await branchTip(repo.root, "capstan/integration/live"),
      result.headSha,
    );
    assert.equal(
      git(repo.root, "symbolic-ref", "HEAD"),
      "refs/heads/capstan/integration/live",
    );
  } finally {
    rmSync(repo.root, { recursive: true, force: true });
  }
});

test("a missing commit, a report already in the base and an existing branch fail cleanly", async () => {
  const repo = makeRepo();
  try {
    const missing = input(repo, "capstan/integration/missing", [
      ["r-x", "9".repeat(40)],
    ]);
    const result = await mergeIntoBranch(repo.root, missing);
    assert.equal(result.kind, "failed");
    assert.equal(worktrees(repo).length, 1);
    assert.equal(
      await branchTip(repo.root, "capstan/integration/missing"),
      null,
    );

    const inBase = await mergeIntoBranch(
      repo.root,
      input(repo, "capstan/integration/inbase", [["r-base", repo.base]]),
    );
    assert.deepEqual(inBase, {
      kind: "failed",
      reason: "every report is already contained in the base commit",
    });
    assert.equal(
      await branchTip(repo.root, "capstan/integration/inbase"),
      null,
    );

    git(repo.root, "branch", "capstan/integration/taken", repo.base);
    const taken = await mergeIntoBranch(
      repo.root,
      input(repo, "capstan/integration/taken", [["r-c", repo.c]]),
    );
    assert.equal(taken.kind, "failed");
    assert.equal(
      await branchTip(repo.root, "capstan/integration/taken"),
      repo.base,
      "a branch that already existed is left alone",
    );
    assert.equal(
      (
        await mergeIntoBranch(
          repo.root,
          input(repo, "bad..name", [["r-c", repo.c]]),
        )
      ).kind,
      "failed",
    );
  } finally {
    rmSync(repo.root, { recursive: true, force: true });
  }
});
