/**
 * Pure rules for the Operator's shell proposals: how a command is normalised and hashed, and which
 * commands may run without a human decision. Auto-approval is an allowlist of tiny read-only
 * commands over a tiny character set; everything else needs a human every time. The denylist is a
 * second layer for ordinary-looking commands, not the primary guard.
 */
import { createHash } from "node:crypto";

export const MAX_OPERATOR_COMMAND_BYTES = 8192;
export const MAX_OPERATOR_REASON_BYTES = 1024;

export type OperatorProposalKind = "command" | "restart";

export type TextRefusal = "empty" | "too_long" | "non_ascii";

export type NormalizedText =
  | { readonly ok: true; readonly text: string }
  | { readonly ok: false; readonly code: TextRefusal };

/** Printable ASCII (0x20-0x7E) plus newline and tab; NUL, C0/C1 controls, bidi and zero-width characters and homoglyphs all fall outside it. */
const ASCII_TEXT = /^[\x20-\x7e\n\t]*$/;

function checkText(text: string, maxBytes: number): NormalizedText {
  if (typeof text !== "string" || !ASCII_TEXT.test(text))
    return { ok: false, code: "non_ascii" };
  if (text.trim() === "") return { ok: false, code: "empty" };
  if (Buffer.byteLength(text, "utf8") > maxBytes)
    return { ok: false, code: "too_long" };
  return { ok: true, text };
}

/** The command exactly as sent, or the reason it is refused. It is never trimmed or rewritten: the hash covers these bytes. */
export function normalizeCommand(text: string): NormalizedText {
  return checkText(text, MAX_OPERATOR_COMMAND_BYTES);
}

export function normalizeReason(text: string): NormalizedText {
  return checkText(text, MAX_OPERATOR_REASON_BYTES);
}

export function commandHash(input: {
  readonly kind: OperatorProposalKind | string;
  readonly command: string;
  readonly forceRestart: boolean;
}): string {
  return createHash("sha256")
    .update(
      JSON.stringify([input.kind, input.command, input.forceRestart]),
      "utf8",
    )
    .digest("hex");
}

export const HASH_PREFIX_CHARS = 12;

export function hashPrefix(hash: string): string {
  return hash.slice(0, HASH_PREFIX_CHARS);
}

/** Words that always need a human decision, matched against the normalised token forms. */
export const OPERATOR_ALWAYS_APPROVAL: readonly string[] = [
  "push",
  "force",
  "force-with-lease",
  "f",
  "hard",
  "delete",
  "d",
  "D",
  "clean",
  "rm",
  "rmdir",
  "unlink",
  "shred",
  "truncate",
  "dd",
  "mkfs",
  "mv",
  "cp",
  "chmod",
  "chown",
  "chgrp",
  "ln",
  "kill",
  "killall",
  "pkill",
  "reboot",
  "shutdown",
  "halt",
  "poweroff",
  "sudo",
  "su",
  "doas",
  "reset",
  "restore",
  "checkout",
  "switch",
  "rebase",
  "merge",
  "cherry-pick",
  "revert",
  "commit",
  "stash",
  "tag",
  "am",
  "apply",
  "gc",
  "prune",
  "filter-branch",
  "update-ref",
  "worktree",
  "submodule",
  "remote",
  "config",
  "fetch",
  "pull",
  "clone",
  "curl",
  "wget",
  "ssh",
  "scp",
  "rsync",
  "nc",
  "ncat",
  "sh",
  "bash",
  "zsh",
  "dash",
  "fish",
  "eval",
  "exec",
  "source",
  "xargs",
  "env",
  "nohup",
  "find",
  "tee",
  "install",
  "patch",
  "tar",
  "unzip",
  "zip",
  "npm",
  "npx",
  "pnpm",
  "yarn",
  "node",
  "python",
  "python3",
  "perl",
  "ruby",
  "make",
  "cmake",
  "cargo",
  "go",
  "pip",
  "docker",
  "kubectl",
  "output",
  "ext-diff",
  "textconv",
  "script-shell",
  "upload-pack",
  "receive-pack",
  "open-files-in-pager",
];

/** Single-dash letters that write, execute or delete: -f -d -D -x -r -R -o -O -e -c. */
export const OPERATOR_DANGEROUS_SHORT_LETTERS: readonly string[] = [
  "f",
  "d",
  "D",
  "x",
  "r",
  "R",
  "o",
  "O",
  "e",
  "c",
];

export type AllowedPositionals = "none" | "paths" | readonly string[];

export type AllowlistEntry = {
  readonly command: string;
  readonly subcommand?: string;
  readonly options: readonly string[];
  /** "none": no argument; "paths": safe path-like arguments; a list: exactly those words. */
  readonly positionals: AllowedPositionals;
};

/** The only commands that may ever run without a human decision; all are read-only and run no project code. */
export const OPERATOR_AUTO_ALLOWLIST: readonly AllowlistEntry[] = [
  {
    command: "ls",
    options: ["-l", "-a", "-la", "-h"],
    positionals: "paths",
  },
  { command: "pwd", options: [], positionals: "none" },
  { command: "whoami", options: [], positionals: "none" },
  { command: "date", options: [], positionals: "none" },
  { command: "uname", options: ["-a"], positionals: "none" },
  { command: "df", options: ["-h"], positionals: "none" },
  {
    command: "cstan",
    subcommand: "ping",
    options: [],
    positionals: "none",
  },
  {
    command: "cstan",
    subcommand: "status",
    options: [],
    positionals: "none",
  },
  {
    command: "git",
    subcommand: "rev-parse",
    options: ["--abbrev-ref", "--short"],
    positionals: ["HEAD"],
  },
  { command: "git", subcommand: "ls-files", options: [], positionals: "none" },
];

/** One line of only these characters can be auto-approved; any other line may still be approved by a human. */
const SIMPLE_COMMAND = /^[A-Za-z0-9 _./:=@%+,-]+$/;
const SAFE_POSITIONAL = /^[A-Za-z0-9_./:@%+,][A-Za-z0-9_./:@%+,-]*$/;

export type CommandClassification = {
  readonly simple: boolean;
  /** The normalised token forms (raw, basename, split on `=`, long flags without dashes). */
  readonly tokens: readonly string[];
  /** The denylist words and dangerous short letters found; empty when none. */
  readonly alwaysApproval: readonly string[];
};

function words(text: string): string[] {
  return text.split(/\s+/).filter((token) => token !== "");
}

function basename(token: string): string {
  const trimmed = token.replace(/\/+$/, "");
  const slash = trimmed.lastIndexOf("/");
  return slash === -1 ? trimmed : trimmed.slice(slash + 1);
}

function wordForms(token: string): string[] {
  const forms = new Set<string>();
  const add = (form: string): void => {
    if (form === "") return;
    forms.add(form);
    forms.add(basename(form));
    if (form.startsWith("--")) {
      const bare = form.replace(/^-+/, "");
      forms.add(bare);
      forms.add(basename(bare));
    }
  };
  add(token);
  for (const part of token.split("=")) add(part);
  for (const form of [...forms])
    if (form.length > 1 && form !== form.toLowerCase())
      forms.add(form.toLowerCase());
  return [...forms];
}

/** The single letters of a combined short flag such as `-fd`; a long flag or a bare word yields none. */
function shortLetters(token: string): string[] {
  const flag = token.split("=")[0]!;
  return /^-[A-Za-z]+$/.test(flag) ? [...flag.slice(1)] : [];
}

export function tokenize(text: string): {
  readonly words: readonly string[];
  readonly letters: readonly string[];
} {
  const found = new Set<string>();
  const letters = new Set<string>();
  for (const token of words(text)) {
    for (const form of wordForms(token)) found.add(form);
    for (const letter of shortLetters(token)) letters.add(letter);
  }
  return { words: [...found], letters: [...letters] };
}

export function classifyCommand(text: string): CommandClassification {
  const simple = SIMPLE_COMMAND.test(text) && !text.includes("\n");
  const { words: forms, letters } = tokenize(text);
  const hits = forms.filter((form) => OPERATOR_ALWAYS_APPROVAL.includes(form));
  const letterHits = letters
    .filter((letter) => OPERATOR_DANGEROUS_SHORT_LETTERS.includes(letter))
    .map((letter) => `-${letter}`);
  return {
    simple,
    tokens: forms,
    alwaysApproval: [...hits, ...letterHits],
  };
}

function findEntry(tokens: readonly string[]): {
  readonly entry: AllowlistEntry;
  readonly rest: readonly string[];
} | null {
  const [command, second, ...others] = tokens;
  for (const entry of OPERATOR_AUTO_ALLOWLIST) {
    if (entry.command !== command) continue;
    if (entry.subcommand === undefined) return { entry, rest: tokens.slice(1) };
    if (entry.subcommand === second) return { entry, rest: others };
  }
  return null;
}

function isSafePositional(token: string): boolean {
  return SAFE_POSITIONAL.test(token) && !token.includes("=");
}

/** Why a token list is not a command the allowlist knows; null when it is exactly allowed. */
function allowlistProblem(tokens: readonly string[]): string | null {
  const match = findEntry(tokens);
  if (match === null)
    return "is not on the read-only allowlist (OPERATOR_AUTO_ALLOWLIST)";
  const { entry, rest } = match;
  for (const token of rest) {
    if (entry.options.includes(token)) continue;
    if (token.startsWith("-") || token.includes("="))
      return `uses option "${token}", which the allowlist does not accept for ${entry.command}`;
    if (Array.isArray(entry.positionals)) {
      if (entry.positionals.includes(token)) continue;
      return `has argument "${token}", which the allowlist does not accept for ${entry.command}`;
    }
    if (entry.positionals === "paths" && isSafePositional(token)) continue;
    return `has argument "${token}", which the allowlist does not accept for ${entry.command}`;
  }
  return null;
}

/** Null when `rule` may be a configured auto-approve rule; otherwise why not. Used at config load. */
export function autoApproveRuleProblem(rule: string): string | null {
  const normalized = normalizeCommand(rule);
  if (!normalized.ok) return `is not a usable command (${normalized.code})`;
  const classification = classifyCommand(rule);
  if (!classification.simple)
    return "must be one line of letters, digits, space and _ . / : = @ % + , - only";
  if (classification.alwaysApproval.length > 0)
    return `contains ${classification.alwaysApproval.map((word) => `"${word}"`).join(", ")}, which always needs a human decision`;
  if (rule !== rule.trim() || /  /.test(rule))
    return "must have single spaces and no leading or trailing space";
  const tokens = words(rule);
  if (tokens[0]!.includes("="))
    return "must not start with a variable assignment";
  return allowlistProblem(tokens);
}

export type AutoApproveVerdict = {
  readonly auto: boolean;
  readonly rule?: string;
  readonly reason: string;
};

/** Whether the command may run without a human. Exact rules match the whole command; prefix rules match whole leading tokens and accept only safe positional tokens after them. */
export function matchesAutoApprove(
  text: string,
  exactRules: readonly string[],
  prefixRules: readonly string[],
): AutoApproveVerdict {
  if (exactRules.length === 0 && prefixRules.length === 0)
    return { auto: false, reason: "no auto-approve rules are configured" };
  if (!normalizeCommand(text).ok)
    return { auto: false, reason: "the command is not accepted text" };
  const classification = classifyCommand(text);
  if (!classification.simple)
    return { auto: false, reason: "the command is not a simple command line" };
  if (classification.alwaysApproval.length > 0)
    return {
      auto: false,
      reason: `the command contains ${classification.alwaysApproval.join(", ")}, which always needs a human decision`,
    };
  const tokens = words(text);
  if (tokens[0]!.includes("="))
    return { auto: false, reason: "the command starts with an assignment" };
  for (const rule of exactRules) {
    if (text !== rule) continue;
    const problem = autoApproveRuleProblem(rule);
    if (problem === null) return { auto: true, rule, reason: "exact rule" };
  }
  for (const rule of prefixRules) {
    if (autoApproveRuleProblem(rule) !== null) continue;
    const head = words(rule);
    if (tokens.length < head.length) continue;
    if (!head.every((token, index) => tokens[index] === token)) continue;
    const rest = tokens.slice(head.length);
    if (!rest.every(isSafePositional)) continue;
    const entry = findEntry(head)?.entry;
    if (rest.length > 0 && entry?.positionals !== "paths") continue;
    if (allowlistProblem(tokens) === null)
      return { auto: true, rule, reason: "prefix rule" };
  }
  return { auto: false, reason: "no auto-approve rule matches" };
}

/** A restart is never auto-approved; a command only when a rule matches. */
export function autoDecision(
  kind: OperatorProposalKind,
  text: string,
  exactRules: readonly string[],
  prefixRules: readonly string[],
): AutoApproveVerdict {
  if (kind === "restart")
    return { auto: false, reason: "a restart always needs a human decision" };
  return matchesAutoApprove(text, exactRules, prefixRules);
}
