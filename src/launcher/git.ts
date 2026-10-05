/** The default git runner and the checks that guard worktree removal. */
import fs from "node:fs";
import path from "node:path";
import { LauncherError, MAX_NOTE_LENGTH, type GitRunner } from "./shared.js";
import { oneLine } from "./text.js";
import { spawnSync } from "node:child_process";
import { checkGitRequirement } from "../git-requirement.js";

/** Whether git lists `worktreePath` as a worktree checked out on a legacy `capstan/` branch or on `recordedBranch`, the branch the ledger records for its agent. */
function isCapstanWorktree(
  git: (args: string[]) => ReturnType<typeof spawnSync>,
  worktreePath: string,
  recordedBranch?: string,
): boolean {
  const result = git(["worktree", "list", "--porcelain", "-z"]);
  if (result.status !== 0 || typeof result.stdout !== "string") return false;
  const real = (value: string): string => {
    try {
      return fs.realpathSync(value);
    } catch {
      return path.resolve(value);
    }
  };
  const wanted = real(worktreePath);
  let current: string | undefined;
  for (const line of result.stdout.split("\0")) {
    if (line.startsWith("worktree ")) current = line.slice("worktree ".length);
    if (
      current !== undefined &&
      (line.startsWith("branch refs/heads/capstan/") ||
        (recordedBranch !== undefined &&
          line === `branch refs/heads/${recordedBranch}`))
    )
      if (real(current) === wanted) return true;
  }
  return false;
}

export function defaultGit(projectRoot: string): GitRunner {
  const git = (args: string[]) =>
    spawnSync("git", args, {
      cwd: projectRoot,
      encoding: "utf8",
      timeout: 30_000,
    });
  return {
    headSha() {
      const requirement = checkGitRequirement(projectRoot);
      if (!requirement.ok)
        throw new LauncherError("git_requirement", requirement.message);
      const result = git(["rev-parse", "HEAD"]);
      const sha = typeof result.stdout === "string" ? result.stdout.trim() : "";
      if (/^[0-9a-f]{64}$/.test(sha))
        throw new LauncherError(
          "git_error",
          "this repository uses SHA-256 object names; only SHA-1 repositories are supported",
        );
      if (result.status !== 0 || !/^[0-9a-f]{40}$/.test(sha))
        throw new LauncherError(
          "git_error",
          "the project has no commit to branch from",
        );
      return sha;
    },
    worktreeRemove(worktreePath, recordedBranch) {
      const args = ["worktree", "remove"];
      if (isCapstanWorktree(git, worktreePath, recordedBranch))
        args.push("--force");
      const result = git([...args, worktreePath]);
      return {
        removed: result.status === 0,
        stderr: oneLine(
          typeof result.stderr === "string" ? result.stderr : "",
          MAX_NOTE_LENGTH,
        ),
      };
    },
    worktreeDirtyCount(worktreePath) {
      const result = spawnSync(
        "git",
        ["-C", worktreePath, "status", "--porcelain", "-z"],
        { encoding: "utf8", timeout: 30_000 },
      );
      if (result.status !== 0 || typeof result.stdout !== "string") return null;
      return result.stdout.split("\0").filter((entry) => entry !== "").length;
    },
    branchTip(branch) {
      const result = git([
        "rev-parse",
        "--verify",
        "--quiet",
        `refs/heads/${branch}^{commit}`,
      ]);
      const sha = typeof result.stdout === "string" ? result.stdout.trim() : "";
      return result.status === 0 && /^[0-9a-f]{40}$/.test(sha) ? sha : null;
    },
    reachableCommit(sha, from) {
      if (!/^[0-9a-f]{40}$/.test(sha)) return false;
      // `--end-of-options` came with git 2.24; an older git must not be mistaken for "no such commit".
      const version = /git version (\d+)\.(\d+)/.exec(
        String(git(["--version"]).stdout ?? ""),
      );
      if (
        version === null ||
        Number(version[1]) < 2 ||
        (Number(version[1]) === 2 && Number(version[2]) < 24)
      )
        throw new LauncherError(
          "old_git",
          "git 2.24 or newer is needed to check a commit",
        );
      const verify = git([
        "rev-parse",
        "--verify",
        "--end-of-options",
        `${sha}^{commit}`,
      ]);
      if (verify.status !== 0) return false;
      return from.some(
        (ref) => git(["merge-base", "--is-ancestor", sha, ref]).status === 0,
      );
    },
    branchNameValid: (branch) =>
      git(["check-ref-format", `refs/heads/${branch}`]).status === 0,
    renameBranch(from, to) {
      const result = git(["branch", "-m", from, to]);
      return {
        renamed: result.status === 0,
        stderr: oneLine(
          typeof result.stderr === "string" ? result.stderr : "",
          MAX_NOTE_LENGTH,
        ),
      };
    },
    saveRef: (ref, sha) => git(["update-ref", ref, sha]).status === 0,
    deleteBranchIf: (branch, sha) =>
      git(["update-ref", "-d", `refs/heads/${branch}`, sha]).status === 0,
    worktreeByBranch(branch) {
      const result = git(["worktree", "list", "--porcelain", "-z"]);
      // A failing git (or one too old for -z) must never read as "no worktree".
      if (result.status !== 0 || typeof result.stdout !== "string")
        throw new LauncherError(
          "git_error",
          "git could not list the worktrees",
        );
      let current: string | undefined;
      for (const line of result.stdout.split("\0")) {
        if (line.startsWith("worktree "))
          current = line.slice("worktree ".length);
        if (line === `branch refs/heads/${branch}`) return current;
      }
      return undefined;
    },
  };
}
