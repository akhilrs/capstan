import fs from "node:fs";
import type { ResolvedRole } from "../config/capstan-config.js";
import { InvalidArgumentError } from "./adapter.js";

/** One argument may not pass about 128 KiB on Linux; leave room for the other arguments. */
const MAX_ARGUMENT_BYTES = 120 * 1024;

export type HostRoleSettings = Pick<ResolvedRole, "model">;

/** A TOML basic string: backslash, quote and every control character escaped, so the value holds no newline. */
export function tomlString(value: string): string {
  if (!value.isWellFormed())
    throw new InvalidArgumentError("text for a Codex argument is not valid");
  let out = '"';
  for (const character of value) {
    const code = character.codePointAt(0)!;
    if (character === "\\") out += "\\\\";
    else if (character === '"') out += '\\"';
    else if (code < 0x20 || (code >= 0x7f && code <= 0x9f))
      out += `\\u${code.toString(16).padStart(4, "0")}`;
    else out += character;
  }
  return `${out}"`;
}

function requireNoDash(model: string | null): void {
  if (model !== null && model.startsWith("-"))
    throw new InvalidArgumentError("a model value must not start with a dash");
}

/**
 * Codex runs with full access and no approval prompt: its sandbox blocks the
 * daemon's Unix socket in every restricted mode, and a prompt would leave the
 * agent blocked. The worktree is pre-trusted with a run-time override, which
 * does not write the operator's own configuration file.
 */
export function codexArguments(
  role: HostRoleSettings,
  promptText: string,
  worktreePath: string | undefined,
): string[] {
  requireNoDash(role.model);
  const args = [
    "--sandbox",
    "danger-full-access",
    "--ask-for-approval",
    "never",
  ];
  if (role.model !== null) args.push("--model", role.model);
  args.push("-c", "check_for_update_on_startup=false");
  if (worktreePath !== undefined)
    args.push(
      "-c",
      `projects.${tomlString(fs.realpathSync(worktreePath))}.trust_level="trusted"`,
    );
  const instructions = `developer_instructions=${tomlString(promptText)}`;
  if (Buffer.byteLength(instructions, "utf8") > MAX_ARGUMENT_BYTES)
    throw new InvalidArgumentError(
      "the role prompt is too large to pass to Codex as one argument",
    );
  args.push("-c", instructions);
  return args;
}

/** OMP runs with every tool call approved: an approval prompt would leave the agent blocked and delivery deferred. */
export function ompArguments(
  role: HostRoleSettings,
  promptFile: string,
): string[] {
  requireNoDash(role.model);
  const args = ["--approval-mode", "yolo"];
  if (role.model !== null) args.push("--model", role.model);
  args.push("--append-system-prompt", promptFile);
  return args;
}
