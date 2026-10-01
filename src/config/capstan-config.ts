import fs from "node:fs";
import path from "node:path";
import { parse as parseToml, TomlError } from "smol-toml";
import { digestJson, sha256 } from "../controller/canonical.js";

export const CONFIG_FILE_NAME = "capstan.toml";
export const DEFAULT_WAIT_TIMEOUT_SECONDS = 90;
export const MAX_WAIT_TIMEOUT_SECONDS = 3600;
export const DEFAULT_HERDR_SESSION = "default";

export const STARTER_CONFIG = `schema_version = 1

[limits]
max_workers = 3

[layout]
spawn = "pane"
split = "auto"

# Variables an agent needs beyond the basic ones (PATH, HOME, USER, LANG, TERM...) are copied
# from the environment where \`cstan start\` runs, never from your interactive shell file alone.
# Name them here (one-line values only); a name that is not set where the daemon starts is reported
# when agents launch.
# [env]
# pass = ["NEXORA_API_KEY"]

[hosts.claude]
kind = "claude"

[roles.pm]
kind = "PM"
host = "claude"

[roles.developer]
kind = "Developer"
host = "claude"
permission_mode = "acceptEdits"
allow = ["Bash(git *)"]
deny = ["Bash(git push)", "Bash(git push *)"]
prompt = "You implement code changes. Work only in your own worktree and commit your work on your own branch in small commits. Never push and never merge. When you finish, tell the project manager the branch name, what you changed and what you could not verify."

[roles.designer]
kind = "Developer"
host = "claude"
permission_mode = "acceptEdits"
allow = ["Bash(git *)"]
deny = ["Bash(git push)", "Bash(git push *)"]
prompt = "You design and build user interface and visual changes. Work only in your own worktree and commit your work on your own branch in small commits. Never push and never merge. When you finish, tell the project manager the branch name, what you changed and what you could not verify."

[roles.reviewer]
kind = "Verifier"
host = "claude"
permission_mode = "acceptEdits"
allow = ["Bash(git *)"]
deny = ["Write", "Edit", "NotebookEdit", "Agent", "Task", "Bash(git push)", "Bash(git push *)"]
prompt = "You review one commit when the controller asks. Read the change, do not edit any file and never push or merge. Judge correctness, tests and risk, and say plainly what you could not check."

[roles.tester]
kind = "Verifier"
host = "claude"
permission_mode = "acceptEdits"
allow = ["Bash(git *)"]
deny = ["Bash(git push)", "Bash(git push *)"]
prompt = "You test and verify behavior. Run the real checks, report exactly what passed and what failed, and add tests only when asked. Work only in your own worktree and commit any test changes on your own branch. Never push and never merge. When you finish, tell the project manager the branch name and the result."

[roles.supervisor]
kind = "Supervisor"
host = "claude"
deny = ["Write", "Edit", "NotebookEdit", "Agent", "Task", "Bash(git push)", "Bash(git push *)", "Bash(herdr *)"]
prompt = "You watch the other agents and raise findings when one is stuck. You only read and report through cstan; you never edit files and never run project commands."
`;
export const ROLE_KINDS = [
  "PM",
  "Developer",
  "Verifier",
  "Supervisor",
] as const;
export const HOST_KINDS = ["claude", "codex", "omp"] as const;
export const PERMISSION_MODES = [
  "default",
  "acceptEdits",
  "plan",
  "auto",
] as const;

export type RoleKind = (typeof ROLE_KINDS)[number];
export type HostKind = (typeof HOST_KINDS)[number];
export type PermissionMode = (typeof PERMISSION_MODES)[number];

export class ConfigError extends Error {
  override readonly name = "ConfigError";
}

export type ResolvedTimers = {
  readonly maxDeferralSeconds: number;
  readonly pmAckTimeoutSeconds: number;
  readonly pmNotifyAfterSeconds: number;
  readonly notifyIntervalSeconds: number;
  readonly stallAfterSeconds: number;
  readonly workerAckTimeoutSeconds: number;
  /** How long a finding may wait for its Supervisor's check before the controller escalates it. */
  readonly findingCheckSeconds: number;
};

export type ResolvedHost = {
  readonly name: string;
  readonly kind: HostKind;
  readonly command: string;
  readonly shellCommandTimeoutSeconds: number;
  readonly waitTimeoutSeconds: number;
};

export type ResolvedRole = {
  readonly name: string;
  readonly kind: RoleKind;
  readonly host: string;
  readonly model: string | null;
  readonly permissionMode: PermissionMode;
  readonly allow: readonly string[];
  readonly deny: readonly string[];
  readonly hooks: "off" | "inherit";
  readonly prompt: {
    readonly source: "inline" | "file" | "none";
    readonly path: string | null;
    readonly hash: string | null;
  };
  /** The prompt's text, kept for the launcher; it is not part of the role hash (the hash covers it through `prompt.hash`). */
  readonly promptText: string | null;
  readonly configHash: string;
};

export type ResolvedNotifications = {
  readonly herdr: boolean;
  readonly fallback: boolean;
};

export type ResolvedLimits = {
  /** The most worker agents (every agent except the PM) that may be active at once. */
  readonly maxWorkers: number;
};

export const DEFAULT_MAX_WORKERS = 3;

/** Names of environment variables copied from the daemon's environment into every agent it starts. */
export type ResolvedEnvironment = {
  readonly pass: readonly string[];
};

export const ENV_NAME_PATTERN = /^[A-Z_][A-Z0-9_]{0,63}$/;
/** Passed by default already, or able to change how the agent's shell or loader behaves. */
export const RESERVED_ENV_NAMES: ReadonlySet<string> = new Set([
  "PATH",
  "HOME",
  "USER",
  "LOGNAME",
  "LANG",
  "LC_ALL",
  "TERM",
  "TMPDIR",
  "IFS",
  "PS0",
  "PS1",
  "PS2",
  "PS4",
  "PROMPT_COMMAND",
  "BASH_ENV",
  "ENV",
  "SHELLOPTS",
  "BASHOPTS",
  "GLOBIGNORE",
  "HISTFILE",
  "CDPATH",
  "LD_PRELOAD",
  "LD_LIBRARY_PATH",
  "LD_AUDIT",
  "NODE_OPTIONS",
  "ZDOTDIR",
  "SHELL",
  "PYTHONSTARTUP",
  "PERL5OPT",
]);
/**
 * Prefixes of names that steer the loader, git or the agent's bash. These lists
 * guard against accidents in a file the operator writes; they are not a
 * security boundary, and other interpreter variables (PYTHONPATH and the like)
 * are left to the operator.
 */
export const RESERVED_ENV_PREFIXES: readonly string[] = [
  "LD_",
  "DYLD_",
  "GIT_",
  "BASH_",
];
export const MAX_PASSED_ENV_NAMES = 32;
export const MAX_MAX_WORKERS = 16;

/** The tools a PM may not use unless its role sets `deny` itself: it delegates and never edits files or starts Claude Code's own subagents (the subagent tool was called Task in older versions). */
export const PM_DEFAULT_DENY: readonly string[] = [
  "Write",
  "Edit",
  "NotebookEdit",
  "Agent",
  "Task",
];

/** A Supervisor only reads and reports: the same file and subagent tools are denied unless its role sets `deny` itself. */
export const SUPERVISOR_DEFAULT_DENY: readonly string[] = PM_DEFAULT_DENY;

export const SPAWN_LAYOUTS = ["tab", "pane"] as const;
export const SPLIT_DIRECTIONS = ["auto", "right", "down"] as const;
export const DEFAULT_MIN_PANE_COLUMNS = 60;
export const DEFAULT_MIN_PANE_ROWS = 12;

export type ResolvedLayout = {
  /** "tab": each worker gets its own Herdr workspace; "pane": it is split into the PM's tab. */
  readonly spawn: (typeof SPAWN_LAYOUTS)[number];
  /** "right" puts panes side by side, "down" stacks them; "auto" picks by the target's size. */
  readonly split: (typeof SPLIT_DIRECTIONS)[number];
  readonly minPaneColumns: number;
  readonly minPaneRows: number;
};

export type CapstanConfig = {
  readonly schemaVersion: 1;
  readonly projectName: string | null;
  readonly herdrSession: string;
  readonly notifications: ResolvedNotifications;
  readonly timers: ResolvedTimers;
  readonly limits: ResolvedLimits;
  readonly layout: ResolvedLayout;
  readonly env: ResolvedEnvironment;
  readonly hosts: readonly ResolvedHost[];
  readonly roles: readonly ResolvedRole[];
};

const MAX_FILE_BYTES = 64 * 1024;
const MAX_PROMPT_CHARS = MAX_FILE_BYTES;
const MAX_LIST_ENTRIES = 64;
const MAX_ENTRY_CHARS = 200;
export const NAME_PATTERN = /^[a-z][a-z0-9-]{0,31}$/;
const SESSION_PATTERN = /^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$/;
const COMMAND_PATTERN = /^(?:\.\/)?[A-Za-z0-9_/][A-Za-z0-9._/-]{0,199}$/;
const UNSAFE_CHARACTERS = /[\p{Cc}\p{Cf}\p{Zl}\p{Zp}]/u;
const CREDENTIAL_SHAPES: readonly RegExp[] = [
  /\bsk-[A-Za-z0-9_-]{8,}/,
  /\bgh[pousr]_[A-Za-z0-9]{8,}/,
  /\bAKIA[A-Z0-9]{8,}/,
  /\bBearer[ \t]+(?=[A-Za-z0-9._~+/=-]*\d)[A-Za-z0-9._~+/=-]{16,}/,
  /-----BEGIN [A-Z ]*PRIVATE KEY-----/,
];

const TIMER_DEFAULTS = {
  max_deferral_seconds: [120, 1, 3600],
  pm_ack_timeout_seconds: [600, 1, 86_400],
  pm_notify_after_seconds: [300, 1, 86_400],
  notify_interval_seconds: [600, 1, 86_400],
  stall_after_seconds: [900, 1, 86_400],
  worker_ack_timeout_seconds: [600, 1, 86_400],
  finding_check_seconds: [1800, 60, 86_400],
} as const;

type Table = Record<string, unknown>;

export function loadCapstanConfig(projectRoot: string): CapstanConfig {
  const file = path.join(projectRoot, CONFIG_FILE_NAME);
  let descriptor: number;
  try {
    descriptor = fs.openSync(
      file,
      fs.constants.O_RDONLY | fs.constants.O_NOFOLLOW,
    );
  } catch (error) {
    const code = error instanceof Error && "code" in error ? error.code : "";
    if (code === "ENOENT")
      throw new ConfigError(`${CONFIG_FILE_NAME} does not exist`);
    if (code === "ELOOP")
      throw new ConfigError(`${CONFIG_FILE_NAME} must be a regular file`);
    throw error;
  }
  try {
    const stat = fs.fstatSync(descriptor);
    if (!stat.isFile())
      throw new ConfigError(`${CONFIG_FILE_NAME} must be a regular file`);
    if (process.getuid && stat.uid !== process.getuid())
      throw new ConfigError(
        `${CONFIG_FILE_NAME} must be owned by the current user`,
      );
    if ((stat.mode & 0o022) !== 0)
      throw new ConfigError(
        `${CONFIG_FILE_NAME} must not be writable by group or others`,
      );
    if (stat.size > MAX_FILE_BYTES)
      throw new ConfigError(
        `${CONFIG_FILE_NAME} exceeds ${MAX_FILE_BYTES} bytes`,
      );
    return parseCapstanConfig(fs.readFileSync(descriptor), projectRoot);
  } finally {
    fs.closeSync(descriptor);
  }
}

export function parseCapstanConfig(
  bytes: Buffer,
  projectRoot: string,
): CapstanConfig {
  if (bytes.length > MAX_FILE_BYTES)
    throw new ConfigError(
      `${CONFIG_FILE_NAME} exceeds ${MAX_FILE_BYTES} bytes`,
    );
  let source: string;
  try {
    source = new TextDecoder("utf-8", { fatal: true, ignoreBOM: true }).decode(
      bytes,
    );
  } catch {
    throw new ConfigError(`${CONFIG_FILE_NAME} is not valid UTF-8`);
  }
  if (source.startsWith("\uFEFF")) source = source.slice(1);
  source = source.replace(/\r\n/g, "\n");

  let root: Table;
  try {
    root = parseToml(source, { integersAsBigInt: true }) as Table;
  } catch (error) {
    // The parser message quotes the offending line, which may hold a secret.
    if (error instanceof TomlError)
      throw new ConfigError(
        `${CONFIG_FILE_NAME} is not valid TOML at line ${error.line}, column ${error.column}`,
      );
    throw new ConfigError(`${CONFIG_FILE_NAME} is not valid TOML`);
  }

  rejectUnknownKeys(
    root,
    [
      "schema_version",
      "project",
      "herdr_session",
      "notifications",
      "timers",
      "limits",
      "layout",
      "env",
      "hosts",
      "roles",
    ],
    "top level",
  );
  if (root.schema_version !== 1n)
    throw new ConfigError("schema_version must be the integer 1");

  const project = optionalTable(root.project, "project");
  rejectUnknownKeys(project, ["name"], "project");
  const projectName = optionalString(project.name, "project.name", 256);
  if (projectName !== null) guardCredentialShape(projectName, "project.name");

  const herdrSession =
    optionalString(root.herdr_session, "herdr_session", 64) ??
    DEFAULT_HERDR_SESSION;
  if (!SESSION_PATTERN.test(herdrSession))
    throw new ConfigError(`herdr_session must match ${SESSION_PATTERN.source}`);

  const notificationTable = optionalTable(root.notifications, "notifications");
  rejectUnknownKeys(notificationTable, ["herdr", "fallback"], "notifications");
  const notifications: ResolvedNotifications = {
    herdr: optionalBoolean(
      notificationTable.herdr,
      "notifications.herdr",
      true,
    ),
    fallback: optionalBoolean(
      notificationTable.fallback,
      "notifications.fallback",
      true,
    ),
  };
  if (!notifications.herdr && !notifications.fallback)
    throw new ConfigError(
      "notifications.herdr and notifications.fallback must not both be false",
    );

  const timerTable = optionalTable(root.timers, "timers");
  rejectUnknownKeys(timerTable, Object.keys(TIMER_DEFAULTS), "timers");
  const timerValue = (key: keyof typeof TIMER_DEFAULTS): number => {
    const [fallback, min, max] = TIMER_DEFAULTS[key];
    return optionalInteger(
      timerTable[key],
      `timers.${key}`,
      min,
      max,
      fallback,
    );
  };
  const timers: ResolvedTimers = {
    maxDeferralSeconds: timerValue("max_deferral_seconds"),
    pmAckTimeoutSeconds: timerValue("pm_ack_timeout_seconds"),
    pmNotifyAfterSeconds: timerValue("pm_notify_after_seconds"),
    notifyIntervalSeconds: timerValue("notify_interval_seconds"),
    stallAfterSeconds: timerValue("stall_after_seconds"),
    workerAckTimeoutSeconds: timerValue("worker_ack_timeout_seconds"),
    findingCheckSeconds: timerValue("finding_check_seconds"),
  };

  const limitTable = optionalTable(root.limits, "limits");
  rejectUnknownKeys(limitTable, ["max_workers"], "limits");
  const limits: ResolvedLimits = {
    maxWorkers: optionalInteger(
      limitTable.max_workers,
      "limits.max_workers",
      1,
      MAX_MAX_WORKERS,
      DEFAULT_MAX_WORKERS,
    ),
  };

  const layoutTable = optionalTable(root.layout, "layout");
  rejectUnknownKeys(
    layoutTable,
    ["spawn", "split", "min_pane_columns", "min_pane_rows"],
    "layout",
  );
  const layout: ResolvedLayout = {
    spawn:
      layoutTable.spawn === undefined
        ? "tab"
        : enumValue(layoutTable.spawn, "layout.spawn", SPAWN_LAYOUTS),
    split:
      layoutTable.split === undefined
        ? "auto"
        : enumValue(layoutTable.split, "layout.split", SPLIT_DIRECTIONS),
    minPaneColumns: optionalInteger(
      layoutTable.min_pane_columns,
      "layout.min_pane_columns",
      1,
      500,
      DEFAULT_MIN_PANE_COLUMNS,
    ),
    minPaneRows: optionalInteger(
      layoutTable.min_pane_rows,
      "layout.min_pane_rows",
      1,
      200,
      DEFAULT_MIN_PANE_ROWS,
    ),
  };

  const envTable = optionalTable(root.env, "env");
  rejectUnknownKeys(envTable, ["pass"], "env");
  const env: ResolvedEnvironment = {
    pass: passedEnvironmentNames(envTable.pass),
  };

  const hosts = resolveHosts(requiredTable(root.hosts, "hosts"));
  const hostsByName = new Map(hosts.map((host) => [host.name, host]));
  const roles = resolveRoles(
    requiredTable(root.roles, "roles"),
    hostsByName,
    projectRoot,
  );
  if (roles.filter((role) => role.kind === "PM").length !== 1)
    throw new ConfigError("exactly one role must have kind PM");

  return {
    schemaVersion: 1,
    projectName,
    herdrSession,
    notifications,
    timers,
    limits,
    layout,
    env,
    hosts,
    roles,
  };
}

function resolveHosts(table: Table): ResolvedHost[] {
  const names = validatedNames(table, "hosts", "host");
  if (names.length === 0)
    throw new ConfigError("hosts must define at least one host");
  return names.map((name) => {
    const at = `hosts.${name}`;
    const host = requiredTable(table[name], at);
    rejectUnknownKeys(
      host,
      [
        "kind",
        "command",
        "shell_command_timeout_seconds",
        "wait_timeout_seconds",
      ],
      at,
    );
    const kind = enumValue(host.kind, `${at}.kind`, HOST_KINDS);
    const command = optionalString(host.command, `${at}.command`, 200) ?? kind;
    if (!isExecutablePath(command))
      throw new ConfigError(`${at}.command must be an executable name or path`);
    guardCredentialShape(command, `${at}.command`);
    const shellCommandTimeoutSeconds = optionalInteger(
      host.shell_command_timeout_seconds,
      `${at}.shell_command_timeout_seconds`,
      1,
      3600,
      120,
    );
    const waitTimeoutSeconds = optionalInteger(
      host.wait_timeout_seconds,
      `${at}.wait_timeout_seconds`,
      1,
      MAX_WAIT_TIMEOUT_SECONDS,
      DEFAULT_WAIT_TIMEOUT_SECONDS,
    );
    if (waitTimeoutSeconds >= shellCommandTimeoutSeconds)
      throw new ConfigError(
        `${at}.wait_timeout_seconds (${waitTimeoutSeconds}) must be below ${at}.shell_command_timeout_seconds (${shellCommandTimeoutSeconds})`,
      );
    return {
      name,
      kind,
      command,
      shellCommandTimeoutSeconds,
      waitTimeoutSeconds,
    };
  });
}

function resolveRoles(
  table: Table,
  hosts: ReadonlyMap<string, ResolvedHost>,
  projectRoot: string,
): ResolvedRole[] {
  const names = validatedNames(table, "roles", "role");
  if (names.length === 0)
    throw new ConfigError("roles must define at least one role");
  return names.map((name) => {
    const at = `roles.${name}`;
    const role = requiredTable(table[name], at);
    rejectUnknownKeys(
      role,
      [
        "kind",
        "host",
        "model",
        "permission_mode",
        "allow",
        "deny",
        "hooks",
        "prompt",
        "prompt_file",
      ],
      at,
    );
    const kind = enumValue(role.kind, `${at}.kind`, ROLE_KINDS);
    const hostName = requiredString(role.host, `${at}.host`, 32);
    const host = hosts.get(hostName);
    if (!host)
      throw new ConfigError(`${at}.host does not name a configured host`);
    const model = optionalString(role.model, `${at}.model`, 100);
    if (model !== null) {
      guardCredentialShape(model, `${at}.model`);
      if (model.startsWith("-"))
        throw new ConfigError(`${at}.model must not start with a dash`);
    }
    const permissionMode =
      role.permission_mode === undefined
        ? "default"
        : enumValue(
            role.permission_mode,
            `${at}.permission_mode`,
            PERMISSION_MODES,
          );
    const allow = stringList(role.allow, `${at}.allow`);
    const deny =
      role.deny === undefined && kind === "PM"
        ? [...PM_DEFAULT_DENY]
        : role.deny === undefined && kind === "Supervisor"
          ? [...SUPERVISOR_DEFAULT_DENY]
          : stringList(role.deny, `${at}.deny`);
    const hooks =
      role.hooks === undefined
        ? "off"
        : enumValue(role.hooks, `${at}.hooks`, ["off", "inherit"] as const);
    const { prompt, text: promptText } = resolvePrompt(role, at, projectRoot);
    const resolved = {
      name,
      kind,
      host: host.name,
      model,
      permissionMode,
      allow,
      deny,
      hooks,
      prompt,
    };
    const { source, hash } = prompt;
    const result = {
      ...resolved,
      configHash: digestJson({
        role: { ...resolved, prompt: { source, hash } },
        host,
      }),
    };
    // Not enumerable: `cstan config check` prints roles as JSON and must not echo prompt text.
    Object.defineProperty(result, "promptText", {
      value: promptText,
      enumerable: false,
    });
    return result as typeof result & { readonly promptText: string | null };
  });
}

function resolvePrompt(
  role: Table,
  at: string,
  projectRoot: string,
): { prompt: ResolvedRole["prompt"]; text: string | null } {
  if (role.prompt !== undefined && role.prompt_file !== undefined)
    throw new ConfigError(`${at} sets both prompt and prompt_file`);
  if (role.prompt !== undefined) {
    const text = requiredString(
      role.prompt,
      `${at}.prompt`,
      MAX_PROMPT_CHARS,
      true,
    );
    guardCredentialShape(text, `${at}.prompt`);
    return {
      prompt: { source: "inline", path: null, hash: sha256(text) },
      text,
    };
  }
  if (role.prompt_file === undefined)
    return { prompt: { source: "none", path: null, hash: null }, text: null };
  const relative = requiredString(role.prompt_file, `${at}.prompt_file`, 200);
  if (path.isAbsolute(relative))
    throw new ConfigError(`${at}.prompt_file must be relative to the project`);
  let realRoot: string;
  let realFile: string;
  try {
    realRoot = fs.realpathSync(projectRoot);
    realFile = fs.realpathSync(path.resolve(realRoot, relative));
  } catch {
    throw new ConfigError(`${at}.prompt_file does not exist`);
  }
  if (!realFile.startsWith(`${realRoot}${path.sep}`))
    throw new ConfigError(`${at}.prompt_file must stay inside the project`);
  let bytes: Buffer;
  try {
    const descriptor = fs.openSync(
      realFile,
      fs.constants.O_RDONLY | fs.constants.O_NOFOLLOW,
    );
    try {
      const stat = fs.fstatSync(descriptor);
      if (!stat.isFile() || stat.size > MAX_FILE_BYTES)
        throw new ConfigError(
          `${at}.prompt_file must be a regular file of at most ${MAX_FILE_BYTES} bytes`,
        );
      bytes = fs.readFileSync(descriptor);
      if (bytes.length > MAX_FILE_BYTES)
        throw new ConfigError(
          `${at}.prompt_file must be a regular file of at most ${MAX_FILE_BYTES} bytes`,
        );
    } finally {
      fs.closeSync(descriptor);
    }
  } catch (error) {
    if (error instanceof ConfigError) throw error;
    throw new ConfigError(`${at}.prompt_file cannot be read as a regular file`);
  }
  let text: string;
  try {
    text = new TextDecoder("utf-8", { fatal: true, ignoreBOM: true })
      .decode(bytes)
      .replace(/^\uFEFF/, "")
      .replace(/\r\n/g, "\n");
  } catch {
    throw new ConfigError(`${at}.prompt_file is not valid UTF-8`);
  }
  if (text.trim().length === 0)
    throw new ConfigError(`${at}.prompt_file must not be empty`);
  assertSafeText(text, `${at}.prompt_file`, true);
  guardCredentialShape(text, `${at}.prompt_file`);
  return {
    prompt: { source: "file", path: realFile, hash: sha256(text) },
    text,
  };
}

function isExecutablePath(command: string): boolean {
  return (
    COMMAND_PATTERN.test(command) &&
    !command.endsWith("/") &&
    ![".", ".."].includes(path.basename(command))
  );
}

function validatedNames(table: Table, at: string, noun: string): string[] {
  const names = Object.keys(table);
  for (const name of names) {
    if (!NAME_PATTERN.test(name))
      throw new ConfigError(
        `${at} has a ${noun} name that does not match ${NAME_PATTERN.source}`,
      );
    guardCredentialShape(name, `${at} ${noun} name`);
  }
  return names;
}

function rejectUnknownKeys(
  table: Table,
  allowed: readonly string[],
  at: string,
): void {
  const unknown = Object.keys(table).filter((key) => !allowed.includes(key));
  if (unknown.length > 0)
    throw new ConfigError(
      `${at} has ${unknown.length} unknown key(s); allowed: ${allowed.join(", ")}`,
    );
}

function isTable(value: unknown): value is Table {
  if (value === null || typeof value !== "object" || Array.isArray(value))
    return false;
  const prototype = Object.getPrototypeOf(value);
  return prototype === null || prototype === Object.prototype;
}

function optionalTable(value: unknown, at: string): Table {
  if (value === undefined) return {};
  return requiredTable(value, at);
}

function requiredTable(value: unknown, at: string): Table {
  if (!isTable(value)) throw new ConfigError(`${at} must be a table`);
  return value;
}

function requiredString(
  value: unknown,
  at: string,
  maxChars: number,
  multiline = false,
): string {
  if (typeof value !== "string" || value.trim().length === 0)
    throw new ConfigError(`${at} must be a non-empty string`);
  if (value.length > maxChars)
    throw new ConfigError(`${at} exceeds ${maxChars} characters`);
  if (!multiline && value !== value.trim())
    throw new ConfigError(`${at} must not have leading or trailing whitespace`);
  assertSafeText(value, at, multiline);
  return value;
}

function assertSafeText(value: string, at: string, multiline: boolean): void {
  const checked = multiline ? value.replace(/[\n\t\u200c\u200d]/g, "") : value;
  if (UNSAFE_CHARACTERS.test(checked))
    throw new ConfigError(
      `${at} contains control, format or line-separator characters`,
    );
}

function optionalString(
  value: unknown,
  at: string,
  maxChars: number,
): string | null {
  return value === undefined ? null : requiredString(value, at, maxChars);
}

function optionalBoolean(
  value: unknown,
  at: string,
  fallback: boolean,
): boolean {
  if (value === undefined) return fallback;
  if (typeof value !== "boolean")
    throw new ConfigError(`${at} must be true or false`);
  return value;
}

function optionalInteger(
  value: unknown,
  at: string,
  min: number,
  max: number,
  fallback: number,
): number {
  if (value === undefined) return fallback;
  if (typeof value !== "bigint")
    throw new ConfigError(`${at} must be an integer`);
  if (value < BigInt(min) || value > BigInt(max))
    throw new ConfigError(`${at} must be between ${min} and ${max}`);
  return Number(value);
}

function enumValue<const T extends readonly string[]>(
  value: unknown,
  at: string,
  allowed: T,
): T[number] {
  if (typeof value !== "string" || !allowed.includes(value))
    throw new ConfigError(`${at} must be one of ${allowed.join(", ")}`);
  return value;
}

/** The names are written by the operator; values never appear in the file. */
function passedEnvironmentNames(value: unknown): string[] {
  if (value === undefined) return [];
  if (!Array.isArray(value))
    throw new ConfigError("env.pass must be an array of variable names");
  if (value.length > MAX_PASSED_ENV_NAMES)
    throw new ConfigError(`env.pass exceeds ${MAX_PASSED_ENV_NAMES} names`);
  const seen = new Set<string>();
  return value.map((entry, index) => {
    const at = `env.pass[${index}]`;
    if (typeof entry !== "string" || !ENV_NAME_PATTERN.test(entry))
      throw new ConfigError(
        `${at} must be an upper-case variable name (letters, digits and underscore, at most 64 characters)`,
      );
    if (entry.startsWith("CAPSTAN_"))
      throw new ConfigError(
        `${at} must not start with CAPSTAN_: those variables belong to Capstan`,
      );
    if (
      RESERVED_ENV_NAMES.has(entry) ||
      RESERVED_ENV_PREFIXES.some((prefix) => entry.startsWith(prefix))
    )
      throw new ConfigError(
        `${at} must not be ${entry}: Capstan already sets it or it changes how an agent's shell, loader or git behaves`,
      );
    if (seen.has(entry)) throw new ConfigError(`${at} repeats ${entry}`);
    seen.add(entry);
    return entry;
  });
}

function stringList(value: unknown, at: string): string[] {
  if (value === undefined) return [];
  if (!Array.isArray(value))
    throw new ConfigError(`${at} must be an array of strings`);
  if (value.length > MAX_LIST_ENTRIES)
    throw new ConfigError(`${at} exceeds ${MAX_LIST_ENTRIES} entries`);
  return value.map((entry, index) => {
    const text = requiredString(entry, `${at}[${index}]`, MAX_ENTRY_CHARS);
    guardCredentialShape(text, `${at}[${index}]`);
    if (text.startsWith("-"))
      throw new ConfigError(`${at}[${index}] must not start with a dash`);
    return text;
  });
}

function guardCredentialShape(text: string, at: string): void {
  if (CREDENTIAL_SHAPES.some((shape) => shape.test(text)))
    throw new ConfigError(
      `${at} looks like a credential; ${CONFIG_FILE_NAME} must not hold secrets`,
    );
}
