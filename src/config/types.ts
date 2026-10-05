/** The shapes, names and constants of a resolved capstan.toml. */

export const CONFIG_FILE_NAME = "capstan.toml";
export const DEFAULT_WAIT_TIMEOUT_SECONDS = 90;
export const MAX_WAIT_TIMEOUT_SECONDS = 3600;
export const DEFAULT_HERDR_SESSION = "default";

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

export const DEFAULT_RESEARCHER_ROLE = "researcher";
export const DEFAULT_RESEARCHER_OUTPUT_DIR = "docs/research";
export const DEFAULT_RESEARCHER_USER_AGENT =
  "capstan-researcher/1.0 (research bot; contact: project owner)";

export type ResolvedResearcher = {
  /** True only when `[researcher]` is present in the file; an absent table leaves every role an ordinary role, even one named "researcher". */
  readonly configured: boolean;
  /** Off: no researcher check runs and nothing about a Researcher reaches an agent. */
  readonly enabled: boolean;
  readonly role: string;
  /** Repo-relative directory the role may write to, with no `..` and no leading slash. */
  readonly outputDir: string;
  readonly userAgent: string;
};

export type ResolvedMcpServer = {
  readonly name: string;
  readonly command: string;
  readonly args: readonly string[];
};

export const MCP_SERVER_NAME_PATTERN = /^[a-z][a-z0-9_-]{0,31}$/;

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
  /** MCP servers the role gets; the resolver always sets it ([] when absent). Optional so literals that predate it still compile. */
  readonly mcp?: readonly ResolvedMcpServer[];
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
  /** Minutes a PM message may stay pending before the operator is told it is stale; the parser always sets it. */
  readonly pmStaleMinutes?: number;
};

export type ResolvedLimits = {
  /** The most worker agents (every agent except the PM) that may be active at once. */
  readonly maxWorkers: number;
};

export type ResolvedLedger = {
  /** Pre-migration ledger backups kept in the state directory; older ones are deleted. */
  readonly keepMigrationBackups: number;
};

export const DEFAULT_KEEP_MIGRATION_BACKUPS = 3;
export const MAX_KEEP_MIGRATION_BACKUPS = 50;

export const DEFAULT_MAX_WORKERS = 3;

export const DEFAULT_WORKTREE_SETUP_TIMEOUT_SECONDS = 600;
export const MAX_WORKTREE_SETUP_TIMEOUT_SECONDS = 3600;
export const MAX_WORKTREE_SETUP_CHARS = 1000;

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

export type ResolvedPromptRelay = {
  /** True only when `[prompt_relay]` is present in the file. */
  readonly present: boolean;
  /** Off: no prompt-relay text reaches any prompt, notice or status view. */
  readonly enabled: boolean;
  readonly captureTtlSeconds: number;
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
  /** The loader always sets it; optional so config literals that predate it still compile. */
  readonly researcher?: ResolvedResearcher;
  readonly mcpServers?: readonly ResolvedMcpServer[];
  readonly promptRelay: ResolvedPromptRelay;
  readonly nexora: ResolvedNexora;
  readonly limits: ResolvedLimits;
  readonly ledger: ResolvedLedger;
  readonly layout: ResolvedLayout;
  /** Absent when `[worktree]` sets no `setup`. */
  readonly worktree?: ResolvedWorktree;
  /** Things the loader accepted but the operator should know (an ignored key); `cstan config check` prints them. */
  readonly warnings: readonly string[];
  readonly env: ResolvedEnvironment;
  readonly hosts: readonly ResolvedHost[];
  readonly roles: readonly ResolvedRole[];
};

export const DEFAULT_SUPERVISION_CHECK_SECONDS = 300;
/** Minutes a PM message may stay pending before it counts as stale. */
export const DEFAULT_PM_STALE_MINUTES = 10;
