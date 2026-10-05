/** The git requirement: the project root is the top of a work tree whose HEAD is a commit. */
import fs from "node:fs";
import path from "node:path";
import { spawnSync } from "node:child_process";

export type GitProblem =
  | "git_missing"
  | "not_a_repository"
  | "bare_repository"
  | "subdirectory"
  | "no_commit";

export type GitCheck =
  | { readonly ok: true; readonly detached: boolean }
  | {
      readonly ok: false;
      readonly problem: GitProblem;
      readonly message: string;
    };

export const INITIAL_COMMIT_MESSAGE = "chore: initial commit";
const FIX_STEPS = `  git init\n  git add -A\n  git commit -m "${INITIAL_COMMIT_MESSAGE}"`;
const INIT_FLAG_HINT =
  "or let Capstan do it: cstan init --git (it lists the files it will commit first)";

function git(root: string, args: string[]) {
  return spawnSync("git", ["-C", root, ...args], {
    encoding: "utf8",
    timeout: 30_000,
  });
}

function real(value: string): string {
  try {
    return fs.realpathSync(value);
  } catch {
    return path.resolve(value);
  }
}

function problem(root: string, kind: GitProblem, what: string): GitCheck {
  const fix =
    kind === "git_missing"
      ? "install git, then run cstan again"
      : kind === "bare_repository"
        ? `run Capstan in a normal checkout (git clone <this repository> <folder>), not in the bare repository`
        : kind === "subdirectory"
          ? "run cstan from the repository's top-level folder (git rev-parse --show-toplevel prints it); a project root inside a larger repository is not supported"
          : `run in ${root}:\n${FIX_STEPS}`;
  const hint =
    kind === "not_a_repository" || kind === "no_commit"
      ? `\n${INIT_FLAG_HINT}`
      : "";
  return {
    ok: false,
    problem: kind,
    message: `Capstan needs a git repository with at least one commit (workers get their own git worktree and branch from HEAD), but ${what}.\nFix: ${fix}${hint}`,
  };
}

/** Checks `root` against the git requirement; never changes anything. */
export function checkGitRequirement(root: string): GitCheck {
  const inside = git(root, ["rev-parse", "--is-inside-work-tree"]);
  if (inside.error !== undefined && "code" in inside.error)
    return problem(root, "git_missing", "git could not be run");
  if (inside.status !== 0 || inside.stdout.trim() !== "true") {
    const bare = git(root, ["rev-parse", "--is-bare-repository"]);
    if (bare.status === 0 && bare.stdout.trim() === "true")
      return problem(root, "bare_repository", `${root} is a bare repository`);
    return problem(
      root,
      "not_a_repository",
      `${root} is not inside a git work tree`,
    );
  }
  const top = git(root, ["rev-parse", "--show-toplevel"]);
  if (top.status === 0 && real(top.stdout.trim()) !== real(root))
    return problem(
      root,
      "subdirectory",
      `${root} is a subdirectory of the repository at ${top.stdout.trim()}`,
    );
  const head = git(root, ["rev-parse", "--verify", "--quiet", "HEAD^{commit}"]);
  if (head.status !== 0)
    return problem(
      root,
      "no_commit",
      `the repository at ${root} has no commit yet (HEAD is an unborn branch)`,
    );
  const symbolic = git(root, ["symbolic-ref", "--quiet", "HEAD"]);
  return { ok: true, detached: symbolic.status !== 0 };
}

/** Adds `/.capstan/` to the repository's local exclude file so project state is never staged; true when the repository root is `root` and the file protects it. */
export function excludeCapstanState(root: string): boolean {
  const top = git(root, ["rev-parse", "--show-toplevel"]);
  if (
    top.status !== 0 ||
    path.resolve(top.stdout.replace(/\r?\n$/, "")) !== root
  )
    return false;
  const exclude = git(root, ["rev-parse", "--git-path", "info/exclude"]);
  if (exclude.status !== 0) return false;
  const excludePath = path.resolve(root, exclude.stdout.trim());
  const existing = fs.existsSync(excludePath)
    ? fs.readFileSync(excludePath, "utf8")
    : "";
  if (!existing.split(/\r?\n/).includes("/.capstan/")) {
    fs.mkdirSync(path.dirname(excludePath), { recursive: true });
    fs.appendFileSync(
      excludePath,
      `${existing && !existing.endsWith("\n") ? "\n" : ""}/.capstan/\n`,
      { mode: 0o600 },
    );
  }
  return true;
}

/** Whether `--git` may run in `root`: not a bare repository and not inside another repository. Returns the refusal message, or undefined. */
export function gitSetupRefusal(root: string): string | undefined {
  const check = checkGitRequirement(root);
  if (check.ok) return undefined;
  if (
    check.problem === "bare_repository" ||
    check.problem === "subdirectory" ||
    check.problem === "git_missing"
  )
    return check.message;
  return undefined;
}

/** Runs `git init` unless `root` is already a work tree. Returns whether it ran. */
export function gitInitIfNeeded(root: string): boolean {
  const inside = git(root, ["rev-parse", "--is-inside-work-tree"]);
  if (inside.status === 0 && inside.stdout.trim() === "true") return false;
  const result = git(root, ["init", "--quiet"]);
  if (result.status !== 0)
    throw new Error(`git init failed: ${result.stderr.trim()}`);
  return true;
}

/** The files `git add -A` would stage: tracked, modified and untracked files not ignored. */
export function filesToCommit(root: string): string[] {
  const result = git(root, [
    "ls-files",
    "-z",
    "--cached",
    "--others",
    "--exclude-standard",
  ]);
  if (result.status !== 0)
    throw new Error(`git ls-files failed: ${result.stderr.trim()}`);
  return result.stdout.split("\0").filter((file) => file !== "");
}

/** Stages everything not ignored and creates the initial commit. */
export function createInitialCommit(root: string): void {
  const add = git(root, ["add", "-A"]);
  if (add.status !== 0) throw new Error(`git add failed: ${add.stderr.trim()}`);
  const commit = git(root, ["commit", "--quiet", "-m", INITIAL_COMMIT_MESSAGE]);
  if (commit.status !== 0)
    throw new Error(
      `git commit failed: ${(commit.stderr || commit.stdout).trim()}\nIf git asks who you are, set it (git config user.name / user.email) and run: git commit -m "${INITIAL_COMMIT_MESSAGE}"`,
    );
}
