/** Resolves [mcp_servers], [hosts], [defaults] and [roles], and each role's prompt. */
import { MAX_ROLE_NAME_CHARS } from "../herdr/naming.js";
import fs from "node:fs";
import path from "node:path";
import { digestJson, sha256 } from "../controller/canonical.js";
import {
  MAX_DENY_ENTRIES,
  MAX_ENTRY_CHARS,
  MAX_FILE_BYTES,
  MAX_LIST_ENTRIES,
  MAX_PROMPT_CHARS,
  PRINTABLE_LINE,
  type Table,
  assertSafeText,
  enumValue,
  guardCredentialShape,
  isExecutablePath,
  optionalInteger,
  optionalString,
  optionalTable,
  rejectUnknownKeys,
  requiredString,
  requiredTable,
  stringList,
  validatedNames,
} from "./primitives.js";
import {
  ConfigError,
  DEFAULT_WAIT_TIMEOUT_SECONDS,
  HOST_KINDS,
  MAX_WAIT_TIMEOUT_SECONDS,
  MCP_SERVER_NAME_PATTERN,
  PERMISSION_MODES,
  PM_DEFAULT_DENY,
  type PermissionMode,
  ROLE_KINDS,
  type ResolvedHost,
  type ResolvedMcpServer,
  type ResolvedRole,
  type RoleKind,
  SUPERVISOR_DEFAULT_DENY,
} from "./types.js";

export const MAX_MCP_SERVERS = 16;

/** `[mcp_servers.<name>]`: a stdio MCP server a role may use. The name becomes the mcp__<name>__ tool prefix. */
export function resolveMcpServers(
  table: Table,
  warnings: string[],
): ResolvedMcpServer[] {
  const names = Object.keys(table);
  if (names.length > MAX_MCP_SERVERS)
    throw new ConfigError(`mcp_servers exceeds ${MAX_MCP_SERVERS} servers`);
  return names.map((name) => {
    if (!MCP_SERVER_NAME_PATTERN.test(name))
      throw new ConfigError(
        `mcp_servers has a server name that does not match ${MCP_SERVER_NAME_PATTERN.source}`,
      );
    const at = `mcp_servers.${name}`;
    const server = requiredTable(table[name], at);
    rejectUnknownKeys(server, ["command", "args"], at);
    const command = requiredString(server.command, `${at}.command`, 200);
    if (command.startsWith("-"))
      throw new ConfigError(`${at}.command must not start with a dash`);
    if (!PRINTABLE_LINE.test(command))
      throw new ConfigError(
        `${at}.command must be one line of printable ASCII text`,
      );
    guardCredentialShape(command, `${at}.command`);
    let args: string[] = [];
    if (server.args !== undefined) {
      if (!Array.isArray(server.args))
        throw new ConfigError(`${at}.args must be an array of strings`);
      if (server.args.length > MAX_LIST_ENTRIES)
        throw new ConfigError(`${at}.args exceeds ${MAX_LIST_ENTRIES} entries`);
      args = server.args.map((entry, index) => {
        const text = requiredString(
          entry,
          `${at}.args[${index}]`,
          MAX_ENTRY_CHARS,
        );
        guardCredentialShape(text, `${at}.args[${index}]`);
        return text;
      });
    }
    args.forEach((arg) => {
      if (arg.endsWith("@latest"))
        warnings.push(
          `${at}.args names ${arg}, an unpinned package; pin an exact version`,
        );
    });
    // The first non-flag argument of a package runner is the package; it needs an @version.
    if (/^(?:.*\/)?(?:npx|bunx|pnpx)$/.test(command)) {
      const pkg = args.find((arg) => !arg.startsWith("-"));
      if (
        pkg !== undefined &&
        !pkg.endsWith("@latest") &&
        !/.@[^@/]+$/.test(pkg)
      )
        warnings.push(
          `${at}.args names ${pkg}, a package with no @version; pin an exact version`,
        );
    }
    return { name, command, args };
  });
}

export function resolveRoleMcp(
  value: unknown,
  at: string,
  servers: readonly ResolvedMcpServer[],
): ResolvedMcpServer[] {
  const names = stringList(value, at);
  const seen = new Set<string>();
  return names.map((name, index) => {
    const server = servers.find((candidate) => candidate.name === name);
    if (server === undefined)
      throw new ConfigError(
        `${at}[${index}] does not name a configured [mcp_servers] table`,
      );
    if (seen.has(name))
      throw new ConfigError(`${at}[${index}] repeats ${name}`);
    seen.add(name);
    return server;
  });
}

export function resolveHosts(table: Table): ResolvedHost[] {
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

/**
 * Codex and OMP run unattended with full access (Codex's sandbox blocks the
 * daemon socket, and any approval prompt would leave the agent blocked), so a
 * rule this controller cannot enforce there is refused, not ignored.
 */
export function rejectUnenforceable(
  at: string,
  host: ResolvedHost,
  kind: RoleKind,
  permissionMode: PermissionMode,
  allow: readonly string[],
  deny: readonly string[],
  mcp: readonly ResolvedMcpServer[],
): void {
  if (kind === "PM" || kind === "Supervisor")
    throw new ConfigError(
      `${at}: a ${kind} role is read-only by design and only a claude host can enforce that; host ${host.name} is ${host.kind}`,
    );
  if (mcp.length > 0)
    throw new ConfigError(
      `${at}.mcp is a Claude Code option and host ${host.name} (${host.kind}) cannot enforce it`,
    );
  if (allow.length > 0 || deny.length > 0)
    throw new ConfigError(
      `${at}: allow and deny are Claude Code tool rules and host ${host.name} (${host.kind}) cannot enforce them`,
    );
  if (permissionMode !== "acceptEdits" && permissionMode !== "auto")
    throw new ConfigError(
      `${at}.permission_mode must be acceptEdits or auto on host ${host.name} (${host.kind}), which runs unattended with full access (set it on the role or in [defaults])`,
    );
}

export type RoleDefaults = {
  readonly model: string | null;
  readonly permissionMode: PermissionMode | null;
};

export type ResolvedDefaults = {
  readonly all: RoleDefaults;
  readonly byKind: Readonly<Record<RoleKind, RoleDefaults>>;
};

export function parseModel(value: unknown, at: string): string | null {
  const model = optionalString(value, at, 100);
  if (model !== null) {
    guardCredentialShape(model, at);
    if (model.startsWith("-"))
      throw new ConfigError(`${at} must not start with a dash`);
  }
  return model;
}

export function parsePermissionMode(
  value: unknown,
  at: string,
): PermissionMode | null {
  return value === undefined ? null : enumValue(value, at, PERMISSION_MODES);
}

/** `[defaults]` and `[defaults.<kind>]`: the model and permission mode a role takes when it sets none of its own. */
export function resolveDefaults(table: Table): ResolvedDefaults {
  rejectUnknownKeys(
    table,
    ["model", "permission_mode", ...ROLE_KINDS],
    "defaults",
  );
  const read = (source: Table, at: string): RoleDefaults => {
    rejectUnknownKeys(source, ["model", "permission_mode"], at);
    return {
      model: parseModel(source.model, `${at}.model`),
      permissionMode: parsePermissionMode(
        source.permission_mode,
        `${at}.permission_mode`,
      ),
    };
  };
  const all = read(
    Object.fromEntries(
      Object.entries(table).filter(
        ([key]) => !ROLE_KINDS.includes(key as RoleKind),
      ),
    ),
    "defaults",
  );
  const byKind = Object.fromEntries(
    ROLE_KINDS.map((kind) => [
      kind,
      read(optionalTable(table[kind], `defaults.${kind}`), `defaults.${kind}`),
    ]),
  ) as Record<RoleKind, RoleDefaults>;
  return { all, byKind };
}

export function resolveRoles(
  table: Table,
  hosts: ReadonlyMap<string, ResolvedHost>,
  projectRoot: string,
  defaults: ResolvedDefaults,
  mcpServers: readonly ResolvedMcpServer[],
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
        "mcp",
        "prompt",
        "prompt_file",
      ],
      at,
    );
    const kind = enumValue(role.kind, `${at}.kind`, ROLE_KINDS);
    if (name.length > MAX_ROLE_NAME_CHARS)
      throw new ConfigError(
        `${at}: the role name is longer than ${MAX_ROLE_NAME_CHARS} characters, so an agent name with the project prefix would not fit Herdr's 32`,
      );
    const hostName = requiredString(role.host, `${at}.host`, 32);
    const host = hosts.get(hostName);
    if (!host)
      throw new ConfigError(`${at}.host does not name a configured host`);
    const kindDefaults = defaults.byKind[kind];
    const model =
      role.model === undefined
        ? (kindDefaults.model ?? defaults.all.model)
        : parseModel(role.model, `${at}.model`);
    const permissionMode: PermissionMode =
      role.permission_mode === undefined
        ? (kindDefaults.permissionMode ??
          defaults.all.permissionMode ??
          "default")
        : (parsePermissionMode(
            role.permission_mode,
            `${at}.permission_mode`,
          ) as PermissionMode);
    const allow = stringList(role.allow, `${at}.allow`);
    const deny =
      role.deny === undefined && kind === "PM"
        ? [...PM_DEFAULT_DENY]
        : role.deny === undefined && kind === "Supervisor"
          ? [...SUPERVISOR_DEFAULT_DENY]
          : stringList(role.deny, `${at}.deny`, MAX_DENY_ENTRIES);
    const hooks =
      role.hooks === undefined
        ? "off"
        : enumValue(role.hooks, `${at}.hooks`, ["off", "inherit"] as const);
    const mcp = resolveRoleMcp(role.mcp, `${at}.mcp`, mcpServers);
    const { prompt, text: promptText } = resolvePrompt(role, at, projectRoot);
    if (host.kind !== "claude")
      rejectUnenforceable(at, host, kind, permissionMode, allow, deny, mcp);
    const resolved = {
      name,
      kind,
      host: host.name,
      model,
      permissionMode,
      allow,
      deny,
      hooks,
      ...(mcp.length === 0 ? {} : { mcp }),
      prompt,
    };
    const { source, hash } = prompt;
    const result = {
      ...resolved,
      mcp,
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

export function resolvePrompt(
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
