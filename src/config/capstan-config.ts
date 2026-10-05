/** Reads capstan.toml and turns it into a CapstanConfig; every export of the config modules is re-exported here. */
import fs from "node:fs";
import path from "node:path";
import { parse as parseToml, TomlError } from "smol-toml";
import {
  MAX_FILE_BYTES,
  SESSION_PATTERN,
  type Table,
  enumValue,
  guardCredentialShape,
  optionalBoolean,
  optionalInteger,
  optionalString,
  optionalTable,
  passedEnvironmentNames,
  rejectUnknownKeys,
  requiredString,
  requiredTable,
} from "./primitives.js";
import {
  resolveArchitect,
  resolveNexora,
  resolveResearcher,
} from "./resolve-agents.js";
import { resolveOperator, resolvePromptRelay } from "./resolve-operator.js";
import {
  resolveDefaults,
  resolveHosts,
  resolveMcpServers,
  resolveRoles,
} from "./resolve-roles.js";
import {
  CONFIG_FILE_NAME,
  type CapstanConfig,
  ConfigError,
  DEFAULT_HERDR_SESSION,
  DEFAULT_KEEP_MIGRATION_BACKUPS,
  DEFAULT_MAX_WORKERS,
  DEFAULT_MIN_PANE_COLUMNS,
  DEFAULT_MIN_PANE_ROWS,
  DEFAULT_PM_STALE_MINUTES,
  DEFAULT_PM_WIDTH_PERCENT,
  DEFAULT_SUPERVISION_CHECK_SECONDS,
  DEFAULT_WORKTREE_SETUP_TIMEOUT_SECONDS,
  DEFAULT_WORKTREE_TEARDOWN_TIMEOUT_SECONDS,
  MAX_KEEP_MIGRATION_BACKUPS,
  MAX_MAX_WORKERS,
  MAX_WORKTREE_SETUP_CHARS,
  MAX_WORKTREE_SETUP_TIMEOUT_SECONDS,
  MAX_WORKTREE_TEARDOWN_TIMEOUT_SECONDS,
  NEXORA_PROJECT_FILE,
  type ResolvedEnvironment,
  type ResolvedLayout,
  type ResolvedLedger,
  type ResolvedLimits,
  type ResolvedNotifications,
  type ResolvedSupervision,
  type ResolvedTimers,
  type ResolvedWorktree,
  SPAWN_LAYOUTS,
} from "./types.js";
export * from "./types.js";
export * from "./starter.js";
export { NAME_PATTERN } from "./primitives.js";

const TIMER_DEFAULTS = {
  max_deferral_seconds: [120, 1, 3600],
  max_busy_deferral_seconds: [3600, 60, 86_400],
  pm_ack_timeout_seconds: [600, 1, 86_400],
  pm_notify_after_seconds: [300, 1, 86_400],
  notify_interval_seconds: [600, 1, 86_400],
  stall_after_seconds: [900, 1, 86_400],
  worker_ack_timeout_seconds: [600, 1, 86_400],
  finding_check_seconds: [1800, 60, 86_400],
  pm_wake_after_seconds: [20, 0, 3600],
  pm_wake_interval_seconds: [120, 10, 3600],
} as const;

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
      "supervision",
      "architect",
      "operator",
      "researcher",
      "mcp_servers",
      "prompt_relay",
      "nexora",
      "defaults",
      "limits",
      "ledger",
      "layout",
      "worktree",
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
  rejectUnknownKeys(
    notificationTable,
    ["herdr", "fallback", "pm_stale_minutes"],
    "notifications",
  );
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
    pmStaleMinutes: optionalInteger(
      notificationTable.pm_stale_minutes,
      "notifications.pm_stale_minutes",
      1,
      1440,
      DEFAULT_PM_STALE_MINUTES,
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
    maxBusyDeferralSeconds: timerValue("max_busy_deferral_seconds"),
    pmAckTimeoutSeconds: timerValue("pm_ack_timeout_seconds"),
    pmNotifyAfterSeconds: timerValue("pm_notify_after_seconds"),
    notifyIntervalSeconds: timerValue("notify_interval_seconds"),
    stallAfterSeconds: timerValue("stall_after_seconds"),
    workerAckTimeoutSeconds: timerValue("worker_ack_timeout_seconds"),
    findingCheckSeconds: timerValue("finding_check_seconds"),
    pmWakeAfterSeconds: timerValue("pm_wake_after_seconds"),
    pmWakeIntervalSeconds: timerValue("pm_wake_interval_seconds"),
  };

  const supervisionTable = optionalTable(root.supervision, "supervision");
  rejectUnknownKeys(
    supervisionTable,
    ["enabled", "check_seconds"],
    "supervision",
  );
  const supervision: ResolvedSupervision = {
    enabled: optionalBoolean(
      supervisionTable.enabled,
      "supervision.enabled",
      true,
    ),
    checkSeconds: optionalInteger(
      supervisionTable.check_seconds,
      "supervision.check_seconds",
      60,
      3600,
      DEFAULT_SUPERVISION_CHECK_SECONDS,
    ),
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

  const ledgerTable = optionalTable(root.ledger, "ledger");
  rejectUnknownKeys(ledgerTable, ["keep_migration_backups"], "ledger");
  const ledger: ResolvedLedger = {
    keepMigrationBackups: optionalInteger(
      ledgerTable.keep_migration_backups,
      "ledger.keep_migration_backups",
      1,
      MAX_KEEP_MIGRATION_BACKUPS,
      DEFAULT_KEEP_MIGRATION_BACKUPS,
    ),
  };

  const layoutTable = optionalTable(root.layout, "layout");
  rejectUnknownKeys(
    layoutTable,
    ["spawn", "split", "pm_width_percent", "min_pane_columns", "min_pane_rows"],
    "layout",
  );
  const warnings: string[] = [];
  if (layoutTable.split !== undefined) {
    enumValue(layoutTable.split, "layout.split", [
      "auto",
      "right",
      "down",
    ] as const);
    warnings.push(
      "layout.split is ignored: the PM pane keeps the left layout.pm_width_percent of its tab and worker panes stack in the column on its right",
    );
  }
  const layout: ResolvedLayout = {
    spawn:
      layoutTable.spawn === undefined
        ? "tab"
        : enumValue(layoutTable.spawn, "layout.spawn", SPAWN_LAYOUTS),
    pmWidthPercent: optionalInteger(
      layoutTable.pm_width_percent,
      "layout.pm_width_percent",
      30,
      80,
      DEFAULT_PM_WIDTH_PERCENT,
    ),
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

  const worktree = resolveWorktree(optionalTable(root.worktree, "worktree"));

  const envTable = optionalTable(root.env, "env");
  rejectUnknownKeys(envTable, ["pass"], "env");
  const env: ResolvedEnvironment = {
    pass: passedEnvironmentNames(envTable.pass),
  };

  const hosts = resolveHosts(requiredTable(root.hosts, "hosts"));
  const hostsByName = new Map(hosts.map((host) => [host.name, host]));
  const mcpServers = resolveMcpServers(
    optionalTable(root.mcp_servers, "mcp_servers"),
    warnings,
  );
  const roles = resolveRoles(
    requiredTable(root.roles, "roles"),
    hostsByName,
    projectRoot,
    resolveDefaults(optionalTable(root.defaults, "defaults")),
    mcpServers,
  );
  if (roles.filter((role) => role.kind === "PM").length !== 1)
    throw new ConfigError("exactly one role must have kind PM");
  const architect = resolveArchitect(
    optionalTable(root.architect, "architect"),
    roles,
    hostsByName,
  );
  const operator = resolveOperator(
    optionalTable(root.operator, "operator"),
    root.operator !== undefined,
    roles,
    hostsByName,
    architect,
  );
  const researcher = resolveResearcher(
    optionalTable(root.researcher, "researcher"),
    root.researcher !== undefined,
    roles,
    hostsByName,
    architect,
    operator,
  );
  const promptRelay = resolvePromptRelay(
    optionalTable(root.prompt_relay, "prompt_relay"),
    root.prompt_relay !== undefined,
  );
  const nexora = resolveNexora(optionalTable(root.nexora, "nexora"));
  if (
    nexora.track !== "never" &&
    !fs.existsSync(path.join(projectRoot, NEXORA_PROJECT_FILE))
  )
    warnings.push(
      `nexora.track is "${nexora.track}" but ${NEXORA_PROJECT_FILE} is missing from the project root, so the PM will not track work in Nexora; set track = "never" in [nexora] to silence this`,
    );

  return {
    schemaVersion: 1,
    projectName,
    herdrSession,
    notifications,
    timers,
    supervision,
    architect,
    operator,
    researcher,
    mcpServers,
    promptRelay,
    nexora,
    limits,
    ledger,
    layout,
    ...(worktree === undefined ? {} : { worktree }),
    env,
    hosts,
    roles,
    warnings,
  };
}

function resolveWorktree(table: Table): ResolvedWorktree | undefined {
  rejectUnknownKeys(
    table,
    ["setup", "setup_timeout_seconds", "teardown", "teardown_timeout_seconds"],
    "worktree",
  );
  if (table.setup === undefined && table.setup_timeout_seconds !== undefined)
    throw new ConfigError(
      "worktree.setup_timeout_seconds is set without worktree.setup",
    );
  if (
    table.teardown === undefined &&
    table.teardown_timeout_seconds !== undefined
  )
    throw new ConfigError(
      "worktree.teardown_timeout_seconds is set without worktree.teardown",
    );
  if (table.setup === undefined && table.teardown === undefined)
    return undefined;
  let setup: string | undefined;
  if (table.setup !== undefined) {
    setup = requiredString(
      table.setup,
      "worktree.setup",
      MAX_WORKTREE_SETUP_CHARS,
    );
    guardCredentialShape(setup, "worktree.setup");
  }
  let teardown: string | undefined;
  if (table.teardown !== undefined) {
    teardown = requiredString(
      table.teardown,
      "worktree.teardown",
      MAX_WORKTREE_SETUP_CHARS,
    );
    guardCredentialShape(teardown, "worktree.teardown");
  }
  return {
    ...(setup === undefined ? {} : { setup }),
    setupTimeoutSeconds: optionalInteger(
      table.setup_timeout_seconds,
      "worktree.setup_timeout_seconds",
      1,
      MAX_WORKTREE_SETUP_TIMEOUT_SECONDS,
      DEFAULT_WORKTREE_SETUP_TIMEOUT_SECONDS,
    ),
    ...(teardown === undefined
      ? {}
      : {
          teardown,
          teardownTimeoutSeconds: optionalInteger(
            table.teardown_timeout_seconds,
            "worktree.teardown_timeout_seconds",
            1,
            MAX_WORKTREE_TEARDOWN_TIMEOUT_SECONDS,
            DEFAULT_WORKTREE_TEARDOWN_TIMEOUT_SECONDS,
          ),
        }),
  };
}
