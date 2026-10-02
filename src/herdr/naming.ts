import path from "node:path";

/** Herdr's own rule for an agent name: it must be unique among the live agents of one server. */
export const HERDR_AGENT_NAME = /^[a-z][a-z0-9_-]{0,31}$/;
export const MAX_SLUG_CHARS = 10;
const MAX_LABEL_NAME_CHARS = 24;
const MAX_LABEL_CHARS = 64;
/** Roles may be named so that `<slug>-<role>-<up to 4 digits>` still fits Herdr's 32 characters. */
export const MAX_ROLE_NAME_CHARS = 32 - MAX_SLUG_CHARS - 1 - 1 - 4;

/** The project's display name: `[project] name` when set, else the base name of its directory. */
export function projectDisplayName(
  configured: string | null,
  projectRoot: string,
): string {
  const name = (configured ?? "").trim();
  const chosen = name !== "" ? name : path.basename(projectRoot);
  const clean = Array.from(chosen)
    .filter((character) => {
      const code = character.codePointAt(0)!;
      return code >= 0x20 && !(code >= 0x7f && code <= 0x9f);
    })
    .join("")
    .trim();
  const cut = Array.from(clean).slice(0, MAX_LABEL_NAME_CHARS).join("").trim();
  return cut === "" ? "capstan" : cut;
}

/** Lower case, runs of anything outside a-z0-9 become one `-`, a leading digit gets a `p`, at most ten characters. */
export function projectSlug(displayName: string): string {
  const base = displayName
    .normalize("NFKD")
    .replace(/\p{M}+/gu, "")
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "");
  const lettered = /^[a-z]/.test(base) ? base : `p${base}`;
  return lettered.slice(0, MAX_SLUG_CHARS).replace(/-+$/, "") || "p";
}

/** The name Herdr sees for a ledger agent id. */
export function herdrAgentName(slug: string, agentId: string): string {
  const name = `${slug}-${agentId}`;
  if (!HERDR_AGENT_NAME.test(name))
    throw new RangeError(
      `the agent name ${name} does not fit Herdr's rule (a lower-case letter first, then a-z, 0-9, _ or -, at most 32 characters)`,
    );
  return name;
}

/** `<project> · <part>`, cut so the whole label stays within the limit. */
export function workspaceLabel(displayName: string, part: string): string {
  const label = `${displayName} · ${part}`;
  return Array.from(label).slice(0, MAX_LABEL_CHARS).join("");
}
