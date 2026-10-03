/**
 * The Herdr adapter: the only module that types into, starts, stops or closes
 * agents in Herdr. It refuses to type into a pane it did not create, into a PM
 * pane once its agent runs, and into any pane in the wrong phase. Herdr's agent
 * state is a hint; nothing here treats it as completion.
 */
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { randomUUID } from "node:crypto";
import {
  HOST_KINDS,
  type HostKind,
  type ResolvedRole,
} from "../config/capstan-config.js";
import {
  HERDR_STATES,
  type DeferralReason,
  type HerdrState,
} from "../controller/messaging.js";
import { herdrAgentName } from "./naming.js";
import { HerdrError, failureOf, runJson, type HerdrRunner } from "./runner.js";
import {
  checkAnswer,
  promptHash,
  relayTextProblem,
  type CaptureOutcome,
  type CapturedPrompt,
  type PromptAnswer,
  type RelayOutcome,
  type RelayRefusal,
} from "./prompt-relay.js";
import {
  extractInputLine,
  freshPromptReady,
  parseHostPrompt,
  parseBlockingDialog,
  classifyInputBlocker,
  type InputBlocker,
  promptFooter,
  TEXT_FIELD_WORDING,
  parseTrustDialogOf,
  trustTexts,
  stripAnsi,
} from "./screen.js";

export const TESTED_HERDR_VERSION = "0.9.1";

export type PaneRole = "PM" | "worker";
export type PanePhase = "fresh" | "prepared" | "started" | "tainted";

export interface PaneEntry {
  readonly role: PaneRole;
  readonly phase: PanePhase;
  readonly kind: string;
  readonly agent?: string;
  readonly worktreePath?: string;
  readonly workspaceId?: string;
}

export interface PaneLayoutView {
  readonly tabId: string;
  readonly workspaceId: string;
  readonly zoomed: boolean;
  readonly panes: readonly {
    readonly paneId: string;
    readonly width: number;
    readonly height: number;
  }[];
}

export class AdapterError extends Error {
  override readonly name: string = "AdapterError";
}
/** A pane move failed and the pane cannot be found at its old id or at a new one. */
export class PaneLost extends AdapterError {
  override readonly name = "PaneLost";
}
export class UnknownPaneError extends AdapterError {
  override readonly name = "UnknownPaneError";
}
export class PmPaneError extends AdapterError {
  override readonly name = "PmPaneError";
}
export class PhaseError extends AdapterError {
  override readonly name = "PhaseError";
}
export class PaneGone extends AdapterError {
  override readonly name = "PaneGone";
}
export class AgentPaneMismatch extends AdapterError {
  override readonly name = "AgentPaneMismatch";
}
export class DeferralNotElapsed extends AdapterError {
  override readonly name = "DeferralNotElapsed";
}
export class NotIdle extends AdapterError {
  override readonly name = "NotIdle";
}
export class NotBlocked extends AdapterError {
  override readonly name = "NotBlocked";
}
export class InputUnreadable extends AdapterError {
  override readonly name = "InputUnreadable";
  /** What keeps the input line from being read, from the screen that was read. */
  readonly blocker: InputBlocker;
  constructor(
    message: string,
    blocker: InputBlocker = "unknown",
    options?: { cause?: unknown },
  ) {
    super(message, options);
    this.blocker = blocker;
  }
}
export class ClearFailed extends AdapterError {
  override readonly name = "ClearFailed";
}
export class PromptUnrecognized extends AdapterError {
  override readonly name = "PromptUnrecognized";
}
export class ShellNotReady extends AdapterError {
  override readonly name = "ShellNotReady";
}
export class DialogStillOpen extends AdapterError {
  override readonly name = "DialogStillOpen";
}
export class UnsupportedHostError extends AdapterError {
  override readonly name = "UnsupportedHostError";
}
export class SendAfterRecordError extends AdapterError {
  override readonly name = "SendAfterRecordError";
}
export class InvalidArgumentError extends AdapterError {
  override readonly name = "InvalidArgumentError";
}

interface PromptRead {
  readonly prompt: CapturedPrompt;
  readonly selectedIndex: number;
  readonly options: CapturedPrompt["options"];
  readonly promptText: string;
  readonly footer: string | undefined;
}

export interface KeyLogEntry {
  readonly kind: "key";
  readonly pane: string;
  readonly key: string;
  readonly reason: string;
}

export type KeyLogger = (entry: KeyLogEntry) => void | Promise<void>;

export type SendOutcome =
  | { readonly sent: true }
  | {
      readonly sent: false;
      readonly reason: DeferralReason;
      readonly detail?: string;
      /** Set with INPUT_UNREADABLE_DETAIL: what is on the screen instead of the input line. */
      readonly blocker?: InputBlocker;
    };

export type DialogOutcome =
  | { readonly handled: true; readonly keys: readonly string[] }
  | { readonly handled: false; readonly reason: string };

export interface AdapterOptions {
  readonly run: HerdrRunner;
  readonly tempRoot?: string;
  readonly pollMs?: number;
  readonly sleep?: (ms: number) => Promise<void>;
  readonly now?: () => number;
  /** The project's slug: Herdr then sees every agent as `<slug>-<agent-id>`, while callers keep passing the ledger id. */
  readonly projectSlug?: string;
}

const MAX_CLEAR_ROUNDS = 5;
const SELECTION_REDRAW_MS = 3_000;
const AGENT_START_MARGIN_MS = 10_000;
export const MAX_TEXT_BYTES = 16 * 1024;
const NAME_PATTERN = /^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$/;
const MAX_NOTIFICATION_TITLE_CHARS = 100;
const MAX_NOTIFICATION_BODY_CHARS = 500;

/** True for a name Herdr and this adapter accept for an agent. */
export function isAgentName(value: unknown): value is string {
  return typeof value === "string" && NAME_PATTERN.test(value);
}

/** The `detail` a deferral carries when the input line could not be read. */
export const INPUT_UNREADABLE_DETAIL = "the input line is unreadable";

/** Maps a Herdr agent status to a state the ledger accepts; anything unrecognized is `unknown`. */
export function herdrStateOf(status: string): HerdrState {
  return (HERDR_STATES as readonly string[]).includes(status)
    ? (status as HerdrState)
    : "unknown";
}
const BRANCH_PATTERN = /^[A-Za-z0-9][A-Za-z0-9._/-]{0,99}$/;
const WORKSPACE_PATTERN = /^w[0-9A-Za-z]+$/;
const PANE_PATTERN = /^w[0-9A-Za-z]+:p[0-9A-Za-z]+$/;
const TAB_PATTERN = /^w[0-9A-Za-z]+:t[0-9A-Za-z]+$/;
const SIMPLE_VALUE = /^[A-Za-z0-9_@%+=:,./-]*$/;
/** The same characters the controller refuses in a message body. */
const UNSAFE_TEXT = /[\p{Cc}\p{Cf}\p{Zl}\p{Zp}\p{Noncharacter_Code_Point}]/u;
/** HOME, PATH and TERM are shell-quoted, so only blank or unsafe-to-show text is refused. */
function isQuotableValue(value: unknown): value is string {
  return (
    typeof value === "string" && value.trim() !== "" && !UNSAFE_TEXT.test(value)
  );
}
const ALLOWED_TEXT_CHARACTERS = /[\n\t\u200c\u200d]/g;
/** In Claude Code a first character of / ! # ? or @ (or a tab) acts on the input box instead of adding text. */
const COMMAND_START = /^(?:\t|\s*[/!#?@])/;

function isSafeText(text: unknown): text is string {
  return (
    typeof text === "string" &&
    text.trim() !== "" &&
    text.isWellFormed() &&
    !UNSAFE_TEXT.test(text.replace(ALLOWED_TEXT_CHARACTERS, ""))
  );
}
const ENVIRONMENT_KEY = /^[A-Z_][A-Z0-9_]*$/;
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
const CONTROL_CHARACTERS = /\p{Cc}/u;

function requireMatch(value: unknown, pattern: RegExp, label: string): string {
  if (typeof value !== "string" || !pattern.test(value))
    throw new InvalidArgumentError(`${label} is not acceptable`);
  return value;
}

/** A display label: printable text on one line, at most 64 characters. */
function requireLabel(value: unknown): string {
  if (
    typeof value !== "string" ||
    value.trim() === "" ||
    value.length > 128 ||
    Array.from(value).length > 64 ||
    !value.isWellFormed() ||
    UNSAFE_TEXT.test(value)
  )
    throw new InvalidArgumentError("label is not acceptable");
  return value;
}

function requireQuotable(value: unknown, label: string): string {
  if (!isQuotableValue(value))
    throw new InvalidArgumentError(`${label} is not acceptable`);
  return value;
}

export function shellQuote(value: string): string {
  return `'${value.replaceAll("'", "'\\''")}'`;
}

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
  if (role.hooks === "off") args.push("--settings", '{"disableAllHooks":true}');
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

function deferralFor(status: string): DeferralReason | undefined {
  if (status === "idle" || status === "done") return undefined;
  return status === "blocked" ? "agent_blocked" : "agent_busy";
}

export class HerdrAdapter {
  readonly #run: HerdrRunner;
  readonly #tempRoot: string;
  readonly #pollMs: number;
  readonly #sleep: (ms: number) => Promise<void>;
  readonly #now: () => number;
  readonly #panes = new Map<string, PaneEntry>();
  readonly #slug: string | undefined;
  #promptDirectory: string | undefined;

  constructor(options: AdapterOptions) {
    this.#run = options.run;
    this.#slug = options.projectSlug;
    this.#tempRoot = options.tempRoot ?? os.tmpdir();
    this.#pollMs = options.pollMs ?? 100;
    this.#sleep =
      options.sleep ??
      ((ms) => new Promise((resolve) => setTimeout(resolve, ms)));
    this.#now = options.now ?? Date.now;
  }

  /** The name Herdr knows an agent by: the ledger id behind the project's slug. */
  #herdrName(agentId: string): string {
    try {
      return this.#slug === undefined
        ? agentId
        : herdrAgentName(this.#slug, agentId);
    } catch (error) {
      throw new InvalidArgumentError(
        error instanceof Error ? error.message : "agent name is not acceptable",
      );
    }
  }

  async version(): Promise<string> {
    const outcome = await this.#run(["--version"]);
    if (outcome.code !== 0)
      throw new HerdrError("version", "herdr --version failed");
    return outcome.stdout.trim();
  }

  paneEntry(paneId: string): PaneEntry | undefined {
    const entry = this.#panes.get(paneId);
    return entry === undefined ? undefined : { ...entry };
  }

  /** Removes the adapter's temporary files, prompt files included, so call it only after every agent that reads one has started. */
  close(): void {
    if (this.#promptDirectory !== undefined)
      fs.rmSync(this.#promptDirectory, { recursive: true, force: true });
    this.#promptDirectory = undefined;
  }

  async createWorktree(input: {
    workspaceId: string;
    branch: string;
    label: string;
    base?: string;
  }): Promise<{
    workspaceId: string;
    paneId: string;
    path: string;
    branch: string;
  }> {
    requireMatch(input.workspaceId, WORKSPACE_PATTERN, "workspace id");
    requireMatch(input.branch, BRANCH_PATTERN, "branch");
    requireLabel(input.label);
    const args = [
      "worktree",
      "create",
      "--workspace",
      input.workspaceId,
      "--branch",
      input.branch,
      "--label",
      input.label,
    ];
    if (input.base !== undefined)
      args.push("--base", requireMatch(input.base, BRANCH_PATTERN, "base"));
    args.push("--no-focus");
    const result = await runJson(this.#run, args);
    const pane = this.#record(result.root_pane, "root_pane");
    const workspace = this.#record(result.workspace, "workspace");
    const worktree = this.#record(workspace.worktree, "worktree");
    const paneId = requireMatch(pane.pane_id, PANE_PATTERN, "pane id");
    const workspaceId = requireMatch(
      workspace.workspace_id,
      WORKSPACE_PATTERN,
      "workspace id",
    );
    const checkout = worktree.checkout_path;
    if (typeof checkout !== "string" || !path.isAbsolute(checkout))
      throw new HerdrError(
        "bad_output",
        "herdr did not report a checkout path",
      );
    this.#panes.set(paneId, {
      role: "worker",
      phase: "fresh",
      kind: "shell",
      worktreePath: checkout,
      workspaceId,
    });
    return { workspaceId, paneId, path: checkout, branch: input.branch };
  }

  /** The panes of the tab that holds `paneId`, with their sizes in terminal cells. */
  async paneLayout(paneId: string): Promise<PaneLayoutView> {
    requireMatch(paneId, PANE_PATTERN, "pane id");
    const result = await runJson(this.#run, [
      "pane",
      "layout",
      "--pane",
      paneId,
    ]);
    const layout = this.#record(result.layout, "layout");
    if (!Array.isArray(layout.panes))
      throw new HerdrError("bad_output", "herdr did not report layout panes");
    return {
      tabId: requireMatch(layout.tab_id, TAB_PATTERN, "tab id"),
      workspaceId: requireMatch(
        layout.workspace_id,
        WORKSPACE_PATTERN,
        "workspace id",
      ),
      // Anything but an explicit false counts as zoomed: no split on a layout we cannot read.
      zoomed: layout.zoomed !== false,
      panes: layout.panes.map((entry) => {
        const pane = this.#record(entry, "layout pane");
        const rect = this.#record(pane.rect, "layout rect");
        return {
          paneId: requireMatch(pane.pane_id, PANE_PATTERN, "pane id"),
          width: typeof rect.width === "number" ? rect.width : Number.NaN,
          height: typeof rect.height === "number" ? rect.height : Number.NaN,
        };
      }),
    };
  }

  async #listPanes(): Promise<
    Array<{
      paneId: string;
      tabId: string;
      workspaceId: string;
      /** undefined when Herdr gave no absolute directory for the pane. */
      cwd: string | undefined;
    }>
  > {
    const result = await runJson(this.#run, ["pane", "list"]);
    if (!Array.isArray(result.panes))
      throw new HerdrError("bad_output", "herdr did not report panes");
    // A pane whose id this adapter cannot read is skipped: it cannot be ours.
    // A pane without a usable directory is kept, so it still counts as existing.
    return result.panes.flatMap((entry) => {
      try {
        const pane = this.#record(entry, "pane");
        return [
          {
            paneId: requireMatch(pane.pane_id, PANE_PATTERN, "pane id"),
            tabId: requireMatch(pane.tab_id, TAB_PATTERN, "tab id"),
            workspaceId: requireMatch(
              pane.workspace_id,
              WORKSPACE_PATTERN,
              "workspace id",
            ),
            cwd:
              typeof pane.cwd === "string" && path.isAbsolute(pane.cwd)
                ? pane.cwd
                : undefined,
          },
        ];
      } catch {
        return [];
      }
    });
  }

  /** The panes whose working directory is exactly `directory` (compared as real paths), with their workspaces. */
  async panesAtPath(
    directory: string,
  ): Promise<Array<{ paneId: string; workspaceId: string }>> {
    if (!path.isAbsolute(directory)) return [];
    const wanted = this.#canonical(directory);
    return (await this.#listPanes())
      .filter(
        (pane) =>
          pane.cwd !== undefined && this.#canonical(pane.cwd) === wanted,
      )
      .map((pane) => ({ paneId: pane.paneId, workspaceId: pane.workspaceId }));
  }

  /**
   * Moves a registered worker pane into another tab as a split. Herdr gives
   * the moved pane a new id; the registry follows it. A move that errors is
   * ambiguous (Herdr may have done it), so the panes are listed again: the old
   * pane still there means nothing moved; exactly one new pane at the worktree
   * path means it moved; anything else is an error.
   */
  async placePane(input: {
    paneId: string;
    tabId: string;
    targetPaneId: string;
    direction: "right" | "down";
    /** The fraction of the target pane the target keeps. */
    keep: number;
    worktreePath: string;
  }): Promise<{ paneId: string; workspaceId: string }> {
    requireMatch(input.paneId, PANE_PATTERN, "pane id");
    requireMatch(input.tabId, TAB_PATTERN, "tab id");
    requireMatch(input.targetPaneId, PANE_PATTERN, "target pane id");
    if (!path.isAbsolute(input.worktreePath))
      throw new InvalidArgumentError("worktree path must be absolute");
    if (input.direction !== "right" && input.direction !== "down")
      throw new InvalidArgumentError("split direction is not acceptable");
    if (!(input.keep >= 0.1 && input.keep <= 0.9))
      throw new InvalidArgumentError("split ratio is not acceptable");
    const entry = this.#panes.get(input.paneId);
    if (entry === undefined)
      throw new UnknownPaneError("pane is not registered");
    if (entry.role !== "worker")
      throw new PhaseError("only a worker pane can be placed");
    const before = new Set((await this.#listPanes()).map((p) => p.paneId));
    let moved: { paneId: string; workspaceId: string };
    try {
      const result = await runJson(this.#run, [
        "pane",
        "move",
        input.paneId,
        "--tab",
        input.tabId,
        "--split",
        input.direction,
        "--target-pane",
        input.targetPaneId,
        "--ratio",
        String(input.keep),
        "--no-focus",
      ]);
      const pane = this.#record(
        this.#record(result.move_result, "move_result").pane,
        "pane",
      );
      moved = {
        paneId: requireMatch(pane.pane_id, PANE_PATTERN, "pane id"),
        workspaceId: requireMatch(
          pane.workspace_id,
          WORKSPACE_PATTERN,
          "workspace id",
        ),
      };
    } catch (error) {
      const after = await this.#listPanes().catch(() => undefined);
      if (after === undefined)
        throw new PaneLost(
          "the pane move failed and the panes could not be listed to check it",
        );
      if (after.some((p) => p.paneId === input.paneId)) throw error;
      const wanted = this.#canonical(input.worktreePath);
      const found = after.filter(
        (p) =>
          !before.has(p.paneId) &&
          p.cwd !== undefined &&
          this.#canonical(p.cwd) === wanted,
      );
      if (found.length !== 1)
        throw new PaneLost(
          "the pane move failed and the pane cannot be found again",
        );
      moved = { paneId: found[0]!.paneId, workspaceId: found[0]!.workspaceId };
    }
    this.#panes.delete(input.paneId);
    this.#panes.set(moved.paneId, { ...entry, workspaceId: moved.workspaceId });
    return moved;
  }

  async createWorkspace(input: {
    cwd: string;
    label: string;
    role: PaneRole;
  }): Promise<{ workspaceId: string; paneId: string; tabId: string }> {
    if (!path.isAbsolute(input.cwd))
      throw new InvalidArgumentError("workspace directory must be absolute");
    requireLabel(input.label);
    const result = await runJson(this.#run, [
      "workspace",
      "create",
      "--cwd",
      input.cwd,
      "--label",
      input.label,
      "--no-focus",
    ]);
    const pane = this.#record(result.root_pane, "root_pane");
    const workspace = this.#record(result.workspace, "workspace");
    const paneId = requireMatch(pane.pane_id, PANE_PATTERN, "pane id");
    const workspaceId = requireMatch(
      workspace.workspace_id,
      WORKSPACE_PATTERN,
      "workspace id",
    );
    const tabId = requireMatch(
      this.#record(result.tab, "tab").tab_id,
      TAB_PATTERN,
      "tab id",
    );
    this.#panes.set(paneId, {
      role: input.role,
      phase: "fresh",
      kind: "shell",
      workspaceId,
    });
    return { workspaceId, paneId, tabId };
  }

  /** A new tab with its own root pane inside an existing workspace. */
  async createTab(input: {
    workspaceId: string;
    cwd: string;
    label: string;
    role: PaneRole;
  }): Promise<{ tabId: string; paneId: string }> {
    requireMatch(input.workspaceId, WORKSPACE_PATTERN, "workspace id");
    if (!path.isAbsolute(input.cwd))
      throw new InvalidArgumentError("tab directory must be absolute");
    requireLabel(input.label);
    const result = await runJson(this.#run, [
      "tab",
      "create",
      "--workspace",
      input.workspaceId,
      "--cwd",
      input.cwd,
      "--label",
      input.label,
      "--no-focus",
    ]);
    const pane = this.#record(result.root_pane, "root_pane");
    const tab = this.#record(result.tab, "tab");
    const paneId = requireMatch(pane.pane_id, PANE_PATTERN, "pane id");
    const tabId = requireMatch(tab.tab_id, TAB_PATTERN, "tab id");
    this.#panes.set(paneId, {
      role: input.role,
      phase: "fresh",
      kind: "shell",
      workspaceId: input.workspaceId,
    });
    return { tabId, paneId };
  }

  async removeWorktree(
    workspaceId: string,
    options: { force?: boolean } = {},
  ): Promise<void> {
    requireMatch(workspaceId, WORKSPACE_PATTERN, "workspace id");
    // A worker placed as a pane shares the PM's workspace id; removing "its" worktree workspace would reach the PM.
    for (const entry of this.#panes.values())
      if (entry.workspaceId === workspaceId && entry.role === "PM")
        throw new PhaseError("that workspace holds the PM");
    await runJson(this.#run, [
      "worktree",
      "remove",
      "--workspace",
      workspaceId,
      ...(options.force ? ["--force"] : []),
    ]);
    for (const [paneId, entry] of this.#panes)
      if (entry.workspaceId === workspaceId && entry.role === "worker")
        this.#panes.delete(paneId);
  }

  /** Display-only metadata for the operator's sidebar; the caller treats a failure as non-fatal. */
  async reportMetadata(
    target: { readonly paneId: string } | { readonly workspaceId: string },
    tokens: Readonly<Record<string, string>>,
  ): Promise<void> {
    const pane = "paneId" in target;
    const id = pane
      ? requireMatch(target.paneId, PANE_PATTERN, "pane id")
      : requireMatch(target.workspaceId, WORKSPACE_PATTERN, "workspace id");
    const args = [pane ? "pane" : "workspace", "report-metadata", id];
    args.push("--source", "capstan");
    for (const [name, value] of Object.entries(tokens)) {
      requireMatch(name, /^[a-z][a-z0-9_]{0,31}$/, "token name");
      args.push("--token", `${name}=${requireLabel(value)}`);
    }
    await this.#runChecked(args);
  }

  /** Names a tab; display only. */
  async renameTab(tabId: string, label: string): Promise<void> {
    requireMatch(tabId, TAB_PATTERN, "tab id");
    await this.#runChecked(["tab", "rename", tabId, requireLabel(label)]);
  }

  /** Gives an existing workspace its current label (an upgraded project still holds the old one). */
  async renameWorkspace(workspaceId: string, label: string): Promise<void> {
    requireMatch(workspaceId, WORKSPACE_PATTERN, "workspace id");
    await this.#runChecked([
      "workspace",
      "rename",
      workspaceId,
      requireLabel(label),
    ]);
  }

  async closePane(paneId: string): Promise<void> {
    requireMatch(paneId, PANE_PATTERN, "pane id");
    await this.#runChecked(["pane", "close", paneId]);
    this.#panes.delete(paneId);
  }

  /**
   * Registers a pane an earlier adapter instance created (a daemon that
   * restarted), after Herdr confirms the pane exists and that the agent name
   * still points at it. Nothing is registered otherwise.
   */
  async adoptPane(input: {
    paneId: string;
    role: PaneRole;
    agent: string;
    workspaceId: string | null;
    worktreePath: string | null;
  }): Promise<void> {
    requireMatch(input.paneId, PANE_PATTERN, "pane id");
    if (!isAgentName(input.agent))
      throw new InvalidArgumentError("agent name is not acceptable");
    if (this.#panes.has(input.paneId))
      throw new PhaseError("the pane is already registered");
    await this.#assertPaneExists(input.paneId);
    let state: { status: string; paneId: string; kind: string };
    try {
      state = await this.agentState(input.agent);
    } catch (error) {
      if (
        this.#slug === undefined ||
        !(error instanceof HerdrError) ||
        error.code !== "agent_not_found"
      )
        throw error;
      // An agent started before names carried the project is still known by its bare id: name it the new way when it is on the recorded pane.
      const legacy = await this.#agentStateByHerdrName(input.agent);
      if (legacy.paneId !== input.paneId)
        throw new AgentPaneMismatch(
          "the agent name points at another pane than the recorded one",
        );
      await this.#runChecked([
        "agent",
        "rename",
        input.paneId,
        this.#herdrName(input.agent),
      ]);
      state = await this.agentState(input.agent);
    }
    if (state.paneId !== input.paneId)
      throw new AgentPaneMismatch(
        "the agent name points at another pane than the recorded one",
      );
    if (!(HOST_KINDS as readonly string[]).includes(state.kind))
      throw new UnsupportedHostError(
        `the agent is a ${state.kind} agent, which this adapter does not drive`,
      );
    this.#panes.set(input.paneId, {
      role: input.role,
      phase: "started",
      kind: state.kind as HostKind,
      agent: input.agent,
      ...(input.workspaceId === null ? {} : { workspaceId: input.workspaceId }),
      ...(input.worktreePath === null
        ? {}
        : { worktreePath: input.worktreePath }),
    });
  }

  /** Registers the fallback watch pane of an earlier instance; it never takes input from the adapter again. */
  async adoptShellPane(
    paneId: string,
    workspaceId: string | null,
  ): Promise<void> {
    requireMatch(paneId, PANE_PATTERN, "pane id");
    await this.#assertPaneExists(paneId);
    this.#panes.set(paneId, {
      role: "worker",
      phase: "started",
      kind: "shell",
      ...(workspaceId === null ? {} : { workspaceId }),
    });
  }

  /** Runs one command in a shell pane the adapter just created (fresh or prepared), then retires the pane from input. */
  async runInPane(paneId: string, command: string): Promise<void> {
    requireMatch(paneId, PANE_PATTERN, "pane id");
    const entry = this.#panes.get(paneId);
    if (entry === undefined)
      throw new UnknownPaneError("the pane was not created by this adapter");
    if (
      entry.role !== "worker" ||
      entry.kind !== "shell" ||
      (entry.phase !== "fresh" && entry.phase !== "prepared")
    )
      throw new PhaseError(
        "only a fresh or prepared shell pane runs a command",
      );
    if (
      typeof command !== "string" ||
      command.trim() === "" ||
      command.length > 2000 ||
      !command.isWellFormed() ||
      CONTROL_CHARACTERS.test(command)
    )
      throw new InvalidArgumentError("the command is not acceptable");
    await this.#waitForFreshPrompt(paneId);
    await this.#runChecked(["pane", "run", paneId, command]);
    this.#panes.set(paneId, { ...entry, phase: "started" });
  }

  async #assertPaneExists(paneId: string): Promise<void> {
    try {
      await runJson(this.#run, ["pane", "get", paneId]);
    } catch (error) {
      if (error instanceof HerdrError && /not_found|no_such/.test(error.code))
        throw new PaneGone("Herdr has no such pane");
      throw error;
    }
  }

  /** Drops a pane from the registry without touching Herdr, for a pane that could not be closed and must no longer block its agent name. */
  forgetPane(paneId: string): void {
    this.#panes.delete(paneId);
  }

  /** The pane the adapter registered for an agent, if any. */
  paneForAgent(agentId: string): string | undefined {
    for (const [paneId, entry] of this.#panes)
      if (entry.agent === agentId) return paneId;
    return undefined;
  }

  /** The mapped Herdr state of an agent on its registered pane; throws when Herdr shows it elsewhere. */
  async agentObservation(agentId: string): Promise<HerdrState> {
    const paneId = this.paneForAgent(agentId);
    if (paneId === undefined)
      throw new UnknownPaneError("no pane is registered for this agent");
    return herdrStateOf(await this.#stateFor(agentId, paneId));
  }

  /** Shows a notification in Herdr; the text is checked like any text that reaches the operator's screen. */
  async notify(title: string, body: string): Promise<void> {
    for (const [value, label, max] of [
      [title, "notification title", MAX_NOTIFICATION_TITLE_CHARS],
      [body, "notification body", MAX_NOTIFICATION_BODY_CHARS],
    ] as const)
      if (
        typeof value !== "string" ||
        value.trim() === "" ||
        value.length > max ||
        !value.isWellFormed() ||
        UNSAFE_TEXT.test(value)
      )
        throw new InvalidArgumentError(`${label} is not acceptable`);
    const result = await runJson(this.#run, [
      "notification",
      "show",
      title,
      "--body",
      body,
      "--sound",
      "request",
    ]);
    // Herdr answers exit 0 with shown:false when notifications are off, so an
    // accepted command alone does not mean the operator saw anything.
    if (result.shown !== true)
      throw new HerdrError(
        "notification_not_shown",
        `Herdr did not show the notification${typeof result.reason === "string" ? `: ${result.reason.replace(/[^A-Za-z0-9 _-]/g, "").slice(0, 60)}` : ""}`,
      );
  }

  async paneState(paneId: string): Promise<{ status: string; agent?: string }> {
    requireMatch(paneId, PANE_PATTERN, "pane id");
    const result = await runJson(this.#run, ["pane", "get", paneId]);
    const pane = this.#record(result.pane, "pane");
    const status =
      typeof pane.agent_status === "string" ? pane.agent_status : "unknown";
    return typeof pane.agent === "string"
      ? { status, agent: pane.agent }
      : { status };
  }

  async agentState(
    name: string,
  ): Promise<{ status: string; paneId: string; kind: string }> {
    requireMatch(name, NAME_PATTERN, "agent name");
    return await this.#agentStateByHerdrName(this.#herdrName(name));
  }

  async #agentStateByHerdrName(
    herdrName: string,
  ): Promise<{ status: string; paneId: string; kind: string }> {
    const result = await runJson(this.#run, ["agent", "get", herdrName]);
    const agent = this.#record(result.agent, "agent");
    return {
      status:
        typeof agent.agent_status === "string" ? agent.agent_status : "unknown",
      paneId: typeof agent.pane_id === "string" ? agent.pane_id : "",
      kind: typeof agent.agent === "string" ? agent.agent : "unknown",
    };
  }

  async readScreen(
    paneId: string,
    options: { ansi?: boolean; lines?: number } = {},
  ): Promise<string> {
    requireMatch(paneId, PANE_PATTERN, "pane id");
    const outcome = await this.#run([
      "pane",
      "read",
      paneId,
      "--source",
      "visible",
      "--lines",
      String(options.lines ?? 80),
      ...(options.ansi ? ["--ansi"] : []),
    ]);
    if (outcome.code !== 0) throw failureOf(["pane", "read"], outcome);
    return outcome.stdout;
  }

  async readInput(paneId: string): Promise<string | undefined> {
    return (await this.#readInputAndBlocker(paneId)).text;
  }

  /** The input line and, from the same screen read, what blocks it when it is unreadable. */
  async #readInputAndBlocker(
    paneId: string,
  ): Promise<{ text: string | undefined; blocker: InputBlocker }> {
    const entry = this.#panes.get(paneId);
    if (entry === undefined)
      throw new UnknownPaneError("pane is not registered");
    const screen = await this.readScreen(paneId, { ansi: true });
    const text = extractInputLine(entry.kind, screen);
    return {
      text,
      blocker:
        text === undefined
          ? classifyInputBlocker(entry.kind, screen)
          : "unknown",
    };
  }

  async prepareShell(input: {
    paneId: string;
    environment: Readonly<Record<string, string>>;
    timeoutMs?: number;
  }): Promise<void> {
    const entry = this.#assertTypable(input.paneId, "prepare");
    if (entry.phase !== "fresh")
      throw new PhaseError("only a fresh pane can be prepared");
    const environment = input.environment;
    const home = requireQuotable(environment.HOME, "HOME");
    if (!home.startsWith("/"))
      throw new InvalidArgumentError("HOME is not acceptable");
    const pathValue = requireQuotable(environment.PATH, "PATH");
    const term = requireQuotable(environment.TERM, "TERM");
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
          `environment value for ${name} is not acceptable`,
        );
    }
    await this.#waitForFreshPrompt(input.paneId);

    const directory = fs.mkdtempSync(
      path.join(this.#tempRoot, "capstan-shell-"),
    );
    try {
      fs.chmodSync(directory, 0o700);
      const envFile = path.join(directory, "env");
      const rcFile = path.join(directory, "rc");
      for (const file of [directory, envFile, rcFile])
        requireMatch(file, SIMPLE_VALUE, "temporary path");
      fs.writeFileSync(
        envFile,
        `${Object.entries(environment)
          .map(([name, value]) => `export ${name}=${shellQuote(value)}`)
          .join("\n")}\n`,
        { mode: 0o600, flag: "wx" },
      );
      fs.writeFileSync(
        rcFile,
        `. '${envFile}'\nPS1='❯ '\nrm -rf '${directory}'\n`,
        { mode: 0o600, flag: "wx" },
      );
      this.#panes.set(input.paneId, {
        ...entry,
        phase: "prepared",
        kind: "shell",
      });
      try {
        await this.#runChecked([
          "pane",
          "run",
          input.paneId,
          `exec env -i HOME=${shellQuote(home)} PATH=${shellQuote(pathValue)} TERM=${shellQuote(term)} bash --noprofile --rcfile '${rcFile}' -i`,
        ]);
      } catch (error) {
        this.#panes.set(input.paneId, { ...entry, phase: "tainted" });
        throw error;
      }
      const deadline = this.#now() + (input.timeoutMs ?? 10_000);
      while (this.#now() < deadline) {
        if (!fs.existsSync(directory)) {
          const input_ = await this.readInput(input.paneId);
          if (input_ === "") return;
        }
        await this.#sleep(this.#pollMs);
      }
      this.#panes.set(input.paneId, { ...entry, phase: "tainted" });
      throw new ShellNotReady(
        "the prepared shell did not show its prompt in time",
      );
    } finally {
      fs.rmSync(directory, { recursive: true, force: true });
    }
  }

  writePromptFile(text: string): string {
    if (!isSafeText(text))
      throw new InvalidArgumentError("prompt text is not acceptable");
    this.#promptDirectory ??= fs.mkdtempSync(
      path.join(this.#tempRoot, "capstan-prompts-"),
    );
    fs.chmodSync(this.#promptDirectory, 0o700);
    const file = path.join(this.#promptDirectory, `${randomUUID()}.md`);
    fs.writeFileSync(file, text, { mode: 0o600, flag: "wx" });
    return file;
  }

  async startAgent(input: {
    name: string;
    kind: string;
    paneId: string;
    args: readonly string[];
    timeoutMs?: number;
    environment?: Readonly<Record<string, string>>;
  }): Promise<{ status: "started" | "blocked_at_startup" }> {
    if (!(HOST_KINDS as readonly string[]).includes(input.kind))
      throw new UnsupportedHostError(
        `agent kind ${input.kind} is not supported yet`,
      );
    requireMatch(input.name, NAME_PATTERN, "agent name");
    const herdrName = this.#herdrName(input.name);
    for (const other of this.#panes.values())
      if (other.agent === input.name)
        throw new InvalidArgumentError(
          "an agent with this name is already registered",
        );
    for (const arg of input.args)
      if (
        typeof arg !== "string" ||
        arg.length === 0 ||
        !arg.isWellFormed() ||
        CONTROL_CHARACTERS.test(arg)
      )
        throw new InvalidArgumentError(
          "an agent argument is empty or has control characters",
        );
    if (input.timeoutMs !== undefined && !Number.isFinite(input.timeoutMs))
      throw new InvalidArgumentError(
        "the start timeout must be a finite number",
      );
    const timeoutMs = Math.max(input.timeoutMs ?? 30_000, 5_000);
    let entry = this.#assertTypable(input.paneId, "start");
    if (entry.phase === "fresh" && input.environment === undefined)
      throw new InvalidArgumentError(
        "a fresh pane needs an environment so its shell starts clean",
      );
    if (entry.phase === "fresh" && input.environment !== undefined) {
      await this.prepareShell({
        paneId: input.paneId,
        environment: input.environment,
      });
      entry = this.#assertTypable(input.paneId, "start");
    } else if ((await this.readInput(input.paneId)) !== "") {
      this.#panes.set(input.paneId, { ...entry, phase: "tainted" });
      throw new PromptUnrecognized(
        "the prepared shell does not show an empty prompt",
      );
    }
    try {
      await runJson(
        this.#run,
        [
          "agent",
          "start",
          herdrName,
          "--kind",
          input.kind,
          "--pane",
          input.paneId,
          "--timeout",
          String(timeoutMs),
          "--",
          ...input.args,
        ],
        { timeoutMs: timeoutMs + AGENT_START_MARGIN_MS },
      );
    } catch (error) {
      if (error instanceof HerdrError && error.code === "agent_not_ready") {
        this.#panes.set(input.paneId, {
          ...entry,
          phase: "started",
          kind: input.kind,
          agent: input.name,
        });
        return { status: "blocked_at_startup" };
      }
      this.#panes.set(input.paneId, { ...entry, phase: "tainted" });
      throw error;
    }
    this.#panes.set(input.paneId, {
      ...entry,
      phase: "started",
      kind: input.kind,
      agent: input.name,
    });
    return { status: "started" };
  }

  async guardedSend(input: {
    paneId: string;
    text: string;
    beforeSend: () => void | Promise<void>;
  }): Promise<SendOutcome> {
    const entry = this.#assertTypable(input.paneId, "send");
    if (
      !isSafeText(input.text) ||
      COMMAND_START.test(input.text) ||
      Buffer.byteLength(input.text, "utf8") > MAX_TEXT_BYTES
    )
      throw new InvalidArgumentError("message text is not acceptable");
    const agent = entry.agent;
    if (agent === undefined) throw new PhaseError("the pane has no agent");
    const first = await this.#stateFor(agent, input.paneId);
    const busy = deferralFor(first);
    if (busy !== undefined) return { sent: false, reason: busy };
    const { text: typed, blocker } = await this.#readInputAndBlocker(
      input.paneId,
    );
    if (typed === undefined)
      return {
        sent: false,
        reason: "input_not_empty",
        detail: INPUT_UNREADABLE_DETAIL,
        blocker,
      };
    if (typed !== "") return { sent: false, reason: "input_not_empty" };
    const second = deferralFor(await this.#stateFor(agent, input.paneId));
    if (second !== undefined) return { sent: false, reason: second };
    await input.beforeSend();
    try {
      await runJson(this.#run, [
        "agent",
        "prompt",
        this.#herdrName(agent),
        input.text,
      ]);
    } catch (error) {
      throw new SendAfterRecordError(
        "the message was recorded but Herdr did not accept the prompt",
        { cause: error },
      );
    }
    return { sent: true };
  }

  /**
   * Types a short wake line into the PM's pane, which `guardedSend` never
   * does. Only a registered, started PM pane is typed into, and only when
   * Herdr reports it idle or done, its input line reads as empty, and both
   * hold again right before the keys. A line that cannot be read or has text
   * on it is never typed over.
   */
  async wakePm(input: {
    paneId: string;
    text: string;
    beforeSend: () => void | Promise<void>;
  }): Promise<
    | { readonly sent: true }
    | {
        readonly sent: false;
        readonly reason: "pm_not_idle" | "input_not_empty";
      }
  > {
    requireMatch(input.paneId, PANE_PATTERN, "pane id");
    const entry = this.#panes.get(input.paneId);
    if (entry === undefined)
      throw new UnknownPaneError("pane is not registered");
    if (entry.role !== "PM" || entry.phase !== "started")
      throw new PhaseError("only a started PM pane can be woken");
    if (entry.agent === undefined)
      throw new PhaseError("the pane has no agent");
    if (
      !isSafeText(input.text) ||
      /[\r\n]/.test(input.text) ||
      COMMAND_START.test(input.text) ||
      Buffer.byteLength(input.text, "utf8") > MAX_TEXT_BYTES
    )
      throw new InvalidArgumentError("wake text is not acceptable");
    const idle = async (): Promise<boolean> =>
      deferralFor(await this.#stateFor(entry.agent!, input.paneId)) ===
      undefined;
    if (!(await idle())) return { sent: false, reason: "pm_not_idle" };
    if ((await this.readInput(input.paneId)) !== "")
      return { sent: false, reason: "input_not_empty" };
    if (!(await idle())) return { sent: false, reason: "pm_not_idle" };
    await input.beforeSend();
    await runJson(this.#run, [
      "agent",
      "prompt",
      this.#herdrName(entry.agent),
      input.text,
    ]);
    return { sent: true };
  }

  async clearAfterDeferral(input: {
    paneId: string;
    deferredForMs: number;
    maxDeferralMs: number;
    discard: (text: string) => void | Promise<void>;
    log: KeyLogger;
  }): Promise<{ cleared: boolean; text: string }> {
    if (
      !Number.isFinite(input.deferredForMs) ||
      !Number.isFinite(input.maxDeferralMs) ||
      input.deferredForMs < 0 ||
      input.maxDeferralMs <= 0
    )
      throw new InvalidArgumentError(
        "the deferral times must be finite, and the maximum must be positive",
      );
    if (input.deferredForMs < input.maxDeferralMs)
      throw new DeferralNotElapsed("the maximum deferral has not elapsed");
    const entry = this.#assertTypable(input.paneId, "clear");
    if (entry.agent === undefined)
      throw new PhaseError("the pane has no agent");
    const status = await this.#stateFor(entry.agent, input.paneId);
    if (deferralFor(status) !== undefined)
      throw new NotIdle("only an idle agent's input line is cleared");
    const { text, blocker } = await this.#readInputAndBlocker(input.paneId);
    if (text === undefined)
      throw new InputUnreadable(
        "the input line cannot be read, so its text cannot be logged",
        blocker,
      );
    if (text === "") return { cleared: false, text: "" };
    await input.discard(text);
    let known = text;
    for (let round = 0; round < MAX_CLEAR_ROUNDS; round += 1) {
      if (round > 0) {
        const again = await this.#stateFor(entry.agent, input.paneId);
        if (deferralFor(again) !== undefined)
          throw new NotIdle("the agent stopped being idle during the clear");
      }
      await this.#sendKey(
        input.paneId,
        "ctrl+u",
        "clear the input line after the maximum deferral",
        input.log,
      );
      const { text: remaining, blocker: after } =
        await this.#readInputAndBlocker(input.paneId);
      if (remaining === "") return { cleared: true, text };
      if (remaining === undefined)
        throw new InputUnreadable(
          "the input line cannot be read after a clear key",
          after,
        );
      if (!known.includes(remaining)) {
        known += `\n${remaining}`;
        await input.discard(remaining);
      }
    }
    throw new ClearFailed(
      `the input line is not empty after ${MAX_CLEAR_ROUNDS} rounds`,
    );
  }

  async answerTrustDialog(input: {
    paneId: string;
    log: KeyLogger;
    timeoutMs?: number;
  }): Promise<DialogOutcome> {
    const entry = this.#assertTypable(input.paneId, "dialog");
    if (entry.agent === undefined || entry.worktreePath === undefined)
      throw new PhaseError(
        "only a worktree pane the adapter created has a dialog it may answer",
      );
    const texts = trustTexts(entry.kind);
    if (texts === undefined)
      return { handled: false, reason: "host_has_no_trust_dialog" };
    const state = await this.agentState(entry.agent);
    if (state.paneId !== input.paneId || state.status !== "blocked")
      throw new NotBlocked("the agent is not blocked at its own pane");
    const check = async (): Promise<
      | { ok: true; selected: number; target: number }
      | { ok: false; reason: string }
    > => {
      const dialog = parseTrustDialogOf(
        entry.kind,
        await this.readScreen(input.paneId),
      );
      if (dialog === undefined) return { ok: false, reason: "no_dialog" };
      if (dialog.kind === "wrapped_path")
        return { ok: false, reason: "wrapped_path" };
      if (!dialog.confirmIsLastLine)
        return { ok: false, reason: "dialog_not_last" };
      if (!this.#samePath(dialog.path, entry.worktreePath!))
        return { ok: false, reason: "path_mismatch" };
      const shown = dialog.options.map((option) => option.text);
      if (
        shown.length !== 2 ||
        !shown.includes(texts.yes) ||
        !shown.includes(texts.no) ||
        dialog.selectedIndex === undefined
      )
        return { ok: false, reason: "unknown_options" };
      return {
        ok: true,
        selected: dialog.selectedIndex,
        target: shown.indexOf(texts.yes),
      };
    };
    const first = await check();
    if (!first.ok) return { handled: false, reason: first.reason };
    const keys: string[] = [];
    const steps = first.target - first.selected;
    for (let step = 0; step < Math.abs(steps); step += 1) {
      const key = steps > 0 ? "down" : "up";
      await this.#sendKey(
        input.paneId,
        key,
        "move the trust dialog selection to the trusted option",
        input.log,
      );
      keys.push(key);
    }
    // The screen redraws a moment after a key, so the selection is polled for
    // a short while; the Enter below is still sent only after a read shows it.
    const redrawDeadline = this.#now() + SELECTION_REDRAW_MS;
    let second = await check();
    // A half-drawn screen can read as no dialog, unknown options or a dialog
    // that is not last; those are waited out. A different path or a wrapped
    // path is not a redraw problem and stops the answer at once.
    const transient = (result: typeof second): boolean =>
      result.ok
        ? result.selected !== result.target
        : ["no_dialog", "unknown_options", "dialog_not_last"].includes(
            result.reason,
          );
    while (
      keys.length > 0 &&
      transient(second) &&
      this.#now() < redrawDeadline
    ) {
      await this.#sleep(this.#pollMs);
      second = await check();
    }
    if (!second.ok) return { handled: false, reason: second.reason };
    if (second.selected !== second.target)
      return { handled: false, reason: "selection_not_reached" };
    await this.#sendKey(
      input.paneId,
      "enter",
      "confirm the trusted option",
      input.log,
    );
    keys.push("enter");
    const deadline = this.#now() + (input.timeoutMs ?? 10_000);
    while (this.#now() < deadline) {
      if (
        parseTrustDialogOf(entry.kind, await this.readScreen(input.paneId)) ===
        undefined
      )
        return { handled: true, keys };
      await this.#sleep(this.#pollMs);
    }
    throw new DialogStillOpen(
      "the trust dialog is still open after the answer",
    );
  }

  /**
   * Reads the blocking permission prompt of a started worker pane. Only a
   * blocked claude agent at its own pane whose screen is a fixture-proven
   * dialog is captured; nothing is typed.
   */
  async capturePrompt(paneId: string): Promise<CaptureOutcome> {
    const entry = this.#assertTypable(paneId, "dialog");
    if (entry.agent === undefined)
      throw new PhaseError("the pane has no agent");
    const refusal = await this.#relayPreflight(entry, paneId);
    if (refusal === undefined) {
      const prompt = await this.#readPrompt(entry.agent, paneId, entry.kind);
      if (prompt !== undefined)
        return { captured: true, prompt: prompt.prompt };
    }
    // A dialog that is not a permission prompt may not set Herdr blocked.
    if (
      entry.kind === "claude" &&
      (await this.#notWorking(entry.agent, paneId))
    ) {
      const dialog = await this.#readDialog(entry.agent, paneId, entry.kind);
      if (dialog !== undefined) return { captured: true, prompt: dialog };
    }
    return {
      captured: false,
      reason: refusal ?? "prompt_unrecognized",
    };
  }

  /**
   * Types an answer to a captured prompt. The screen is read again and must
   * hash to `promptSha` before the first key and before each Enter; arrows
   * are sent one at a time and the Enter only after a read shows the target
   * option selected. `beforeType` runs once, after every check that precedes
   * the first key and before it.
   */
  async answerPrompt(input: {
    paneId: string;
    promptSha: string;
    answer: PromptAnswer;
    beforeType: () => void | Promise<void>;
    log: KeyLogger;
  }): Promise<RelayOutcome> {
    const { paneId } = input;
    const entry = this.#assertTypable(paneId, "dialog");
    const agent = entry.agent;
    if (agent === undefined) throw new PhaseError("the pane has no agent");
    const keys: string[] = [];
    const refuse = (reason: RelayRefusal): RelayOutcome => ({
      typed: false,
      reason,
      keys: [...keys],
    });
    if (entry.kind === "claude") {
      const screen = await this.readScreen(paneId, { ansi: true });
      if (parseHostPrompt(entry.kind, screen) === undefined) {
        if (parseBlockingDialog(entry.kind, screen) !== undefined)
          return this.#answerDialog(entry, input, screen);
        // Neither a permission prompt nor a dialog: nothing here may be answered.
        return refuse("prompt_unrecognized");
      }
    }
    const preflight = await this.#relayPreflight(entry, paneId);
    if (preflight !== undefined) return refuse(preflight);
    const first = await this.#readPrompt(agent, paneId, entry.kind);
    if (first === undefined) return refuse("prompt_unrecognized");
    if (first.prompt.promptSha !== input.promptSha)
      return refuse("prompt_changed");
    const answer = input.answer;
    const problem = checkAnswer(first.prompt, answer);
    if (problem !== undefined) return refuse(problem);
    if (answer.kind === "text" && relayTextProblem(answer.text) !== undefined)
      return refuse("text_refused");

    let announced = false;
    const press = async (key: string, reason: string): Promise<void> => {
      if (!announced) {
        announced = true;
        await input.beforeType();
      }
      await this.#sendKey(paneId, key, reason, input.log);
      keys.push(key);
    };
    const stillBlocked = (): Promise<boolean> => this.#isBlocked(agent, paneId);

    if (answer.kind === "esc") {
      if (!(await stillBlocked())) return refuse("not_blocked");
      await press("esc", "dismiss the worker's permission prompt");
      return { typed: true, keys };
    }

    const target = answer.number - 1;
    const steps = target - first.selectedIndex;
    for (let step = 0; step < Math.abs(steps); step += 1)
      await press(
        steps > 0 ? "down" : "up",
        "move the prompt selection to the answer",
      );
    // The screen redraws a moment after a key, so the selection is polled for
    // a short while; the Enter below is still sent only after a read shows it.
    const settled = await this.#awaitPrompt(
      agent,
      paneId,
      entry.kind,
      (read) => read.selectedIndex === target,
      input.promptSha,
    );
    if (!settled.ok) return refuse(settled.reason);
    if (!(await stillBlocked())) return refuse("not_blocked");

    if (answer.kind === "option") {
      await press("enter", "confirm the selected answer");
      return { typed: true, keys };
    }

    const original = settled.read.options[target]!.text;
    const fieldWording = TEXT_FIELD_WORDING[original];
    if (fieldWording === undefined) return refuse("no_text_option");
    // Only the target option may differ from the prompt as it was captured.
    const sameExceptTarget = (read: PromptRead, expected: string): boolean =>
      read.selectedIndex === target &&
      read.promptText === settled.read.promptText &&
      read.options.length === settled.read.options.length &&
      read.options[target]!.text === expected &&
      read.options.every(
        (entry, index) =>
          index === target || entry.text === settled.read.options[index]!.text,
      );
    await press("tab", "open the option's text field");
    // Esc would cancel the whole prompt, so a field that is not as expected is left as it is.
    const opened = await this.#awaitPrompt(
      agent,
      paneId,
      entry.kind,
      (read) => read.options[target]?.text !== original,
      undefined,
    );
    if (
      !opened.ok ||
      !sameExceptTarget(opened.read, fieldWording) ||
      opened.read.footer !== "Esc to cancel"
    )
      return refuse("text_field_not_open");
    if (!(await stillBlocked())) return refuse("not_blocked");
    await input.log({
      kind: "key",
      pane: paneId,
      key: "text",
      reason: `type ${Buffer.byteLength(answer.text, "utf8")} bytes into the open text field`,
    });
    await this.#runChecked(["pane", "send-text", paneId, answer.text]);
    keys.push("text");
    const typed = await this.#awaitPrompt(
      agent,
      paneId,
      entry.kind,
      (read) => sameExceptTarget(read, `${original}, ${answer.text.trimEnd()}`),
      undefined,
    );
    if (!typed.ok) return refuse("text_field_not_open");
    if (!(await stillBlocked())) return refuse("not_blocked");
    await press("enter", "submit the typed answer");
    return { typed: true, keys };
  }

  /**
   * Interrupts a working agent with exactly one Esc. Nothing is sent when Herdr
   * does not show the agent working, and no second key is ever sent.
   */
  async interruptWorking(input: {
    paneId: string;
    log: KeyLogger;
  }): Promise<{ readonly sent: boolean }> {
    const entry = this.#assertTypable(input.paneId, "dialog");
    if (entry.agent === undefined)
      throw new PhaseError("the pane has no agent");
    if ((await this.#stateFor(entry.agent, input.paneId)) !== "working")
      return { sent: false };
    await this.#sendKey(
      input.paneId,
      "esc",
      "interrupt a paused worker",
      input.log,
    );
    return { sent: true };
  }

  /**
   * Answers an Esc-only dialog relay: the hash must match the screen just
   * read, the agent must not be working, and exactly one Esc is sent. Then the
   * input line is polled for a short while; no second key is ever sent.
   */
  async #answerDialog(
    entry: PaneEntry,
    input: {
      paneId: string;
      promptSha: string;
      answer: PromptAnswer;
      beforeType: () => void | Promise<void>;
      log: KeyLogger;
    },
    screen: string,
  ): Promise<RelayOutcome> {
    const { paneId } = input;
    const agent = entry.agent!;
    const refuse = (reason: RelayRefusal): RelayOutcome => ({
      typed: false,
      reason,
      keys: [],
    });
    if (input.answer.kind !== "esc") return refuse("no_such_option");
    const read = this.#dialogOf(agent, paneId, entry.kind, screen);
    if (read === undefined) return refuse("prompt_unrecognized");
    if (read.promptSha !== input.promptSha) return refuse("prompt_changed");
    if (!(await this.#notWorking(agent, paneId))) return refuse("not_blocked");
    await input.beforeType();
    await this.#sendKey(
      paneId,
      "esc",
      "dismiss the worker's blocking dialog",
      input.log,
    );
    const deadline = this.#now() + SELECTION_REDRAW_MS;
    let inputReadable = false;
    for (;;) {
      inputReadable = (await this.readInput(paneId)) !== undefined;
      if (inputReadable || this.#now() >= deadline) break;
      await this.#sleep(this.#pollMs);
    }
    return { typed: true, keys: ["esc"], inputReadable };
  }

  /** True unless Herdr shows the agent working; a name that points at another pane is not usable here. */
  async #notWorking(agent: string, paneId: string): Promise<boolean> {
    try {
      return (await this.#stateFor(agent, paneId)) !== "working";
    } catch (error) {
      if (error instanceof AgentPaneMismatch) return false;
      throw error;
    }
  }

  #dialogOf(
    agent: string,
    paneId: string,
    kind: string,
    screen: string,
  ): CapturedPrompt | undefined {
    const parsed = parseBlockingDialog(kind, screen);
    if (parsed === undefined) return undefined;
    const hashed = {
      agentId: agent,
      paneId,
      hostKind: kind,
      text: parsed.text,
      options: [] as CapturedPrompt["options"],
      dialog: true,
    };
    return { ...hashed, promptSha: promptHash(hashed) };
  }

  async #readDialog(
    agent: string,
    paneId: string,
    kind: string,
  ): Promise<CapturedPrompt | undefined> {
    return this.#dialogOf(
      agent,
      paneId,
      kind,
      await this.readScreen(paneId, { ansi: true }),
    );
  }

  /** Why a prompt may not be read at all: another kind of host, or an agent that is not blocked. */
  async #relayPreflight(
    entry: PaneEntry,
    paneId: string,
  ): Promise<RelayRefusal | undefined> {
    if (entry.kind !== "claude") return "unsupported_host";
    return (await this.#isBlocked(entry.agent!, paneId))
      ? undefined
      : "not_blocked";
  }

  /** True only when Herdr shows the agent blocked at this very pane; a name that points at another pane is not blocked here. */
  async #isBlocked(agent: string, paneId: string): Promise<boolean> {
    try {
      return (await this.#stateFor(agent, paneId)) === "blocked";
    } catch (error) {
      if (error instanceof AgentPaneMismatch) return false;
      throw error;
    }
  }

  async #readPrompt(
    agent: string,
    paneId: string,
    kind: string,
  ): Promise<PromptRead | undefined> {
    const screen = await this.readScreen(paneId, { ansi: true });
    const parsed = parseHostPrompt(kind, screen);
    if (parsed === undefined) return undefined;
    const hashed = {
      agentId: agent,
      paneId,
      hostKind: kind,
      text: parsed.text,
      options: parsed.options,
    };
    return {
      prompt: { ...hashed, promptSha: promptHash(hashed) },
      selectedIndex: parsed.selectedIndex,
      options: parsed.options,
      promptText: parsed.text,
      footer: promptFooter(screen),
    };
  }

  /**
   * Polls the prompt until `done` holds for it. A read that does not parse or
   * does not satisfy `done` is waited out for a short while; a read whose hash
   * differs from `sha` stops the answer at once.
   */
  async #awaitPrompt(
    agent: string,
    paneId: string,
    kind: string,
    done: (read: PromptRead) => boolean,
    sha: string | undefined,
  ): Promise<
    | {
        ok: true;
        read: PromptRead;
      }
    | { ok: false; reason: RelayRefusal }
  > {
    const deadline = this.#now() + SELECTION_REDRAW_MS;
    for (;;) {
      const read = await this.#readPrompt(agent, paneId, kind);
      if (read !== undefined) {
        if (sha !== undefined && read.prompt.promptSha !== sha)
          return { ok: false, reason: "prompt_changed" };
        if (done(read)) return { ok: true, read };
      }
      if (this.#now() >= deadline)
        return {
          ok: false,
          reason:
            read === undefined
              ? "prompt_unrecognized"
              : "selection_not_reached",
        };
      await this.#sleep(this.#pollMs);
    }
  }

  /** For commands that print nothing on success: a non-zero exit or a JSON error is a failure. */
  async #runChecked(args: readonly string[]): Promise<void> {
    const outcome = await this.#run(args);
    if (outcome.code !== 0) throw failureOf(args, outcome);
  }

  #record(value: unknown, label: string): Record<string, unknown> {
    if (typeof value !== "object" || value === null || Array.isArray(value))
      throw new HerdrError("bad_output", `herdr did not report ${label}`);
    return value as Record<string, unknown>;
  }

  #canonical(value: string): string {
    const trim = (text: string): string => text.replace(/\/+$/, "") || "/";
    try {
      return trim(fs.realpathSync(value));
    } catch {
      return trim(path.resolve(value));
    }
  }

  #samePath(shown: string, expected: string): boolean {
    try {
      const normalize = (value: string): string =>
        fs.realpathSync(value.length > 1 ? value.replace(/\/+$/, "") : value);
      return normalize(shown) === normalize(expected);
    } catch {
      return false;
    }
  }

  async #stateFor(agent: string, paneId: string): Promise<string> {
    const state = await this.agentState(agent);
    if (state.paneId !== paneId)
      throw new AgentPaneMismatch(
        "the agent name no longer points at the registered pane",
      );
    return state.status;
  }

  async #sendKey(
    paneId: string,
    key: string,
    reason: string,
    log: KeyLogger,
  ): Promise<void> {
    await log({ kind: "key", pane: paneId, key, reason });
    await this.#runChecked(["pane", "send-keys", paneId, key]);
  }

  async #waitForFreshPrompt(paneId: string): Promise<void> {
    const deadline = this.#now() + 3_000;
    do {
      if (freshPromptReady(stripAnsi(await this.readScreen(paneId)))) return;
      await this.#sleep(this.#pollMs);
    } while (this.#now() < deadline);
    throw new PromptUnrecognized(
      "the pane does not end in a bare prompt symbol, so nothing was typed",
    );
  }

  #assertTypable(
    paneId: string,
    action: "prepare" | "start" | "send" | "clear" | "dialog",
  ): PaneEntry {
    const entry = this.#panes.get(paneId);
    if (entry === undefined)
      throw new UnknownPaneError("the pane was not created by this adapter");
    if (entry.phase === "tainted")
      throw new PhaseError("the pane is tainted and must be closed");
    const launching = action === "prepare" || action === "start";
    if (entry.role === "PM" && (entry.phase === "started" || !launching))
      throw new PmPaneError(
        "the PM pane accepts only the controller's launch sequence",
      );
    if (launching) {
      if (entry.phase === "started")
        throw new PhaseError("the agent is already started");
      if (action === "prepare" && entry.phase !== "fresh")
        throw new PhaseError("the pane is already prepared");
    } else if (entry.phase !== "started") {
      throw new PhaseError("the pane has no started agent");
    }
    return entry;
  }
}
