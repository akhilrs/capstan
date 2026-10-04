/** The environment and command-line arguments an agent gets. */
import type { ResolvedRole } from "../config/capstan-config.js";
import { InvalidArgumentError } from "./adapter-errors.js";
import {
  CONTROL_CHARACTERS,
  ENVIRONMENT_KEY,
  UNSAFE_TEXT,
} from "./adapter-validate.js";

const ALLOWLISTED_BASE = [
  "PATH",
  "HOME",
  "USER",
  "LOGNAME",
  "LANG",
  "LC_ALL",
  "TERM",
  "TMPDIR",
] as const;

/**
 * Copies only the allowlisted names from `base`, then the names the operator
 * listed in `pass`, then applies `extras`; CAPSTAN_ variables never come from
 * `base`. A passed value that is not acceptable fails and names the variable,
 * never the value.
 */
export function buildAgentEnvironment(
  base: NodeJS.ProcessEnv,
  extras: Readonly<Record<string, string>>,
  pass: readonly string[] = [],
): Record<string, string> {
  const environment: Record<string, string> = {};
  for (const name of [...ALLOWLISTED_BASE, ...pass]) {
    if (name.startsWith("CAPSTAN_") || !ENVIRONMENT_KEY.test(name))
      throw new InvalidArgumentError(
        `environment name ${name} is not acceptable`,
      );
    const value = base[name];
    // An empty value of a listed name counts as unset: the launcher reports it, and nothing is passed.
    if (value !== undefined && (value !== "" || !pass.includes(name)))
      environment[name] = value;
  }
  for (const [name, value] of Object.entries(extras)) environment[name] = value;
  for (const [name, value] of Object.entries(environment)) {
    if (!ENVIRONMENT_KEY.test(name))
      throw new InvalidArgumentError(
        `environment name ${name} is not acceptable`,
      );
    if (
      typeof value !== "string" ||
      !value.isWellFormed() ||
      UNSAFE_TEXT.test(value)
    )
      throw new InvalidArgumentError(
        `environment value for ${name} is not acceptable (one line of printable text is allowed; a trailing carriage return from a CRLF file is not)`,
      );
  }
  return environment;
}

/** Read-only and silent when nothing waits; Claude Code runs it after every tool call. */
export const INBOX_HOOK_COMMAND = "cstan inbox --hook";

export type ClaudeRoleSettings = Pick<
  ResolvedRole,
  "model" | "permissionMode" | "allow" | "deny" | "hooks"
> &
  Partial<Pick<ResolvedRole, "mcp">>;

/** The arguments Claude Code gets for a role. Herdr quotes each argument safely, so none is quoted here; a newline is refused because Herdr refuses it. */
export function claudeArguments(
  role: ClaudeRoleSettings,
  promptFile?: string,
): string[] {
  const args: string[] = [];
  if (role.model !== null) args.push("--model", role.model);
  args.push("--permission-mode", role.permissionMode);
  if (role.allow.length > 0) args.push("--allowedTools", ...role.allow);
  if (role.deny.length > 0) args.push("--disallowedTools", ...role.deny);
  const mcp = role.mcp ?? [];
  // JSON.stringify escapes control characters, so the check below cannot see them inside the config.
  for (const server of mcp)
    for (const text of [server.name, server.command, ...server.args])
      if (!text.isWellFormed() || CONTROL_CHARACTERS.test(text))
        throw new InvalidArgumentError(
          "an mcp server name, command or argument has control characters",
        );
  if (mcp.length > 0)
    args.push(
      "--mcp-config",
      JSON.stringify({
        mcpServers: Object.fromEntries(
          mcp.map((server) => [
            server.name,
            { type: "stdio", command: server.command, args: server.args },
          ]),
        ),
      }),
      "--strict-mcp-config",
    );
  // Both keys: Claude Code has renamed the attribution setting before.
  const attribution = {
    includeCoAuthoredBy: false,
    attribution: { commit: "", pr: "" },
  };
  if (role.hooks === "off")
    args.push(
      "--settings",
      JSON.stringify({ disableAllHooks: true, ...attribution }),
    );
  else
    args.push(
      "--settings",
      JSON.stringify({
        ...attribution,
        hooks: {
          PostToolUse: [
            {
              matcher: "*",
              hooks: [
                { type: "command", command: INBOX_HOOK_COMMAND, timeout: 5 },
              ],
            },
          ],
        },
      }),
    );
  if (promptFile !== undefined)
    args.push("--append-system-prompt-file", promptFile);
  for (const arg of args)
    if (
      typeof arg !== "string" ||
      arg.length === 0 ||
      !arg.isWellFormed() ||
      CONTROL_CHARACTERS.test(arg)
    )
      throw new InvalidArgumentError(
        "an agent argument is empty or has control characters",
      );
  const values = [
    ...(role.model === null ? [] : [role.model]),
    ...role.allow,
    ...role.deny,
    ...mcp.flatMap((server) => [server.command]),
  ];
  if (values.some((value) => value.startsWith("-")))
    throw new InvalidArgumentError(
      "a model, allow, deny or mcp command value must not start with a dash",
    );
  return args;
}
