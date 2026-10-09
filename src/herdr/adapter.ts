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
import { HOST_KINDS } from "../config/capstan-config.js";
import { HERDR_STATES, type HerdrState } from "../controller/messaging.js";
import {
  AgentPaneMismatch,
  InvalidArgumentError,
  PhaseError,
  PmPaneError,
  PromptUnrecognized,
  ShellNotReady,
  UnknownPaneError,
  UnsupportedHostError,
} from "./adapter-errors.js";
import { PaneInput } from "./adapter-input.js";
import {
  PaneOperations,
  type AdapterCore,
  type PaneEntry,
} from "./adapter-panes.js";
import {
  CONTROL_CHARACTERS,
  ENVIRONMENT_KEY,
  NAME_PATTERN,
  PANE_PATTERN,
  SIMPLE_VALUE,
  UNSAFE_TEXT,
  isSafeText,
  requireMatch,
  requireQuotable,
  shellQuote,
} from "./adapter-validate.js";
import { herdrAgentName } from "./naming.js";
import { HerdrError, failureOf, runJson, type HerdrRunner } from "./runner.js";
import {
  extractInputLine,
  freshPromptReady,
  classifyInputBlocker,
  type InputBlocker,
  stripAnsi,
} from "./screen.js";

export * from "./adapter-errors.js";
export {
  INPUT_UNREADABLE_DETAIL,
  type DialogOutcome,
  type KeyLogEntry,
  type KeyLogger,
  type SendOutcome,
} from "./adapter-input.js";
export type {
  PaneEntry,
  PaneIdentity,
  PaneLayoutView,
  PanePhase,
  PaneRole,
} from "./adapter-panes.js";
export { MAX_TEXT_BYTES, isAgentName, shellQuote } from "./adapter-validate.js";
export {
  INBOX_HOOK_COMMAND,
  buildAgentEnvironment,
  claudeArguments,
  type ClaudeRoleSettings,
} from "./claude-args.js";

export const TESTED_HERDR_VERSION = "0.9.1";

export interface AdapterOptions {
  readonly run: HerdrRunner;
  readonly tempRoot?: string;
  readonly pollMs?: number;
  readonly sleep?: (ms: number) => Promise<void>;
  readonly now?: () => number;
  /** The project's slug: Herdr then sees every agent as `<slug>-<agent-id>`, while callers keep passing the ledger id. */
  readonly projectSlug?: string;
}

const AGENT_START_MARGIN_MS = 10_000;
const MAX_NOTIFICATION_TITLE_CHARS = 100;
const MAX_NOTIFICATION_BODY_CHARS = 500;

/** Maps a Herdr agent status to a state the ledger accepts; anything unrecognized is `unknown`. */
export function herdrStateOf(status: string): HerdrState {
  return (HERDR_STATES as readonly string[]).includes(status)
    ? (status as HerdrState)
    : "unknown";
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
  readonly #core: AdapterCore;
  readonly #paneOperations: PaneOperations;
  readonly #paneInput: PaneInput;

  constructor(options: AdapterOptions) {
    this.#run = options.run;
    this.#slug = options.projectSlug;
    this.#tempRoot = options.tempRoot ?? os.tmpdir();
    this.#pollMs = options.pollMs ?? 100;
    this.#sleep =
      options.sleep ??
      ((ms) => new Promise((resolve) => setTimeout(resolve, ms)));
    this.#now = options.now ?? Date.now;

    this.#core = {
      run: this.#run,
      panes: this.#panes,
      slug: this.#slug,
      pollMs: this.#pollMs,
      now: () => this.#now(),
      sleep: (ms) => this.#sleep(ms),
      herdrName: (id) => this.#herdrName(id),
      runChecked: (args) => this.#runChecked(args),
      record: (value, label) => this.#record(value, label),
      assertTypable: (paneId, action) => this.#assertTypable(paneId, action),
      stateFor: (agent, paneId) => this.#stateFor(agent, paneId),
      agentState: (name) => this.agentState(name),
      agentStateByHerdrName: (name) => this.#agentStateByHerdrName(name),
      readScreen: (paneId, options) => this.readScreen(paneId, options),
      readInput: (paneId) => this.readInput(paneId),
      readInputAndBlocker: (paneId) => this.#readInputAndBlocker(paneId),
    };
    this.#paneOperations = new PaneOperations(this.#core);
    this.#paneInput = new PaneInput(this.#core);
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

  async #stateFor(agent: string, paneId: string): Promise<string> {
    const state = await this.agentState(agent);
    if (state.paneId !== paneId)
      throw new AgentPaneMismatch(
        "the agent name no longer points at the registered pane",
      );
    return state.status;
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

  createWorktree(
    ...args: Parameters<PaneOperations["createWorktree"]>
  ): ReturnType<PaneOperations["createWorktree"]> {
    return this.#paneOperations.createWorktree(...args);
  }

  paneLayout(
    ...args: Parameters<PaneOperations["paneLayout"]>
  ): ReturnType<PaneOperations["paneLayout"]> {
    return this.#paneOperations.paneLayout(...args);
  }

  panesAtPath(
    ...args: Parameters<PaneOperations["panesAtPath"]>
  ): ReturnType<PaneOperations["panesAtPath"]> {
    return this.#paneOperations.panesAtPath(...args);
  }

  placePane(
    ...args: Parameters<PaneOperations["placePane"]>
  ): ReturnType<PaneOperations["placePane"]> {
    return this.#paneOperations.placePane(...args);
  }

  createWorkspace(
    ...args: Parameters<PaneOperations["createWorkspace"]>
  ): ReturnType<PaneOperations["createWorkspace"]> {
    return this.#paneOperations.createWorkspace(...args);
  }

  createTab(
    ...args: Parameters<PaneOperations["createTab"]>
  ): ReturnType<PaneOperations["createTab"]> {
    return this.#paneOperations.createTab(...args);
  }

  removeWorktree(
    ...args: Parameters<PaneOperations["removeWorktree"]>
  ): ReturnType<PaneOperations["removeWorktree"]> {
    return this.#paneOperations.removeWorktree(...args);
  }

  reportMetadata(
    ...args: Parameters<PaneOperations["reportMetadata"]>
  ): ReturnType<PaneOperations["reportMetadata"]> {
    return this.#paneOperations.reportMetadata(...args);
  }

  renameTab(
    ...args: Parameters<PaneOperations["renameTab"]>
  ): ReturnType<PaneOperations["renameTab"]> {
    return this.#paneOperations.renameTab(...args);
  }

  renameWorkspace(
    ...args: Parameters<PaneOperations["renameWorkspace"]>
  ): ReturnType<PaneOperations["renameWorkspace"]> {
    return this.#paneOperations.renameWorkspace(...args);
  }

  paneIdentity(
    ...args: Parameters<PaneOperations["paneIdentity"]>
  ): ReturnType<PaneOperations["paneIdentity"]> {
    return this.#paneOperations.paneIdentity(...args);
  }

  closePane(
    ...args: Parameters<PaneOperations["closePane"]>
  ): ReturnType<PaneOperations["closePane"]> {
    return this.#paneOperations.closePane(...args);
  }

  adoptPane(
    ...args: Parameters<PaneOperations["adoptPane"]>
  ): ReturnType<PaneOperations["adoptPane"]> {
    return this.#paneOperations.adoptPane(...args);
  }

  adoptShellPane(
    ...args: Parameters<PaneOperations["adoptShellPane"]>
  ): ReturnType<PaneOperations["adoptShellPane"]> {
    return this.#paneOperations.adoptShellPane(...args);
  }

  guardedSend(
    ...args: Parameters<PaneInput["guardedSend"]>
  ): ReturnType<PaneInput["guardedSend"]> {
    return this.#paneInput.guardedSend(...args);
  }

  wakePm(
    ...args: Parameters<PaneInput["wakePm"]>
  ): ReturnType<PaneInput["wakePm"]> {
    return this.#paneInput.wakePm(...args);
  }

  clearAfterDeferral(
    ...args: Parameters<PaneInput["clearAfterDeferral"]>
  ): ReturnType<PaneInput["clearAfterDeferral"]> {
    return this.#paneInput.clearAfterDeferral(...args);
  }

  answerTrustDialog(
    ...args: Parameters<PaneInput["answerTrustDialog"]>
  ): ReturnType<PaneInput["answerTrustDialog"]> {
    return this.#paneInput.answerTrustDialog(...args);
  }

  capturePrompt(
    ...args: Parameters<PaneInput["capturePrompt"]>
  ): ReturnType<PaneInput["capturePrompt"]> {
    return this.#paneInput.capturePrompt(...args);
  }

  answerPrompt(
    ...args: Parameters<PaneInput["answerPrompt"]>
  ): ReturnType<PaneInput["answerPrompt"]> {
    return this.#paneInput.answerPrompt(...args);
  }

  interruptWorking(
    ...args: Parameters<PaneInput["interruptWorking"]>
  ): ReturnType<PaneInput["interruptWorking"]> {
    return this.#paneInput.interruptWorking(...args);
  }
}
