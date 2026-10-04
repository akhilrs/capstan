/** Input to a started agent: guarded send, PM wake, clear, trust dialog, prompt relay and interrupt. */
import {
  AgentPaneMismatch,
  ClearFailed,
  DeferralNotElapsed,
  DialogStillOpen,
  InputUnreadable,
  InvalidArgumentError,
  NotBlocked,
  NotIdle,
  PhaseError,
  SendAfterRecordError,
  UnknownPaneError,
} from "./adapter-errors.js";
import type { AdapterCore, PaneEntry } from "./adapter-panes.js";
import {
  COMMAND_START,
  MAX_TEXT_BYTES,
  PANE_PATTERN,
  isSafeText,
  requireMatch,
} from "./adapter-validate.js";
import type { DeferralReason } from "../controller/messaging.js";
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
import { runJson } from "./runner.js";
import {
  parseBlockingDialog,
  parseHostPrompt,
  parseTrustDialogOf,
  promptFooter,
  TEXT_FIELD_WORDING,
  trustTexts,
  type InputBlocker,
} from "./screen.js";
import fs from "node:fs";

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

const MAX_CLEAR_ROUNDS = 5;
const SELECTION_REDRAW_MS = 3_000;

/** The `detail` a deferral carries when the input line could not be read. */
export const INPUT_UNREADABLE_DETAIL = "the input line is unreadable";

function deferralFor(status: string): DeferralReason | undefined {
  if (status === "idle" || status === "done") return undefined;
  return status === "blocked" ? "agent_blocked" : "agent_busy";
}

export class PaneInput {
  readonly core: AdapterCore;

  constructor(core: AdapterCore) {
    this.core = core;
  }

  async guardedSend(input: {
    paneId: string;
    text: string;
    beforeSend: () => void | Promise<void>;
  }): Promise<SendOutcome> {
    const entry = this.core.assertTypable(input.paneId, "send");
    if (
      !isSafeText(input.text) ||
      COMMAND_START.test(input.text) ||
      Buffer.byteLength(input.text, "utf8") > MAX_TEXT_BYTES
    )
      throw new InvalidArgumentError("message text is not acceptable");
    const agent = entry.agent;
    if (agent === undefined) throw new PhaseError("the pane has no agent");
    const first = await this.core.stateFor(agent, input.paneId);
    const busy = deferralFor(first);
    if (busy !== undefined) return { sent: false, reason: busy };
    const { text: typed, blocker } = await this.core.readInputAndBlocker(
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
    const second = deferralFor(await this.core.stateFor(agent, input.paneId));
    if (second !== undefined) return { sent: false, reason: second };
    await input.beforeSend();
    try {
      await runJson(this.core.run, [
        "agent",
        "prompt",
        this.core.herdrName(agent),
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
    const entry = this.core.panes.get(input.paneId);
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
      deferralFor(await this.core.stateFor(entry.agent!, input.paneId)) ===
      undefined;
    if (!(await idle())) return { sent: false, reason: "pm_not_idle" };
    if ((await this.core.readInput(input.paneId)) !== "")
      return { sent: false, reason: "input_not_empty" };
    if (!(await idle())) return { sent: false, reason: "pm_not_idle" };
    await input.beforeSend();
    await runJson(this.core.run, [
      "agent",
      "prompt",
      this.core.herdrName(entry.agent),
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
    const entry = this.core.assertTypable(input.paneId, "clear");
    if (entry.agent === undefined)
      throw new PhaseError("the pane has no agent");
    const status = await this.core.stateFor(entry.agent, input.paneId);
    if (deferralFor(status) !== undefined)
      throw new NotIdle("only an idle agent's input line is cleared");
    const { text, blocker } = await this.core.readInputAndBlocker(input.paneId);
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
        const again = await this.core.stateFor(entry.agent, input.paneId);
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
        await this.core.readInputAndBlocker(input.paneId);
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
    const entry = this.core.assertTypable(input.paneId, "dialog");
    if (entry.agent === undefined || entry.worktreePath === undefined)
      throw new PhaseError(
        "only a worktree pane the adapter created has a dialog it may answer",
      );
    const texts = trustTexts(entry.kind);
    if (texts === undefined)
      return { handled: false, reason: "host_has_no_trust_dialog" };
    const state = await this.core.agentState(entry.agent);
    if (state.paneId !== input.paneId || state.status !== "blocked")
      throw new NotBlocked("the agent is not blocked at its own pane");
    const check = async (): Promise<
      | { ok: true; selected: number; target: number }
      | { ok: false; reason: string }
    > => {
      const dialog = parseTrustDialogOf(
        entry.kind,
        await this.core.readScreen(input.paneId),
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
    const redrawDeadline = this.core.now() + SELECTION_REDRAW_MS;
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
      this.core.now() < redrawDeadline
    ) {
      await this.core.sleep(this.core.pollMs);
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
    const deadline = this.core.now() + (input.timeoutMs ?? 10_000);
    while (this.core.now() < deadline) {
      if (
        parseTrustDialogOf(
          entry.kind,
          await this.core.readScreen(input.paneId),
        ) === undefined
      )
        return { handled: true, keys };
      await this.core.sleep(this.core.pollMs);
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
    const entry = this.core.assertTypable(paneId, "dialog");
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
    const entry = this.core.assertTypable(paneId, "dialog");
    const agent = entry.agent;
    if (agent === undefined) throw new PhaseError("the pane has no agent");
    const keys: string[] = [];
    const refuse = (reason: RelayRefusal): RelayOutcome => ({
      typed: false,
      reason,
      keys: [...keys],
    });
    if (entry.kind === "claude") {
      const screen = await this.core.readScreen(paneId, { ansi: true });
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
    await this.core.runChecked(["pane", "send-text", paneId, answer.text]);
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
    const entry = this.core.assertTypable(input.paneId, "dialog");
    if (entry.agent === undefined)
      throw new PhaseError("the pane has no agent");
    if ((await this.core.stateFor(entry.agent, input.paneId)) !== "working")
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
    const deadline = this.core.now() + SELECTION_REDRAW_MS;
    let inputReadable = false;
    for (;;) {
      inputReadable = (await this.core.readInput(paneId)) !== undefined;
      if (inputReadable || this.core.now() >= deadline) break;
      await this.core.sleep(this.core.pollMs);
    }
    return { typed: true, keys: ["esc"], inputReadable };
  }

  /** True unless Herdr shows the agent working; a name that points at another pane is not usable here. */
  async #notWorking(agent: string, paneId: string): Promise<boolean> {
    try {
      return (await this.core.stateFor(agent, paneId)) !== "working";
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
      await this.core.readScreen(paneId, { ansi: true }),
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
      return (await this.core.stateFor(agent, paneId)) === "blocked";
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
    const screen = await this.core.readScreen(paneId, { ansi: true });
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
    const deadline = this.core.now() + SELECTION_REDRAW_MS;
    for (;;) {
      const read = await this.#readPrompt(agent, paneId, kind);
      if (read !== undefined) {
        if (sha !== undefined && read.prompt.promptSha !== sha)
          return { ok: false, reason: "prompt_changed" };
        if (done(read)) return { ok: true, read };
      }
      if (this.core.now() >= deadline)
        return {
          ok: false,
          reason:
            read === undefined
              ? "prompt_unrecognized"
              : "selection_not_reached",
        };
      await this.core.sleep(this.core.pollMs);
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
  async #sendKey(
    paneId: string,
    key: string,
    reason: string,
    log: KeyLogger,
  ): Promise<void> {
    await log({ kind: "key", pane: paneId, key, reason });
    await this.core.runChecked(["pane", "send-keys", paneId, key]);
  }
}
