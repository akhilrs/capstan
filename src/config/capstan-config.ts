import { MAX_ROLE_NAME_CHARS } from "../herdr/naming.js";
import fs from "node:fs";
import path from "node:path";
import { parse as parseToml, TomlError } from "smol-toml";
import { digestJson, sha256 } from "../controller/canonical.js";
import { autoApproveRuleProblem } from "../operator-policy.js";

export const CONFIG_FILE_NAME = "capstan.toml";
export const DEFAULT_WAIT_TIMEOUT_SECONDS = 90;
export const MAX_WAIT_TIMEOUT_SECONDS = 3600;
export const DEFAULT_HERDR_SESSION = "default";

export const STARTER_CONFIG = `schema_version = 1

[limits]
max_workers = 3

[layout]
spawn = "pane"
pm_width_percent = 60

# While workers are active the controller keeps one Supervisor running and sends it a routine
# check. A Supervisor is a Claude session, so it uses usage; set enabled = false to turn it off.
[supervision]
enabled = true
check_seconds = 300

# An optional Architect plans normal and high-risk work, runs integration and signs it off. The
# user still merges to the main branch. While enabled = false, nothing about plans reaches an agent.
# To use it, remove the leading # from this table and from [roles.architect] below. The role must
# be a Developer role on a claude host; reviewer_role, if set, must be a Verifier role.
# [architect]
# enabled = true
# role = "architect"
# plan_review = "high_risk"     # "high_risk" | "always" | "never"
# reviewer_role = "reviewer"
# max_packages = 8              # 1 to 20
# count_toward_worker_limit = false
# high_risk_triggers = ["schema or migrations", "security or auth", "public contracts or wire formats", "cross-cutting changes"]

# An optional Operator runs shell commands for the PM, but only one proposal at a time and only after
# you approve it in the PM's picker; auto_approve lists exact read-only commands that skip the picker
# (the allowlist is in src/operator-policy.ts). While enabled = false, nothing about an Operator
# reaches an agent. To use it, remove the leading # from this table and from [roles.operator] below.
# The role must be a Developer role on a claude host, different from the architect role.
# [operator]
# enabled = true
# role = "operator"
# auto_approve = ["ls -l", "git rev-parse --short HEAD"]   # exact commands only
# auto_approve_prefix = []      # opt-in prefixes; only safe path arguments may follow
# timeout_seconds = 300
# max_timeout_seconds = 1800    # at most 3600
# output_tail_bytes = 8192      # at most 12288
# proposal_ttl_minutes = 60
# approval_ttl_minutes = 10
# max_pending_proposals = 5
# count_toward_worker_limit = false
# restart_health_timeout_seconds = 60
# restart_idle_wait_seconds = 120
# session_grant_max_minutes = 60    # how long "approve and allow again" grants last; at most 480
# full_auto_default_minutes = 30    # full auto: the PM may switch off every guard for this long, only after asking you
# full_auto_max_minutes = 120       # the longest full auto period; at most 480

# Whether the PM mirrors work into Nexora. Policy only: connection details stay in .nexora.toml,
# which Capstan never reads. "ask" shows the PM's intake picker, "always" applies default_action
# without asking, "never" removes every Nexora instruction from the PM prompt.
# [nexora]
# track = "ask"                 # "ask" | "always" | "never"
# default_action = "create"     # what "always" does: "create" | "link" | "none"

# The model and permission mode a role gets when it sets none of its own, by role kind
# (PM, Supervisor, Developer, Verifier). [defaults] itself applies to every kind. A role's own
# model or permission_mode always wins. permission_mode = "auto" lets Claude Code's auto mode
# approve routine actions, so agents stall at permission prompts less often.
[defaults.PM]
model = "claude-opus-5-5"

[defaults.Supervisor]
model = "claude-opus-5-5"

[defaults.Developer]
model = "claude-sonnet-5-5"
# permission_mode = "auto"

[defaults.Verifier]
model = "claude-sonnet-5-5"

# Variables an agent needs beyond the basic ones (PATH, HOME, USER, LANG, TERM...) are copied
# from the environment where \`cstan start\` runs, never from your interactive shell file alone.
# Name them here (one-line values only); a name that is not set where the daemon starts is reported
# when agents launch.
# [env]
# pass = ["NEXORA_API_KEY"]

[hosts.claude]
kind = "claude"

# Optional Codex and OMP hosts for Developer and Verifier roles. Both run unattended with full access
# and without a sandbox: nothing stops a push or an edit outside the worktree, and \`cstan config check\`
# warns about every such role. PM, Supervisor, architect and operator roles stay on a claude host.
# [hosts.codex]
# kind = "codex"
# [hosts.omp]
# kind = "omp"

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
deny = ["Write", "Edit", "NotebookEdit", "Agent", "Task", "Bash(git push)", "Bash(git push *)", "Bash(herdr *)", "Bash(tmux *)"]
prompt = "You watch the other agents and raise findings when one is stuck. You only read and report through cstan; you never edit files and never run project commands."

# A worker role on Codex or OMP (remove the leading # here and from the host above). It must set its
# own model, because the [defaults.Developer] model is a Claude model; it must set permission_mode
# to acceptEdits or auto; and it must have no allow or deny. Use any model your CLI accepts.
# [roles.codex-developer]
# kind = "Developer"
# host = "codex"
# model = "<a model name your codex CLI accepts>"
# permission_mode = "acceptEdits"
# prompt = "You implement code changes. Work only in your own worktree and commit your work on your own branch in small commits. Never push and never merge. When you finish, tell the project manager the branch name, what you changed and what you could not verify."

# [roles.architect]
# kind = "Developer"
# host = "claude"
# permission_mode = "acceptEdits"
# allow = ["Bash(git *)"]
# deny = ["Write", "Edit", "NotebookEdit", "Agent", "Task", "Bash(git push)", "Bash(git push *)", "Bash(git merge *)"]
# prompt = "You plan and integrate; you never edit or commit project files. Follow the project rules the PM gives you."

# [roles.operator]
# kind = "Developer"
# host = "claude"
# permission_mode = "default"
# allow = ["Bash(cstan *)"]
# deny = ["Write", "Edit", "NotebookEdit", "Agent", "Task", "Read", "Glob", "Grep"]
# prompt = "You run shell commands for the PM through cstan op propose, and nothing else."
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
  /** How long a message waits for a busy or blocked worker before it expires and the PM is told. */
  readonly maxBusyDeferralSeconds: number;
  readonly pmAckTimeoutSeconds: number;
  readonly pmNotifyAfterSeconds: number;
  readonly notifyIntervalSeconds: number;
  readonly stallAfterSeconds: number;
  readonly workerAckTimeoutSeconds: number;
  /** How long a finding may wait for its Supervisor's check before the controller escalates it. */
  readonly findingCheckSeconds: number;
  /** An unread message waits this long before the controller types a wake line into an idle PM; 0 turns the wake off. */
  readonly pmWakeAfterSeconds: number;
  readonly pmWakeIntervalSeconds: number;
};

export type ResolvedSupervision = {
  /** The controller keeps a Supervisor running while workers are active and queues it a routine check. */
  readonly enabled: boolean;
  readonly checkSeconds: number;
};

export const PLAN_REVIEW_MODES = ["high_risk", "always", "never"] as const;
export const DEFAULT_ARCHITECT_ROLE = "architect";
export const DEFAULT_ARCHITECT_MAX_PACKAGES = 8;
export const MAX_ARCHITECT_PACKAGES = 20;
export const DEFAULT_HIGH_RISK_TRIGGERS: readonly string[] = [
  "schema or migrations",
  "security or auth",
  "public contracts or wire formats",
  "cross-cutting changes",
];

export const DEFAULT_OPERATOR_ROLE = "operator";
export const OPERATOR_HARD_MAX_TIMEOUT_SECONDS = 3600;
export const MAX_OPERATOR_OUTPUT_TAIL_BYTES = 12288;
export const OPERATOR_HARD_MAX_SESSION_MINUTES = 480;
/** Tool rules the Operator role must deny so it cannot read the state directory, tokens or the key, or hand work to a subagent. */
export const OPERATOR_REQUIRED_DENY: readonly string[] = [
  "Write",
  "Edit",
  "NotebookEdit",
  "Agent",
  "Task",
  "Read",
  "Glob",
  "Grep",
];
const OPERATOR_ALLOW_RULE = /^Bash\(cstan[ :][^()`$;&|<>\\\n]*\)$/;

export const NEXORA_TRACK_MODES = ["ask", "always", "never"] as const;
export const NEXORA_DEFAULT_ACTIONS = ["create", "link", "none"] as const;
export const NEXORA_PROJECT_FILE = ".nexora.toml";

export type ResolvedNexora = {
  /** Policy only: Capstan never reads Nexora's own config or calls Nexora. */
  readonly track: (typeof NEXORA_TRACK_MODES)[number];
  /** What "always" does, and the picker's recommended option. */
  readonly defaultAction: (typeof NEXORA_DEFAULT_ACTIONS)[number];
};

export type ResolvedArchitect = {
  /** Off: no plan commands reach any prompt and behaviour is exactly that of a project without the table. */
  readonly enabled: boolean;
  /** The role of kind Developer that is the Architect; checked against the roles only when enabled. */
  readonly role: string;
  readonly planReview: (typeof PLAN_REVIEW_MODES)[number];
  /** A Verifier role for plan reviews; null means the reviewer choice the report reviews use. */
  readonly reviewerRole: string | null;
  readonly maxPackages: number;
  readonly countTowardWorkerLimit: boolean;
  /** Prompt text for the PM, not a classifier: the tier stays the PM's judgement. */
  readonly highRiskTriggers: readonly string[];
};

export type ResolvedOperator = {
  /** True only when `[operator]` is present in the file; an absent table leaves every role an ordinary role, even one named "operator". */
  readonly configured: boolean;
  /** Off: no operator text reaches any prompt and behaviour is exactly that of a project without the table. */
  readonly enabled: boolean;
  readonly role: string;
  /** Exact command strings that need no human decision; each is on the read-only allowlist. */
  readonly autoApprove: readonly string[];
  /** Opt-in command prefixes; only safe positional tokens may follow one. */
  readonly autoApprovePrefix: readonly string[];
  readonly timeoutSeconds: number;
  readonly maxTimeoutSeconds: number;
  readonly outputTailBytes: number;
  readonly proposalTtlMinutes: number;
  readonly approvalTtlMinutes: number;
  readonly maxPendingProposals: number;
  readonly countTowardWorkerLimit: boolean;
  readonly restartHealthTimeoutSeconds: number;
  readonly restartIdleWaitSeconds: number;
  /** The longest a session grant lasts; a grant also ends on release of the Operator agent and on controller restart. */
  readonly sessionGrantMaxMinutes: number;
  readonly fullAutoDefaultMinutes: number;
  readonly fullAutoMaxMinutes: number;
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

export const DEFAULT_WORKTREE_SETUP_TIMEOUT_SECONDS = 600;
export const MAX_WORKTREE_SETUP_TIMEOUT_SECONDS = 3600;
const MAX_WORKTREE_SETUP_CHARS = 1000;

export const MAX_WORKTREE_TEARDOWN_TIMEOUT_SECONDS = 3600;
export const DEFAULT_WORKTREE_TEARDOWN_TIMEOUT_SECONDS = 60;

export type ResolvedWorktree = {
  /** A shell command run once in each new worktree, with the worktree as its working directory. Absent when only `teardown` is set. */
  readonly setup?: string;
  readonly setupTimeoutSeconds: number;
  /** A shell command run in the project root just before a worktree is removed. */
  readonly teardown?: string;
  readonly teardownTimeoutSeconds?: number;
};

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

/**
 * A Supervisor only reads and reports: file, subagent and the common ways to
 * push or to type into another agent's pane are denied unless its role sets
 * `deny` itself. Tool rules, not a sandbox: Bash stays open for `cstan`.
 */
export const SUPERVISOR_DEFAULT_DENY: readonly string[] = [
  ...PM_DEFAULT_DENY,
  "Bash(git push)",
  "Bash(git push *)",
  "Bash(herdr *)",
  "Bash(tmux *)",
];

export const SPAWN_LAYOUTS = ["tab", "pane"] as const;
export const DEFAULT_PM_WIDTH_PERCENT = 60;
export const DEFAULT_MIN_PANE_COLUMNS = 60;
export const DEFAULT_MIN_PANE_ROWS = 12;

export type ResolvedLayout = {
  /** "tab": each worker gets its own Herdr workspace; "pane": it is split into the PM's tab. */
  readonly spawn: (typeof SPAWN_LAYOUTS)[number];
  /** How much of the PM's tab width the PM pane keeps when the first worker pane is placed; workers stack in the column on its right. */
  readonly pmWidthPercent: number;
  readonly minPaneColumns: number;
  readonly minPaneRows: number;
};

export type CapstanConfig = {
  readonly schemaVersion: 1;
  readonly projectName: string | null;
  readonly herdrSession: string;
  readonly notifications: ResolvedNotifications;
  readonly timers: ResolvedTimers;
  readonly supervision: ResolvedSupervision;
  readonly architect: ResolvedArchitect;
  readonly operator: ResolvedOperator;
  readonly nexora: ResolvedNexora;
  readonly limits: ResolvedLimits;
  readonly layout: ResolvedLayout;
  /** Absent when `[worktree]` sets no `setup`. */
  readonly worktree?: ResolvedWorktree;
  /** Things the loader accepted but the operator should know (an ignored key); `cstan config check` prints them. */
  readonly warnings: readonly string[];
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

export const DEFAULT_SUPERVISION_CHECK_SECONDS = 300;

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
      "supervision",
      "architect",
      "operator",
      "nexora",
      "defaults",
      "limits",
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
  const roles = resolveRoles(
    requiredTable(root.roles, "roles"),
    hostsByName,
    projectRoot,
    resolveDefaults(optionalTable(root.defaults, "defaults")),
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
    nexora,
    limits,
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

function resolveNexora(table: Table): ResolvedNexora {
  rejectUnknownKeys(table, ["track", "default_action"], "nexora");
  const nexora: ResolvedNexora = {
    track:
      table.track === undefined
        ? "ask"
        : enumValue(table.track, "nexora.track", NEXORA_TRACK_MODES),
    defaultAction:
      table.default_action === undefined
        ? "create"
        : enumValue(
            table.default_action,
            "nexora.default_action",
            NEXORA_DEFAULT_ACTIONS,
          ),
  };
  if (nexora.track === "always" && nexora.defaultAction === "none")
    throw new ConfigError(
      'nexora.track = "always" contradicts nexora.default_action = "none"; use track = "never" to turn tracking off',
    );
  return nexora;
}

function resolveArchitect(
  table: Table,
  roles: readonly ResolvedRole[],
  hosts: ReadonlyMap<string, ResolvedHost>,
): ResolvedArchitect {
  rejectUnknownKeys(
    table,
    [
      "enabled",
      "role",
      "plan_review",
      "reviewer_role",
      "max_packages",
      "count_toward_worker_limit",
      "high_risk_triggers",
    ],
    "architect",
  );
  const role =
    optionalString(table.role, "architect.role", 32) ?? DEFAULT_ARCHITECT_ROLE;
  if (!NAME_PATTERN.test(role))
    throw new ConfigError(`architect.role must match ${NAME_PATTERN.source}`);
  const reviewerRole = optionalString(
    table.reviewer_role,
    "architect.reviewer_role",
    32,
  );
  if (reviewerRole !== null && !NAME_PATTERN.test(reviewerRole))
    throw new ConfigError(
      `architect.reviewer_role must match ${NAME_PATTERN.source}`,
    );
  const architect: ResolvedArchitect = {
    enabled: optionalBoolean(table.enabled, "architect.enabled", false),
    role,
    planReview:
      table.plan_review === undefined
        ? "high_risk"
        : enumValue(
            table.plan_review,
            "architect.plan_review",
            PLAN_REVIEW_MODES,
          ),
    reviewerRole,
    maxPackages: optionalInteger(
      table.max_packages,
      "architect.max_packages",
      1,
      MAX_ARCHITECT_PACKAGES,
      DEFAULT_ARCHITECT_MAX_PACKAGES,
    ),
    countTowardWorkerLimit: optionalBoolean(
      table.count_toward_worker_limit,
      "architect.count_toward_worker_limit",
      false,
    ),
    highRiskTriggers:
      table.high_risk_triggers === undefined
        ? DEFAULT_HIGH_RISK_TRIGGERS
        : stringList(table.high_risk_triggers, "architect.high_risk_triggers"),
  };
  if (!architect.enabled) return architect;

  const architectRole = roles.find((candidate) => candidate.name === role);
  if (architectRole === undefined)
    throw new ConfigError(
      `architect.role "${role}" does not name a configured role`,
    );
  if (architectRole.kind !== "Developer")
    throw new ConfigError(
      `architect.role "${role}" must be a Developer role; roles.${role}.kind is ${architectRole.kind}`,
    );
  const host = hosts.get(architectRole.host);
  if (host?.kind !== "claude")
    throw new ConfigError(
      `roles.${role}: the architect role needs a claude host so its deny rules are enforced; host ${architectRole.host} is ${host?.kind ?? "unknown"}`,
    );
  if (reviewerRole !== null) {
    const reviewer = roles.find((candidate) => candidate.name === reviewerRole);
    if (reviewer === undefined)
      throw new ConfigError(
        `architect.reviewer_role "${reviewerRole}" does not name a configured role`,
      );
    if (reviewer.kind !== "Verifier")
      throw new ConfigError(
        `architect.reviewer_role "${reviewerRole}" must be a Verifier role; roles.${reviewerRole}.kind is ${reviewer.kind}`,
      );
  }
  return architect;
}

function operatorRules(value: unknown, at: string): string[] {
  const rules = stringList(value, at);
  rules.forEach((rule, index) => {
    const problem = autoApproveRuleProblem(rule);
    if (problem !== null) throw new ConfigError(`${at}[${index}] ${problem}`);
  });
  return rules;
}

function resolveOperator(
  table: Table,
  configured: boolean,
  roles: readonly ResolvedRole[],
  hosts: ReadonlyMap<string, ResolvedHost>,
  architect: ResolvedArchitect,
): ResolvedOperator {
  rejectUnknownKeys(
    table,
    [
      "enabled",
      "role",
      "auto_approve",
      "auto_approve_prefix",
      "timeout_seconds",
      "max_timeout_seconds",
      "output_tail_bytes",
      "proposal_ttl_minutes",
      "approval_ttl_minutes",
      "max_pending_proposals",
      "count_toward_worker_limit",
      "restart_health_timeout_seconds",
      "restart_idle_wait_seconds",
      "session_grant_max_minutes",
      "full_auto_default_minutes",
      "full_auto_max_minutes",
    ],
    "operator",
  );
  const role =
    optionalString(table.role, "operator.role", 32) ?? DEFAULT_OPERATOR_ROLE;
  if (!NAME_PATTERN.test(role))
    throw new ConfigError(`operator.role must match ${NAME_PATTERN.source}`);
  const operator: ResolvedOperator = {
    configured,
    enabled: optionalBoolean(table.enabled, "operator.enabled", false),
    role,
    autoApprove: operatorRules(table.auto_approve, "operator.auto_approve"),
    autoApprovePrefix: operatorRules(
      table.auto_approve_prefix,
      "operator.auto_approve_prefix",
    ),
    timeoutSeconds: optionalInteger(
      table.timeout_seconds,
      "operator.timeout_seconds",
      1,
      OPERATOR_HARD_MAX_TIMEOUT_SECONDS,
      300,
    ),
    maxTimeoutSeconds: optionalInteger(
      table.max_timeout_seconds,
      "operator.max_timeout_seconds",
      1,
      OPERATOR_HARD_MAX_TIMEOUT_SECONDS,
      1800,
    ),
    outputTailBytes: optionalInteger(
      table.output_tail_bytes,
      "operator.output_tail_bytes",
      1,
      MAX_OPERATOR_OUTPUT_TAIL_BYTES,
      8192,
    ),
    proposalTtlMinutes: optionalInteger(
      table.proposal_ttl_minutes,
      "operator.proposal_ttl_minutes",
      1,
      1440,
      60,
    ),
    approvalTtlMinutes: optionalInteger(
      table.approval_ttl_minutes,
      "operator.approval_ttl_minutes",
      1,
      120,
      10,
    ),
    maxPendingProposals: optionalInteger(
      table.max_pending_proposals,
      "operator.max_pending_proposals",
      1,
      50,
      5,
    ),
    countTowardWorkerLimit: optionalBoolean(
      table.count_toward_worker_limit,
      "operator.count_toward_worker_limit",
      false,
    ),
    restartHealthTimeoutSeconds: optionalInteger(
      table.restart_health_timeout_seconds,
      "operator.restart_health_timeout_seconds",
      1,
      600,
      60,
    ),
    restartIdleWaitSeconds: optionalInteger(
      table.restart_idle_wait_seconds,
      "operator.restart_idle_wait_seconds",
      0,
      3600,
      120,
    ),
    sessionGrantMaxMinutes: optionalInteger(
      table.session_grant_max_minutes,
      "operator.session_grant_max_minutes",
      1,
      OPERATOR_HARD_MAX_SESSION_MINUTES,
      60,
    ),
    fullAutoDefaultMinutes: optionalInteger(
      table.full_auto_default_minutes,
      "operator.full_auto_default_minutes",
      1,
      OPERATOR_HARD_MAX_SESSION_MINUTES,
      30,
    ),
    fullAutoMaxMinutes: optionalInteger(
      table.full_auto_max_minutes,
      "operator.full_auto_max_minutes",
      1,
      OPERATOR_HARD_MAX_SESSION_MINUTES,
      120,
    ),
  };
  if (operator.fullAutoDefaultMinutes > operator.fullAutoMaxMinutes)
    throw new ConfigError(
      `operator.full_auto_default_minutes (${operator.fullAutoDefaultMinutes}) must not exceed operator.full_auto_max_minutes (${operator.fullAutoMaxMinutes})`,
    );
  if (operator.timeoutSeconds > operator.maxTimeoutSeconds)
    throw new ConfigError(
      `operator.timeout_seconds (${operator.timeoutSeconds}) must not exceed operator.max_timeout_seconds (${operator.maxTimeoutSeconds})`,
    );
  if (!operator.enabled) return operator;

  if (role === architect.role)
    throw new ConfigError(
      `operator.role "${role}" must differ from architect.role`,
    );
  const operatorRole = roles.find((candidate) => candidate.name === role);
  if (operatorRole === undefined)
    throw new ConfigError(
      `operator.role "${role}" does not name a configured role`,
    );
  if (operatorRole.kind !== "Developer")
    throw new ConfigError(
      `operator.role "${role}" must be a Developer role; roles.${role}.kind is ${operatorRole.kind}`,
    );
  const host = hosts.get(operatorRole.host);
  if (host?.kind !== "claude")
    throw new ConfigError(
      `roles.${role}: the operator role needs a claude host so its deny rules are enforced; host ${operatorRole.host} is ${host?.kind ?? "unknown"}`,
    );
  operatorRole.allow.forEach((rule, index) => {
    if (!OPERATOR_ALLOW_RULE.test(rule))
      throw new ConfigError(
        `roles.${role}.allow[${index}] must be a Bash(cstan ...) rule; the operator role may not allow ${rule}`,
      );
  });
  for (const required of OPERATOR_REQUIRED_DENY)
    if (!operatorRole.deny.includes(required))
      throw new ConfigError(
        `roles.${role}.deny must include ${required}: the operator role may not read or edit project files or start subagents`,
      );
  return operator;
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

/**
 * Codex and OMP run unattended with full access (Codex's sandbox blocks the
 * daemon socket, and any approval prompt would leave the agent blocked), so a
 * rule this controller cannot enforce there is refused, not ignored.
 */
function rejectUnenforceable(
  at: string,
  host: ResolvedHost,
  kind: RoleKind,
  permissionMode: PermissionMode,
  allow: readonly string[],
  deny: readonly string[],
): void {
  if (kind === "PM" || kind === "Supervisor")
    throw new ConfigError(
      `${at}: a ${kind} role is read-only by design and only a claude host can enforce that; host ${host.name} is ${host.kind}`,
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

type RoleDefaults = {
  readonly model: string | null;
  readonly permissionMode: PermissionMode | null;
};

type ResolvedDefaults = {
  readonly all: RoleDefaults;
  readonly byKind: Readonly<Record<RoleKind, RoleDefaults>>;
};

function parseModel(value: unknown, at: string): string | null {
  const model = optionalString(value, at, 100);
  if (model !== null) {
    guardCredentialShape(model, at);
    if (model.startsWith("-"))
      throw new ConfigError(`${at} must not start with a dash`);
  }
  return model;
}

function parsePermissionMode(
  value: unknown,
  at: string,
): PermissionMode | null {
  return value === undefined ? null : enumValue(value, at, PERMISSION_MODES);
}

/** `[defaults]` and `[defaults.<kind>]`: the model and permission mode a role takes when it sets none of its own. */
function resolveDefaults(table: Table): ResolvedDefaults {
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

function resolveRoles(
  table: Table,
  hosts: ReadonlyMap<string, ResolvedHost>,
  projectRoot: string,
  defaults: ResolvedDefaults,
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
          : stringList(role.deny, `${at}.deny`);
    const hooks =
      role.hooks === undefined
        ? "off"
        : enumValue(role.hooks, `${at}.hooks`, ["off", "inherit"] as const);
    const { prompt, text: promptText } = resolvePrompt(role, at, projectRoot);
    if (host.kind !== "claude")
      rejectUnenforceable(at, host, kind, permissionMode, allow, deny);
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
