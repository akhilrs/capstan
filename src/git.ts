/**
 * Git facts the controller checks itself. Every call is an argument array
 * (no shell) with a clean environment and replace refs switched off, so the
 * daemon's own environment and replace refs cannot steer the check. A worker
 * shares the repository and can still change its own configuration, object
 * alternates and branch refs; that same-user hole is DEC-005's, not closed here.
 */
import { execFile } from "node:child_process";

const GIT_TIMEOUT_MS = 10_000;
const FULL_SHA = /^[0-9a-f]{40}$/;

export class GitCheckError extends Error {
  override readonly name = "GitCheckError";
}

export interface CommitInspection {
  readonly commitExists: boolean;
  readonly branchTip: string | null;
  readonly isAncestorOfTip: boolean;
  readonly isAncestorOfBase: boolean;
}

/** Only what git needs; every inherited GIT_* variable, the user's and the system's git configuration are left out. */
export function cleanGitEnvironment(): NodeJS.ProcessEnv {
  return {
    PATH: process.env.PATH ?? "/usr/bin:/bin",
    LC_ALL: "C",
    GIT_CONFIG_NOSYSTEM: "1",
    GIT_CONFIG_GLOBAL: "/dev/null",
    GIT_TERMINAL_PROMPT: "0",
  };
}

interface GitOutcome {
  readonly code: number;
  readonly stdout: string;
}

function runGit(
  repoRoot: string,
  args: readonly string[],
): Promise<GitOutcome> {
  return new Promise((resolve, reject) => {
    execFile(
      "git",
      ["--no-replace-objects", "-C", repoRoot, ...args],
      {
        env: cleanGitEnvironment(),
        timeout: GIT_TIMEOUT_MS,
        killSignal: "SIGKILL",
        maxBuffer: 64 * 1024,
        encoding: "utf8",
      },
      (error, stdout) => {
        if (error === null) return resolve({ code: 0, stdout });
        const code = (error as NodeJS.ErrnoException & { code?: unknown }).code;
        if (typeof code === "number") return resolve({ code, stdout });
        reject(
          new GitCheckError(
            `git could not be run (${error.killed ? "timed out" : String(code ?? error.message)})`,
          ),
        );
      },
    );
  });
}

/** Exit 0 true, exit 1 false, anything else is a failure of git itself and an error. */
async function isAncestor(
  repoRoot: string,
  ancestor: string,
  descendant: string,
): Promise<boolean> {
  const outcome = await runGit(repoRoot, [
    "merge-base",
    "--is-ancestor",
    ancestor,
    descendant,
  ]);
  if (outcome.code === 0) return true;
  if (outcome.code === 1) return false;
  throw new GitCheckError("git could not compare the commits");
}

export async function inspectCommit(
  repoRoot: string,
  input: {
    readonly branch: string;
    readonly baseSha: string | null;
    readonly sha: string;
  },
): Promise<CommitInspection> {
  if (!FULL_SHA.test(input.sha))
    throw new GitCheckError(
      "the commit id must be 40 lowercase hex characters",
    );
  if (input.baseSha !== null && !FULL_SHA.test(input.baseSha))
    throw new GitCheckError("the base commit id is not a full id");
  const format = await runGit(repoRoot, ["rev-parse", "--show-object-format"]);
  if (format.code !== 0)
    throw new GitCheckError(
      "git could not report the repository's object format (git 2.29 or newer is needed)",
    );
  if (format.stdout.trim() !== "sha1")
    throw new GitCheckError("only sha1 repositories are supported");
  const ref = `refs/heads/${input.branch}`;
  const valid = await runGit(repoRoot, ["check-ref-format", ref]);
  if (valid.code !== 0)
    throw new GitCheckError("the recorded branch name is not a valid ref");
  const tipOutcome = await runGit(repoRoot, [
    "rev-parse",
    "--verify",
    "--quiet",
    `${ref}^{commit}`,
  ]);
  if (tipOutcome.code !== 0 && tipOutcome.code !== 1)
    throw new GitCheckError("git could not read the branch");
  const tip = tipOutcome.code === 0 ? tipOutcome.stdout.trim() : null;
  if (tip !== null && !FULL_SHA.test(tip))
    throw new GitCheckError("git gave an unexpected branch tip");
  const exists = await runGit(repoRoot, [
    "rev-parse",
    "--verify",
    "--quiet",
    `${input.sha}^{commit}`,
  ]);
  if (exists.code !== 0 && exists.code !== 1)
    throw new GitCheckError("git could not look the commit up");
  const commitExists = exists.code === 0;
  return {
    commitExists,
    branchTip: tip,
    isAncestorOfTip:
      commitExists && tip !== null
        ? await isAncestor(repoRoot, input.sha, tip)
        : false,
    isAncestorOfBase:
      commitExists && input.baseSha !== null
        ? await isAncestor(repoRoot, input.sha, input.baseSha)
        : false,
  };
}
