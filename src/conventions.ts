// Naming conventions: commit subjects, attribution lines and branch names.

export const BRANCH_TYPES = [
  "feat",
  "fix",
  "docs",
  "refactor",
  "perf",
  "test",
  "build",
  "ci",
  "chore",
] as const;
export const COMMIT_TYPES = [...BRANCH_TYPES, "style", "revert"] as const;

const MAX_BRANCH = 100;

export type CommitRule =
  | "subject-format"
  | "body-separation"
  | "claude-co-author"
  | "claude-session"
  | "claude-code-footer";

export type ParsedSubject =
  | {
      ok: true;
      type: string;
      scope: string | null;
      breaking: boolean;
      description: string;
    }
  | { ok: false; rule: "subject-format"; reason: string };

const SUBJECT_RE = /^([a-z]+)(?:\(([^()\s]*)\))?(!)?: (.*)$/;

export function parseCommitSubject(subject: string): ParsedSubject {
  const fail = (reason: string): ParsedSubject => ({
    ok: false,
    rule: "subject-format",
    reason,
  });
  const m = SUBJECT_RE.exec(subject);
  if (!m)
    return fail(
      "subject must look like 'type(scope)!: description' with a space after the colon",
    );
  const [, type, scope, bang, description] = m as unknown as [
    string,
    string,
    string | undefined,
    string | undefined,
    string,
  ];
  if (!(COMMIT_TYPES as readonly string[]).includes(type)) {
    return fail(
      `unknown type '${type}'; use one of ${COMMIT_TYPES.join(", ")}`,
    );
  }
  if (scope !== undefined && scope === "")
    return fail("scope must not be empty");
  if (description.trim() === "" || /^\s/.test(description))
    return fail(
      "description must start right after one space and not be empty",
    );
  return {
    ok: true,
    type,
    scope: scope ?? null,
    breaking: bang === "!",
    description,
  };
}

export function checkCommitMessage(
  message: string,
  parents: number,
): { rule: CommitRule; reason: string }[] {
  const out: { rule: CommitRule; reason: string }[] = [];
  const lines = message.replace(/\r\n?/g, "\n").split("\n");
  if (parents < 2) {
    const parsed = parseCommitSubject(lines[0] ?? "");
    if (!parsed.ok) out.push({ rule: parsed.rule, reason: parsed.reason });
    if (lines.length > 1 && (lines[1] ?? "").trim() !== "") {
      out.push({
        rule: "body-separation",
        reason: "leave a blank line between the subject and the body",
      });
    }
  }
  for (const line of lines) {
    if (
      /^\s*co-authored-by:.*(\bclaude\b|noreply@anthropic\.com)/i.test(line)
    ) {
      out.push({
        rule: "claude-co-author",
        reason: "remove the Claude Co-Authored-By line",
      });
      break;
    }
  }
  if (lines.some((l) => /^\s*claude-session:/i.test(l))) {
    out.push({
      rule: "claude-session",
      reason: "remove the Claude-Session line",
    });
  }
  if (
    lines.some(
      (l) =>
        /generated with \[?claude code\]?/i.test(l) || l.includes("\u{1F916}"),
    )
  ) {
    out.push({
      rule: "claude-code-footer",
      reason: "remove the 'Generated with Claude Code' footer",
    });
  }
  return out;
}

const FOLD: Record<string, string> = {
  ß: "ss",
  ø: "o",
  æ: "ae",
  œ: "oe",
  đ: "d",
  ł: "l",
  þ: "th",
  ð: "d",
};

export function slugify(title: string, max = 40): string {
  const folded = title
    .normalize("NFKD")
    .replace(/\p{M}/gu, "")
    .toLowerCase()
    .replace(/[ßøæœđłþð]/g, (c) => FOLD[c] ?? c);
  const slug = folded.replace(/[^a-z0-9]+/g, "-").replace(/^-+|-+$/g, "");
  const cut = slug.slice(0, Math.max(1, max)).replace(/-+$/, "");
  return cut === "" ? "work" : cut;
}

export function formatSubject(
  input: {
    type: string;
    scope?: string;
    breaking?: boolean;
    description: string;
  },
  max = 72,
): string {
  const type = (COMMIT_TYPES as readonly string[]).includes(input.type)
    ? input.type
    : "chore";
  const scope = input.scope
    ? input.scope
        .toLowerCase()
        .replace(/[^a-z0-9._/-]+/g, "-")
        .replace(/^-+|-+$/g, "")
    : "";
  const bang = input.breaking ? "!" : "";
  let prefix = `${type}${scope ? `(${scope})` : ""}${bang}: `;
  if (prefix.length + 8 > max && scope) prefix = `${type}${bang}: `;
  let description = input.description.replace(/\s+/g, " ").trim();
  if (description === "") description = "update";
  const room = Math.max(1, max - prefix.length);
  if (description.length > room) {
    const head = description.slice(0, room + 1);
    const space = head.lastIndexOf(" ");
    description = (
      space > 0 ? head.slice(0, space) : description.slice(0, room)
    ).replace(/[\s.,;:-]+$/, "");
    if (description === "")
      description = input.description.trim().slice(0, room);
  }
  return prefix + description;
}

function cleanId(text: string, max: number): string {
  const id = text
    .replace(/[^A-Za-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "")
    .slice(0, max)
    .replace(/-+$/, "");
  return id === "" ? "x" : id;
}

function branchType(type: string): string {
  return (BRANCH_TYPES as readonly string[]).includes(type) ? type : "chore";
}

function clip(name: string): string {
  return name.length <= MAX_BRANCH
    ? name
    : name.slice(0, MAX_BRANCH).replace(/[-./]+$/, "");
}

export function workerBranchName(input: {
  type: string;
  taskId: string;
  slug: string;
}): string {
  return clip(
    `${branchType(input.type)}/${cleanId(input.taskId, 40)}-${slugify(input.slug)}`,
  );
}

export function adHocBranchName(agentId: string, slug: string): string {
  return clip(`chore/${cleanId(agentId, 40)}-${slugify(slug)}`);
}

export function reviewBranchName(agentId: string, targetId: string): string {
  return clip(`chore/review-${cleanId(agentId, 40)}-${cleanId(targetId, 40)}`);
}

export function integrationBranchName(input: {
  integrationId: string;
  planId?: string;
  planTitle?: string;
  attempt?: number;
}): string {
  const parts = [cleanId(input.integrationId, 40)];
  if (input.planId) parts.push(cleanId(input.planId, 30));
  if (input.planTitle) parts.push(slugify(input.planTitle, 30));
  if (input.attempt !== undefined && input.attempt >= 2)
    parts.push(`a${Math.floor(input.attempt)}`);
  return clip(`integration/${parts.join("-")}`);
}

export function withSuffix(branch: string, n: number): string {
  if (!(n >= 2)) return branch;
  const suffix = `-${Math.floor(n)}`;
  if (branch.length + suffix.length <= MAX_BRANCH) return branch + suffix;
  return (
    branch.slice(0, MAX_BRANCH - suffix.length).replace(/[-./]+$/, "") + suffix
  );
}

export type ParsedBranch =
  | { kind: "conventional"; type: string; rest: string }
  | { kind: "integration"; rest: string }
  | { kind: "legacy-worker"; agentId: string; generation: number }
  | { kind: "legacy-integration"; integrationId: string }
  | { kind: "other" };

export function parseBranch(name: string): ParsedBranch {
  let m = /^capstan\/integration\/(.+)$/.exec(name);
  if (m) return { kind: "legacy-integration", integrationId: m[1]! };
  m = /^capstan\/(.+)-g(\d+)$/.exec(name);
  if (m)
    return { kind: "legacy-worker", agentId: m[1]!, generation: Number(m[2]) };
  m = /^integration\/(.+)$/.exec(name);
  if (m) return { kind: "integration", rest: m[1]! };
  m = /^([a-z]+)\/(.+)$/.exec(name);
  if (m && (BRANCH_TYPES as readonly string[]).includes(m[1]!)) {
    return { kind: "conventional", type: m[1]!, rest: m[2]! };
  }
  return { kind: "other" };
}

export function isTaskId(text: string): boolean {
  return /^[A-Za-z][A-Za-z0-9]*-\d+(\/[A-Za-z0-9][A-Za-z0-9._-]*)?$/.test(text);
}
