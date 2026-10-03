import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { existsSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { test } from "node:test";
import {
  branchTip,
  coveredReports,
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
    subject: "chore: combined",
    body: shas
      .map(([reportId]) => `Report ${reportId} (agent-x): done`)
      .join("\n"),
    merges: shas.map(([reportId, sha]) => ({ reportId, sha })),
  };
}

function worktrees(repo: Repo): string[] {
  return git(repo.root, "worktree", "list", "--porcelain")
    .split("\n")
    .filter((line) => line.startsWith("worktree "));
}

test("reports squash into one commit on a new branch, and nothing is checked out", async () => {
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
    assert.equal(
      git(repo.root, "rev-list", "--count", `${repo.base}..${result.headSha}`),
      "1",
    );
    assert.equal(git(repo.root, "rev-parse", `${result.headSha}^@`), repo.base);
    const message = git(repo.root, "log", "-1", "--format=%B", result.headSha);
    assert.match(
      message,
      /^chore: combined\n\nReport r-a \(agent-x\): done\nReport r-c/,
    );
    assert.doesNotMatch(message, /Co-Authored-By|Claude/);
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
      omitted: 0,
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
      "0-a,b.txt",
      "0-a\nb.txt",
      "0-caf\u00e9.txt",
      '0-q"uote.txt',
      "0-back\\slash.txt",
    ];
    for (let i = 0; i < 53; i += 1) names.push(`n${i}.txt`);
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
      subject: "chore: x",
      body: "Report r (a): y",
      merges: [{ reportId: "r", sha: y }],
    });
    assert.equal(result.kind, "conflicted");
    if (result.kind !== "conflicted") return;
    assert.equal(result.files.length, 50);
    assert.equal(result.omitted, 8);
    assert.ok(result.files.every((f) => /^[\x20-\x7e]+$/.test(f)));
    assert.ok(result.files.includes("0-a\\x0ab.txt"));
    assert.ok(result.files.includes("0-a\\x2cb.txt"));
    assert.equal(printablePath("a...#0123456789ab"), "a...\\x230123456789ab");
    assert.ok(result.files.includes("0-caf\\xc3\\xa9.txt"));
    assert.ok(result.files.includes("0-q\\x22uote.txt"));
    assert.ok(result.files.includes("0-back\\x5cslash.txt"));
    const long = printablePath("x".repeat(300));
    assert.equal(long.length, 216);
    assert.match(long, /^x{200}\.\.\.#[0-9a-f]{12}$/);
    assert.notEqual(long, printablePath(`${"x".repeat(299)}y`));
    const split = printablePath("\u00e9".repeat(100));
    assert.ok(!/\\x[0-9a-f]?\.\.\./.test(split), split);
    assert.match(split, /^(\\x[0-9a-f]{2})+\.\.\.#[0-9a-f]{12}$/);
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

function referenceTree(repo: Repo, shas: string[]): string {
  git(
    repo.root,
    "worktree",
    "add",
    "-q",
    "--detach",
    `${repo.root}-ref`,
    repo.base,
  );
  try {
    for (const sha of shas)
      git(`${repo.root}-ref`, "merge", "-q", "--no-ff", "-m", "m", sha);
    return git(`${repo.root}-ref`, "rev-parse", "HEAD^{tree}");
  } finally {
    git(repo.root, "worktree", "remove", "--force", `${repo.root}-ref`);
  }
}

test("the squash tree equals the tree of merging each report with --no-ff, including a contained report", async () => {
  const repo = makeRepo();
  try {
    git(repo.root, "checkout", "-q", "-b", "d", repo.c);
    writeFileSync(path.join(repo.root, "more.txt"), "more\n");
    git(repo.root, "add", "more.txt");
    git(repo.root, "commit", "-q", "-m", "d");
    const d = git(repo.root, "rev-parse", "HEAD");
    git(repo.root, "checkout", "-q", "main");
    const cases: [string, string[]][] = [
      ["one", [repo.a]],
      ["disjoint", [repo.a, repo.c]],
      ["contained", [repo.c, d]],
      ["contained-first", [d, repo.c]],
    ];
    for (const [name, shas] of cases) {
      const result = await mergeIntoBranch(
        repo.root,
        input(
          repo,
          `capstan/integration/${name}`,
          shas.map((s, i) => [`r${i}`, s]),
        ),
      );
      assert.equal(result.kind, "merged", name);
      if (result.kind !== "merged") return;
      assert.equal(
        git(
          repo.root,
          "rev-list",
          "--count",
          `${repo.base}..${result.headSha}`,
        ),
        "1",
        name,
      );
      assert.equal(
        git(repo.root, "rev-parse", `${result.headSha}^{tree}`),
        referenceTree(repo, shas),
        name,
      );
    }
  } finally {
    rmSync(repo.root, { recursive: true, force: true });
  }
});

test("overlapping files in different hunks squash to the reference tree", async () => {
  const repo = makeRepo();
  try {
    const lines = Array.from({ length: 30 }, (_, i) => `line ${i}`);
    const write = (edit: (l: string[]) => void): void => {
      const copy = [...lines];
      edit(copy);
      writeFileSync(path.join(repo.root, "big.txt"), `${copy.join("\n")}\n`);
    };
    write(() => undefined);
    git(repo.root, "add", "big.txt");
    git(repo.root, "commit", "-q", "-m", "big");
    const base = git(repo.root, "rev-parse", "HEAD");
    const branchWith = (name: string, line: number): string => {
      git(repo.root, "checkout", "-q", "-b", name, base);
      write((l) => {
        l[line] = name;
      });
      git(repo.root, "commit", "-q", "-am", name);
      const sha = git(repo.root, "rev-parse", "HEAD");
      git(repo.root, "checkout", "-q", "main");
      return sha;
    };
    const top = branchWith("top", 1);
    const bottom = branchWith("bottom", 27);
    const moved = { ...repo, base };
    const result = await mergeIntoBranch(
      repo.root,
      input(moved, "capstan/integration/hunks", [
        ["r1", top],
        ["r2", bottom],
      ]),
    );
    assert.equal(result.kind, "merged");
    if (result.kind !== "merged") return;
    assert.equal(
      git(repo.root, "rev-parse", `${result.headSha}^{tree}`),
      referenceTree(moved, [top, bottom]),
    );
  } finally {
    rmSync(repo.root, { recursive: true, force: true });
  }
});

async function headOf(
  repo: Repo,
  reports: [string, string][],
): Promise<string> {
  const result = await mergeIntoBranch(
    repo.root,
    input(repo, "capstan/integration/cover", reports),
  );
  assert.equal(result.kind, "merged");
  return (result as { headSha: string }).headSha;
}

function commitOnBranch(
  repo: Repo,
  name: string,
  from: string,
  file: string,
  text: string,
): string {
  git(repo.root, "checkout", "-q", "-b", name, from);
  writeFileSync(path.join(repo.root, file), text);
  git(repo.root, "add", file);
  git(repo.root, "commit", "-q", "-m", name);
  const sha = git(repo.root, "rev-parse", "HEAD");
  git(repo.root, "checkout", "-q", "main");
  return sha;
}

test("a report that is an ancestor of an integrated commit is covered as ancestor", async () => {
  const repo = makeRepo();
  try {
    const aThenB = commitOnBranch(repo, "ab", repo.c, "more.txt", "more\n");
    const head = await headOf(repo, [["r-ab", aThenB]]);
    const covered = await coveredReports(
      repo.root,
      head,
      [{ reportId: "r-c", commitSha: repo.c }],
      { memberCommits: [aThenB] },
    );
    assert.deepEqual(covered, [{ reportId: "r-c", how: "ancestor" }]);
  } finally {
    rmSync(repo.root, { recursive: true, force: true });
  }
});

test("an amended report whose changes are all in the head is covered by tree; an overwritten one is not", async () => {
  const repo = makeRepo();
  try {
    git(repo.root, "checkout", "-q", "-b", "amended", repo.c);
    git(repo.root, "commit", "-q", "--amend", "-m", "amended c");
    const amended = git(repo.root, "rev-parse", "HEAD");
    git(repo.root, "checkout", "-q", "main");
    assert.notEqual(amended, repo.c);
    const overwriter = commitOnBranch(
      repo,
      "overwrite",
      repo.base,
      "own.txt",
      "someone else\n",
    );
    const head = await headOf(repo, [["r-amended", amended]]);
    assert.deepEqual(
      await coveredReports(repo.root, head, [
        { reportId: "r-c", commitSha: repo.c },
      ]),
      [{ reportId: "r-c", how: "tree" }],
    );
    const otherHead = (await mergeIntoBranch(
      repo.root,
      input(repo, "capstan/integration/overwrite", [["r-o", overwriter]]),
    )) as { headSha: string };
    assert.deepEqual(
      await coveredReports(repo.root, otherHead.headSha, [
        { reportId: "r-c", commitSha: repo.c },
        { reportId: "r-a", commitSha: repo.a },
      ]),
      [],
    );
  } finally {
    rmSync(repo.root, { recursive: true, force: true });
  }
});

test("a report whose commit object is missing is skipped with a reason and does not throw", async () => {
  const repo = makeRepo();
  try {
    const head = await headOf(repo, [["r-a", repo.a]]);
    const skipped: [string, string][] = [];
    const covered = await coveredReports(
      repo.root,
      head,
      [
        { reportId: "r-gone", commitSha: "e".repeat(40) },
        { reportId: "r-a2", commitSha: repo.a },
      ],
      { onSkipped: (id, reason) => skipped.push([id, reason]) },
    );
    assert.deepEqual(covered, [{ reportId: "r-a2", how: "tree" }]);
    assert.deepEqual(skipped, [["r-gone", "its commit does not exist"]]);
  } finally {
    rmSync(repo.root, { recursive: true, force: true });
  }
});
