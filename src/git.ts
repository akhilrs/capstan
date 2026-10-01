/**
 * Git facts the controller checks itself. Every call is an argument array
 * (no shell) with a clean environment and replace refs switched off, so the
 * daemon's own environment and replace refs cannot steer the check. A worker
 * shares the repository and can still change its own configuration, object
 * alternates and branch refs; that same-user hole is DEC-005's, not closed here.
 */
import { execFile } from "node:child_process";
import {
  MAX_CONFLICT_FILES,
  MAX_CONFLICT_PATH_CHARS,
} from "./controller/core.js";
import { createHash } from "node:crypto";

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

interface RunOptions {
  readonly encoding?: "utf8" | "latin1";
  readonly timeoutMs?: number;
  readonly maxBuffer?: number;
  readonly env?: NodeJS.ProcessEnv;
}

function runGit(
  repoRoot: string,
  args: readonly string[],
  options: RunOptions = {},
): Promise<GitOutcome> {
  return new Promise((resolve, reject) => {
    execFile(
      "git",
      ["--no-replace-objects", "-C", repoRoot, ...args],
      {
        env: { ...cleanGitEnvironment(), ...options.env },
        timeout: options.timeoutMs ?? GIT_TIMEOUT_MS,
        killSignal: "SIGKILL",
        maxBuffer: options.maxBuffer ?? 64 * 1024,
        encoding: options.encoding ?? "utf8",
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

/** Whether a commit with this id exists in the repository; false for a missing id or an object that is not a commit. */
export async function commitExists(
  repoRoot: string,
  sha: string,
): Promise<boolean> {
  if (!FULL_SHA.test(sha))
    throw new GitCheckError(
      "the commit id must be 40 lowercase hex characters",
    );
  const outcome = await runGit(repoRoot, [
    "rev-parse",
    "--verify",
    "--quiet",
    `${sha}^{commit}`,
  ]);
  if (outcome.code === 0) return true;
  if (outcome.code === 1) return false;
  throw new GitCheckError("git could not look the commit up");
}

const INTEGRATION_TIMEOUT_MS = 60_000;
const INTEGRATION_BUFFER = 1024 * 1024;
const CONTROLLER_IDENTITY: NodeJS.ProcessEnv = {
  GIT_AUTHOR_NAME: "capstan",
  GIT_AUTHOR_EMAIL: "capstan@localhost",
  GIT_COMMITTER_NAME: "capstan",
  GIT_COMMITTER_EMAIL: "capstan@localhost",
};
const WRITE_OPTIONS: RunOptions = {
  timeoutMs: INTEGRATION_TIMEOUT_MS,
  maxBuffer: INTEGRATION_BUFFER,
  env: CONTROLLER_IDENTITY,
};
const NO_COMMIT = "0".repeat(40);

/**
 * A path as text that is safe to show and to store: git's bytes were read one
 * character per byte, so every byte outside printable ASCII (a quote, a
 * backslash, a newline, any part of a UTF-8 name) becomes \xNN. Two different
 * paths stay different.
 */
export function printablePath(raw: string): string {
  const text = raw.replace(
    /[^\x20-\x7e]|["\\]/g,
    (c) => `\\x${c.charCodeAt(0).toString(16).padStart(2, "0")}`,
  );
  if (text.length <= MAX_CONFLICT_PATH_CHARS) return text;
  // Cut between escapes, and add a digest of the whole name so two long paths with one beginning stay different.
  const cut = text
    .slice(0, MAX_CONFLICT_PATH_CHARS)
    .replace(/\\x[0-9a-f]?$/, "");
  const digest = createHash("sha256").update(raw, "latin1").digest("hex");
  return `${cut}...#${digest.slice(0, 8)}`;
}

/** The commit HEAD points at in the repository, as a full sha1. */
export async function headCommit(repoRoot: string): Promise<string> {
  const format = await runGit(repoRoot, ["rev-parse", "--show-object-format"]);
  if (format.code !== 0 || format.stdout.trim() !== "sha1")
    throw new GitCheckError("only sha1 repositories are supported");
  const outcome = await runGit(repoRoot, [
    "rev-parse",
    "--verify",
    "--quiet",
    "HEAD^{commit}",
  ]);
  const sha = outcome.stdout.trim();
  if (outcome.code !== 0 || !FULL_SHA.test(sha))
    throw new GitCheckError("the project has no commit to integrate onto");
  return sha;
}

export type MergeResult =
  | { readonly kind: "merged"; readonly headSha: string }
  | {
      readonly kind: "conflicted";
      readonly reportId: string;
      readonly files: readonly string[];
    }
  | { readonly kind: "failed"; readonly reason: string };

export interface IntegrationMergeInput {
  readonly baseSha: string;
  readonly branch: string;
  readonly merges: readonly {
    readonly reportId: string;
    readonly sha: string;
    readonly message: string;
  }[];
}

/**
 * Merges the commits in order onto the base, each as its own merge commit, and
 * creates the branch at the result. It builds the trees with `git merge-tree`
 * and the commits with `git commit-tree`, so nothing is checked out: no
 * worktree exists, and no hook, filter, fsmonitor or rerere setting of the
 * repository can run. (A merge driver named in the committed attributes still
 * can; that is part of DEC-005's same-user hole.) A commit already contained in
 * the result so far is skipped, as `git merge` does. A conflict stops the run
 * and leaves nothing but unreferenced objects.
 */
export async function mergeIntoBranch(
  repoRoot: string,
  input: IntegrationMergeInput,
): Promise<MergeResult> {
  const ref = `refs/heads/${input.branch}`;
  for (const sha of [input.baseSha, ...input.merges.map((m) => m.sha)])
    if (!FULL_SHA.test(sha))
      return { kind: "failed", reason: "a commit id is not a full sha1" };
  if ((await runGit(repoRoot, ["check-ref-format", ref])).code !== 0)
    return { kind: "failed", reason: "the branch name is not a valid ref" };
  for (const merge of input.merges)
    if (!(await commitExists(repoRoot, merge.sha)))
      return {
        kind: "failed",
        reason: `the commit of report ${merge.reportId} does not exist`,
      };
  let head = input.baseSha;
  for (const merge of input.merges) {
    if (await isAncestor(repoRoot, merge.sha, head)) continue;
    const merged = await runGit(
      repoRoot,
      ["merge-tree", "--write-tree", "--name-only", "-z", head, merge.sha],
      { ...WRITE_OPTIONS, encoding: "latin1" },
    );
    const fields = merged.stdout.split("\0");
    if (merged.code === 1) {
      const end = fields.indexOf("", 1);
      const names = [
        ...new Set(fields.slice(1, end < 0 ? undefined : end)),
      ].filter((name) => name !== "");
      const files = names.slice(0, MAX_CONFLICT_FILES).map(printablePath);
      if (names.length > MAX_CONFLICT_FILES)
        files.push(`(and ${names.length - MAX_CONFLICT_FILES} more)`);
      if (files.length > 0)
        return { kind: "conflicted", reportId: merge.reportId, files };
    }
    if (merged.code !== 0 || !FULL_SHA.test(fields[0] ?? ""))
      return {
        kind: "failed",
        reason:
          merged.code === 129
            ? "git 2.38 or newer is needed to merge"
            : `git could not merge report ${merge.reportId} (exit ${merged.code})`,
      };
    const commit = await runGit(
      repoRoot,
      [
        "-c",
        "commit.gpgSign=false",
        "commit-tree",
        fields[0]!,
        "-p",
        head,
        "-p",
        merge.sha,
        "-m",
        merge.message,
      ],
      WRITE_OPTIONS,
    );
    const next = commit.stdout.trim();
    if (commit.code !== 0 || !FULL_SHA.test(next))
      return {
        kind: "failed",
        reason: `git could not commit the merge of report ${merge.reportId}`,
      };
    head = next;
  }
  if (head === input.baseSha)
    return {
      kind: "failed",
      reason: "every report is already contained in the base commit",
    };
  const created = await runGit(
    repoRoot,
    ["update-ref", ref, head, NO_COMMIT],
    WRITE_OPTIONS,
  );
  if (created.code !== 0)
    return {
      kind: "failed",
      reason: "git could not create the integration branch",
    };
  return { kind: "merged", headSha: head };
}

/** Whether any worktree of the repository has this branch checked out. */
async function branchCheckedOut(
  repoRoot: string,
  branch: string,
): Promise<boolean> {
  const outcome = await runGit(repoRoot, [
    "worktree",
    "list",
    "--porcelain",
    "-z",
  ]);
  if (outcome.code !== 0)
    throw new GitCheckError("git could not list the worktrees");
  return outcome.stdout.split("\0").includes(`branch refs/heads/${branch}`);
}

/** Deletes a branch only while it still points at `sha` and no worktree has it checked out. */
export async function deleteBranchAt(
  repoRoot: string,
  branch: string,
  sha: string,
): Promise<boolean> {
  const ref = `refs/heads/${branch}`;
  if ((await runGit(repoRoot, ["check-ref-format", ref])).code !== 0)
    return false;
  if (!FULL_SHA.test(sha)) return false;
  if (await branchCheckedOut(repoRoot, branch)) return false;
  const outcome = await runGit(
    repoRoot,
    ["update-ref", "-d", ref, sha],
    WRITE_OPTIONS,
  );
  return outcome.code === 0;
}

/** The commit a branch points at, or null when the branch does not exist. */
export async function branchTip(
  repoRoot: string,
  branch: string,
): Promise<string | null> {
  const ref = `refs/heads/${branch}`;
  if ((await runGit(repoRoot, ["check-ref-format", ref])).code !== 0)
    return null;
  const outcome = await runGit(repoRoot, [
    "rev-parse",
    "--verify",
    "--quiet",
    `${ref}^{commit}`,
  ]);
  if (outcome.code === 1) return null;
  const tip = outcome.stdout.trim();
  if (outcome.code !== 0 || !FULL_SHA.test(tip))
    throw new GitCheckError("git could not read the branch");
  return tip;
}

/** Whether the commit is already part of the project's HEAD. */
export async function isInHead(
  repoRoot: string,
  sha: string,
): Promise<boolean> {
  if (!FULL_SHA.test(sha))
    throw new GitCheckError(
      "the commit id must be 40 lowercase hex characters",
    );
  return isAncestor(repoRoot, sha, "HEAD");
}
