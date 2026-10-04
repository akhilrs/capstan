import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import {
  existsSync,
  mkdtempSync,
  readdirSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { test } from "node:test";
import { LauncherError, defaultGit } from "../src/launcher.js";
import { ctx } from "./harness.js";
import { SHA } from "./launcher-stubs.js";
import { eventNames, launched, reportAs, world } from "./launcher-harness.js";

test("a SHA-256 repository is named as unsupported instead of reported as having no commit", () => {
  const root = mkdtempSync(path.join(tmpdir(), "capstan-sha256-"));
  try {
    const init = spawnSync(
      "git",
      ["init", "-q", "--object-format=sha256", root],
      { encoding: "utf8" },
    );
    if (init.status !== 0) return; // this git cannot make one; nothing to check
    spawnSync(
      "git",
      [
        "-c",
        "user.name=t",
        "-c",
        "user.email=t@example.com",
        "commit",
        "-q",
        "--allow-empty",
        "-m",
        "x",
      ],
      { cwd: root },
    );
    assert.throws(
      () => defaultGit(root).headSha(),
      (e: unknown) =>
        e instanceof LauncherError &&
        e.code === "git_error" &&
        /SHA-256/.test(e.message),
    );
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("when git cannot say whether a worktree exists the branch and the row are kept, never deleted", async () => {
  const w = await world();
  try {
    await launched(w);
    const seat = w.core.createSeat(ctx(w.core, w.owner), {
      seatId: "developer-seat",
      name: "developer",
      role: "Developer",
    });
    const actor = w.core.createActor(ctx(w.core, w.owner), {
      displayName: "d",
      role: "Developer",
      seatId: seat.seatId,
    });
    w.core.registerAgent(ctx(w.core, w.owner), {
      agentId: "developer-1",
      roleName: "developer",
      seatId: seat.seatId,
      actorId: actor.actorId,
    });
    w.core.recordAgentPane(ctx(w.core, w.owner), {
      agentId: "developer-1",
      workspaceId: null,
      paneId: null,
      worktreePath: null,
      branch: "capstan/developer-1-g1",
      baseSha: SHA,
    });
    w.git.byBranchError = new LauncherError(
      "git_error",
      "git could not list the worktrees",
    );
    await w.launcher.adoptAll();
    assert.deepEqual(w.git.deleted, []);
    assert.deepEqual(w.git.removed, []);
    assert.ok(
      w.core.agentPanes(w.owner).some((r) => r.agentId === "developer-1"),
      "the row stays",
    );
    assert.ok(eventNames(w).includes("worktree_unknown"));
    w.git.byBranchError = undefined;
    w.git.byBranch.set("capstan/developer-1-g1", "/tmp/found");
    await w.launcher.adoptAll();
    assert.deepEqual(w.git.removed, ["/tmp/found"]);
    assert.deepEqual(w.git.deleted, [["capstan/developer-1-g1", SHA]]);
    assert.equal(
      w.core.agentPanes(w.owner).some((r) => r.agentId === "developer-1"),
      false,
    );
  } finally {
    w.cleanup();
  }
});

test("a failed start leaves no worktree and no branch in a real repository, and the log has no worktree_kept", async () => {
  const root = mkdtempSync(path.join(tmpdir(), "capstan-spawn-git-"));
  const run = (...args: string[]) =>
    spawnSync("git", args, { cwd: root, encoding: "utf8" });
  run("init", "-q");
  run(
    "-c",
    "user.name=t",
    "-c",
    "user.email=t@t",
    "commit",
    "-q",
    "--allow-empty",
    "-m",
    "i",
  );
  const w = await world(true, true, 3, {}, { git: defaultGit(root) });
  try {
    await launched(w);
    const create = w.adapter.createWorktree.bind(w.adapter);
    w.adapter.createWorktree = async (input) => {
      const made = await create(input);
      const checkout = path.join(root, "trees", input.branch.split("/")[1]!);
      const added = run("worktree", "add", "-q", "-b", input.branch, checkout);
      assert.equal(added.status, 0, added.stderr);
      writeFileSync(path.join(checkout, "node_modules.txt"), "dependencies");
      return { ...made, path: checkout };
    };
    w.adapter.startError = new Error("start failed");
    await assert.rejects(w.launcher.spawn("developer"), /step start/);
    const trees = run("worktree", "list", "--porcelain").stdout;
    assert.equal(trees.split("worktree ").length - 1, 1, trees);
    assert.equal(run("branch", "--list", "capstan/*").stdout.trim(), "");
    assert.ok(!w.events.some((e) => e.event === "worktree_kept"));
    const removing = w.events.find((e) => e.event === "worktree_removing")!;
    assert.equal(removing.details.dirty, 1);
  } finally {
    w.cleanup();
    rmSync(root, { recursive: true, force: true });
  }
});

test("release logs how many files a worktree holds before removing it, and git's message when it keeps one", async () => {
  const w = await world();
  try {
    await launched(w);
    const first = await w.launcher.spawn("developer");
    w.git.dirty = 3;
    await w.launcher.release(first.agentId);
    const removing = w.events.filter((e) => e.event === "worktree_removing");
    assert.deepEqual(removing.at(-1)!.details, {
      agentId: first.agentId,
      dirty: 3,
    });
    const second = await w.launcher.spawn("developer");
    w.git.dirty = 0;
    w.git.removeOk = false;
    w.git.removeStderr = "fatal: cannot remove a locked working tree";
    const kept = await w.launcher.release(second.agentId);
    assert.equal(kept.worktreeRemoved, false);
    assert.deepEqual(
      w.events.filter((e) => e.event === "worktree_removing").at(-1)!.details,
      { agentId: second.agentId, dirty: 0 },
    );
    assert.deepEqual(
      w.events.find((e) => e.event === "worktree_kept")!.details,
      {
        agentId: second.agentId,
        worktreePath: second.worktreePath,
        stderr: "fatal: cannot remove a locked working tree",
      },
    );
  } finally {
    w.cleanup();
  }
});

test("defaultGit removes a Capstan worktree that holds untracked files, and reports git's message for a locked or unknown one", () => {
  const root = mkdtempSync(path.join(tmpdir(), "capstan-remove-"));
  const run = (...args: string[]) =>
    spawnSync("git", args, { cwd: root, encoding: "utf8" });
  try {
    run("init", "-q");
    run(
      "-c",
      "user.name=t",
      "-c",
      "user.email=t@t",
      "commit",
      "-q",
      "--allow-empty",
      "-m",
      "i",
    );
    const git = defaultGit(root);
    const tree = (branch: string): string => {
      const dir = path.join(root, "trees", branch.replace("/", "-"));
      assert.equal(run("worktree", "add", "-q", "-b", branch, dir).status, 0);
      return dir;
    };

    const dirty = tree("capstan/dev-1-g1");
    writeFileSync(path.join(dirty, "untracked.txt"), "x");
    assert.equal(git.worktreeDirtyCount(dirty), 1);
    assert.equal(git.worktreeRemove(dirty).removed, true);
    assert.equal(existsSync(dirty), false);

    const locked = tree("capstan/dev-2-g1");
    run("worktree", "lock", locked);
    const refused = git.worktreeRemove(locked);
    assert.equal(refused.removed, false);
    assert.match(refused.stderr, /locked/);
    assert.equal(git.worktreeDirtyCount(locked), 0);

    const foreign = path.join(root, "trees", "foreign");
    assert.equal(
      run("worktree", "add", "-q", "-b", "feature/x", foreign).status,
      0,
    );
    writeFileSync(path.join(foreign, "untracked.txt"), "x");
    assert.equal(
      git.worktreeRemove(foreign).removed,
      false,
      "a worktree of another branch is never forced",
    );
    assert.match(git.worktreeRemove(foreign).stderr, /untracked/);

    const unknown = path.join(root, "trees", "nowhere");
    const missing = git.worktreeRemove(unknown);
    assert.equal(missing.removed, false);
    assert.notEqual(missing.stderr, "");
    assert.equal(git.worktreeDirtyCount(unknown), null);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("cleanup never deletes or force-removes a branch the ledger records for another active agent", async () => {
  const w = await world();
  try {
    await launched(w);
    const first = await w.launcher.spawn("developer");
    const second = await w.launcher.spawn("developer");
    w.core.recordAgentPane(ctx(w.core, w.owner), {
      agentId: second.agentId,
      workspaceId: "w3",
      paneId: second.paneId,
      worktreePath: second.worktreePath,
      branch: first.branch,
      baseSha: SHA,
    });
    const before = w.git.deleted.length;
    await w.launcher.release(first.agentId);
    assert.equal(w.git.deleted.length, before, "no branch deleted");
    assert.deepEqual(w.git.removedWithBranch.at(-1), undefined);
  } finally {
    w.cleanup();
  }
});

test("defaultGit forces the worktree of a branch the ledger records, a legacy capstan/ branch, and nothing else", () => {
  const root = mkdtempSync(path.join(tmpdir(), "capstan-recorded-"));
  const run = (...args: string[]) =>
    spawnSync("git", args, { cwd: root, encoding: "utf8" });
  try {
    run("init", "-q");
    run(
      "-c",
      "user.name=t",
      "-c",
      "user.email=t@t",
      "commit",
      "-q",
      "--allow-empty",
      "-m",
      "i",
    );
    const git = defaultGit(root);
    const tree = (branch: string, name: string): string => {
      const dir = path.join(root, "trees", name);
      assert.equal(run("worktree", "add", "-q", "-b", branch, dir).status, 0);
      writeFileSync(path.join(dir, "node_modules"), "x");
      return dir;
    };
    const recorded = tree("feat/NX-1-parser", "recorded");
    assert.equal(
      git.worktreeRemove(recorded, "feat/NX-1-parser").removed,
      true,
      "the branch recorded for the agent is forced",
    );
    assert.equal(existsSync(recorded), false);
    const unrecorded = tree("feat/NX-2-other", "unrecorded");
    assert.equal(
      git.worktreeRemove(unrecorded, "feat/NX-9-else").removed,
      false,
    );
    assert.equal(git.worktreeRemove(unrecorded).removed, false);
    const legacy = tree("capstan/dev-1-g1", "legacy");
    assert.equal(git.worktreeRemove(legacy).removed, true);
    const renamed = git.renameBranch("feat/NX-2-other", "feat/NX-2-renamed");
    assert.equal(renamed.renamed, true);
    assert.equal(
      git.worktreeRemove(unrecorded, "feat/NX-2-renamed").removed,
      true,
    );
    assert.equal(git.branchTip("feat/NX-2-other"), null);
    assert.equal(git.saveRef("refs/capstan/kept/x", git.headSha()), true);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("a legacy capstan/<agent>-g<n> agent keeps working: a report on it is accepted, replace continues its name and a branch with commits is kept on assign", async () => {
  const w = await world();
  try {
    await launched(w);
    const old = await w.launcher.spawn("developer");
    const legacy = "capstan/developer-1-g1";
    w.core.recordAgentPane(ctx(w.core, w.owner), {
      agentId: old.agentId,
      workspaceId: "w3",
      paneId: old.paneId,
      worktreePath: old.worktreePath,
      branch: legacy,
      baseSha: SHA,
    });
    const commit = "c".repeat(40);
    w.git.reachable.add(commit);
    w.git.tips.set(legacy, commit);
    reportAs(w, old.agentId, 1, commit, "legacy work");
    const kept = await w.launcher.renameBranchForTask(old.agentId, "req-3");
    assert.equal(kept.renamed, false);
    assert.match(kept.note!, /^branch kept: capstan\/developer-1-g1 /);
    const result = await w.launcher.replace(old.agentId);
    assert.equal(result.state, "started");
    if (result.state !== "started") return;
    assert.equal(
      result.branch,
      legacy,
      "the successor continues the legacy name",
    );
    assert.ok(w.adapter.calls.includes(`worktree:${legacy}:${commit}`));
  } finally {
    w.cleanup();
  }
});

test("no source file builds a branch from an agent id or a generation", () => {
  const files: string[] = [];
  const walk = (directory: string): void => {
    for (const entry of readdirSync(directory, { withFileTypes: true })) {
      const full = path.join(directory, entry.name);
      if (entry.isDirectory()) walk(full);
      else if (entry.name.endsWith(".ts")) files.push(full);
    }
  };
  walk(path.resolve(import.meta.dirname, "..", "..", "src"));
  assert.ok(files.length > 20);
  const offenders = files.filter((file) => {
    const text = readFileSync(file, "utf8");
    return (
      /`capstan\/\$\{/.test(text) ||
      /-g\$\{/.test(text) ||
      /["']-g["']\s*\+/.test(text)
    );
  });
  assert.deepEqual(offenders, []);
});
