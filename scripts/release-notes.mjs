// Release helpers: SemVer level from Conventional Commits and the CHANGELOG section.
// Plain ESM with no build step; the subject rules duplicate src/conventions.ts (a test keeps them in step).
import { execFileSync } from "node:child_process";

const COMMIT_TYPES = [
  "feat",
  "fix",
  "docs",
  "refactor",
  "perf",
  "test",
  "build",
  "ci",
  "chore",
  "style",
  "revert",
];
const SUBJECT_RE = /^([a-z]+)(?:\(([^()\s]*)\))?(!)?: (.*)$/;
const BREAKING_RE = /^BREAKING[ -]CHANGE: ?(.*)$/;
const TAG_RE = /^v(\d+)\.(\d+)\.(\d+)$/;
const VERSION_RE = /^(\d+)\.(\d+)\.(\d+)$/;

const SECTIONS = [
  ["feat", "Features"],
  ["fix", "Bug fixes"],
  ["perf", "Performance"],
  ["revert", "Reverts"],
];
const OTHER_TITLES = {
  docs: "Documentation",
  refactor: "Refactoring",
  test: "Tests",
  build: "Build",
  ci: "CI",
  chore: "Chores",
  style: "Style",
};

/** Parse a commit message; null when the subject is not a conforming Conventional Commit. */
export function parseCommit(message) {
  const lines = String(message).replace(/\r\n?/g, "\n").split("\n");
  const m = SUBJECT_RE.exec(lines[0] ?? "");
  if (!m) return null;
  const [, type, scope, bang, description] = m;
  if (!COMMIT_TYPES.includes(type)) return null;
  if (scope === "") return null;
  if (description.trim() === "" || /^\s/.test(description)) return null;
  let note = null;
  for (const line of lines.slice(1)) {
    const f = BREAKING_RE.exec(line);
    if (f) {
      note = f[1].trim();
      break;
    }
  }
  return {
    type,
    scope: scope ?? null,
    breaking: bang === "!" || note !== null,
    description,
    note: note || null,
  };
}

function normalize(commit) {
  const message = typeof commit === "string" ? commit : commit.message;
  const parsed = parseCommit(message);
  if (!parsed) return null;
  return {
    ...parsed,
    sha: typeof commit === "string" ? "" : (commit.sha ?? ""),
  };
}

function parseVersion(version) {
  const m = VERSION_RE.exec(version);
  if (!m) throw new Error(`not a X.Y.Z version: ${version}`);
  return [Number(m[1]), Number(m[2]), Number(m[3])];
}

export function compareVersions(a, b) {
  const x = parseVersion(a);
  const y = parseVersion(b);
  for (let i = 0; i < 3; i++) if (x[i] !== y[i]) return x[i] < y[i] ? -1 : 1;
  return 0;
}

/** Next version from commits (strings or { message }); null when nothing is releasable. */
export function nextVersion(current, commits) {
  const [major, minor, patch] = parseVersion(current);
  let level = null;
  for (const raw of commits) {
    const c = normalize(raw);
    if (!c) continue;
    if (c.breaking) level = "major";
    else if (c.type === "feat" && level !== "major") level = "minor";
    else if (["fix", "perf", "revert"].includes(c.type) && level === null)
      level = "patch";
  }
  if (level === null) return null;
  if (level === "major" && major === 0) level = "minor";
  const version =
    level === "major"
      ? `${major + 1}.0.0`
      : level === "minor"
        ? `${major}.${minor + 1}.0`
        : `${major}.${minor}.${patch + 1}`;
  return { version, level };
}

const MIN_CUT_OFF = 30;
const TYPE_RANK = [
  "feat",
  "fix",
  "perf",
  "revert",
  "refactor",
  "docs",
  "test",
  "build",
  "ci",
  "style",
  "chore",
];

function normalizeSubject(description) {
  return description
    .trim()
    .toLowerCase()
    .replace(/\s+/g, " ")
    .replace(/[\s.,;:!?]+$/, "");
}

/** Equal subjects, or one a prefix of the other where the shorter is long enough to be a cut-off title. */
function sameSubject(a, b) {
  const [short, long] = a.length <= b.length ? [a, b] : [b, a];
  return (
    short === long || (short.length >= MIN_CUT_OFF && long.startsWith(short))
  );
}

function mergeCommits(prior, c) {
  const better =
    TYPE_RANK.indexOf(c.type) < TYPE_RANK.indexOf(prior.type) ? c : prior;
  const scopes = [prior.scope, c.scope].filter(Boolean);
  const note = prior.note ?? c.note;
  return {
    ...better,
    scope: scopes.sort((x, y) => y.length - x.length)[0] ?? null,
    description:
      c.description.trim().length > prior.description.trim().length
        ? c.description
        : prior.description,
    breaking: prior.breaking || c.breaking,
    note,
  };
}

/**
 * Parsed commits with repeats removed. Scope is ignored when comparing; subjects match when their
 * normalized forms (trimmed, lower-case, no trailing punctuation) are equal or one is a prefix of the
 * other and at least 30 characters (a cut-off title). Types are compared across: the merged entry takes
 * the highest-ranking type, the longest subject and the more specific scope, at the first one's position.
 */
export function dedupeCommits(commits) {
  const kept = [];
  for (const raw of commits) {
    const c = normalize(raw);
    if (!c) continue;
    const subject = normalizeSubject(c.description);
    const at = kept.findIndex((k) => sameSubject(k.subject, subject));
    if (at === -1) kept.push({ subject, commit: c });
    else {
      const merged = mergeCommits(kept[at].commit, c);
      kept[at] = {
        subject: normalizeSubject(merged.description),
        commit: merged,
      };
    }
  }
  return kept.map((k) => k.commit);
}

/** The CHANGELOG section for a release; commits are strings or { sha, message }. */
export function renderChangelogSection(version, date, commits) {
  const parsed = dedupeCommits(commits);
  const item = (c, text) =>
    `- ${c.scope ? `**${c.scope}:** ` : ""}${text}${c.sha ? ` (${c.sha.slice(0, 7)})` : ""}`;
  const groups = [];
  const breaking = parsed.filter((c) => c.breaking);
  if (breaking.length > 0)
    groups.push([
      "Breaking changes",
      breaking.map((c) => item(c, c.note ?? c.description)),
    ]);
  const rest = parsed.filter((c) => !c.breaking);
  for (const [type, title] of SECTIONS) {
    const own = rest.filter((c) => c.type === type);
    if (own.length > 0)
      groups.push([title, own.map((c) => item(c, c.description))]);
  }
  for (const [type, title] of Object.entries(OTHER_TITLES)) {
    const own = rest.filter((c) => c.type === type);
    if (own.length > 0)
      groups.push([title, own.map((c) => item(c, c.description))]);
  }
  let out = `## ${version} (${date})\n`;
  for (const [title, lines] of groups)
    out += `\n### ${title}\n\n${lines.join("\n")}\n`;
  return out;
}

/** The highest vX.Y.Z tag reachable from HEAD, or null. */
export function lastReleaseTag(root) {
  let out;
  try {
    out = execFileSync("git", ["tag", "--merged", "HEAD", "--list", "v*"], {
      cwd: root,
      encoding: "utf8",
      stdio: ["ignore", "pipe", "ignore"],
    });
  } catch {
    return null; // no commits yet
  }
  const tags = out
    .split("\n")
    .filter((t) => TAG_RE.test(t))
    .sort((a, b) => compareVersions(a.slice(1), b.slice(1)));
  return tags.at(-1) ?? null;
}
