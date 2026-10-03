/**
 * Types shared by the Herdr layer and the launcher for relaying a worker's
 * blocking permission prompt: what was captured, how it is hashed, and which
 * answers may be typed. Nothing here talks to Herdr.
 */
import { createHash } from "node:crypto";

export type RelayHostKind = "claude" | "codex" | "omp";

export interface RelayOption {
  /** 1-based, in screen order. */
  readonly number: number;
  readonly text: string;
  readonly acceptsText: boolean;
  readonly widensPermissions: boolean;
}

export interface CapturedPrompt {
  readonly agentId: string;
  readonly paneId: string;
  readonly hostKind: string;
  readonly text: string;
  readonly options: readonly RelayOption[];
  /** 64 lowercase hex characters. */
  readonly promptSha: string;
  /** True only for an Esc-only relay of an unrecognised blocking dialog; its options are []. */
  readonly dialog?: boolean;
}

export type PromptAnswer =
  | { readonly kind: "option"; readonly number: number }
  | { readonly kind: "esc" }
  | { readonly kind: "text"; readonly number: number; readonly text: string };

export type RelayRefusal =
  | "not_blocked"
  | "unsupported_host"
  | "prompt_unrecognized"
  | "prompt_changed"
  | "no_such_option"
  | "no_text_option"
  | "text_refused"
  | "selection_not_reached"
  | "text_field_not_open"
  | "dialog_still_open";

export type CaptureOutcome =
  | { readonly captured: true; readonly prompt: CapturedPrompt }
  | { readonly captured: false; readonly reason: RelayRefusal };

export type RelayOutcome =
  | {
      readonly typed: true;
      readonly keys: readonly string[];
      /** Dialog relays only: the input line reads again after the Esc. */
      readonly inputReadable?: boolean;
    }
  | {
      readonly typed: false;
      readonly reason: RelayRefusal;
      /** Keys already sent; empty when nothing was typed. */
      readonly keys: readonly string[];
    };

export const RELAY_TEXT_MAX_BYTES = 1000;
export const RELAY_PROMPT_MAX_BYTES = 8192;

const UNSAFE = /[\p{Cc}\p{Cf}\p{Zl}\p{Zp}\p{Noncharacter_Code_Point}]/u;
/** A first character of / ! # ? or @ (or a tab) acts on the input box instead of adding text. */
const COMMAND_START = /^(?:\t|\s*[/!#?@])/;

/** The SHA-256 of a canonical JSON of the prompt, with a fixed key order. */
export function promptHash(input: {
  agentId: string;
  paneId: string;
  hostKind: string;
  text: string;
  options: readonly RelayOption[];
  dialog?: boolean;
}): string {
  const canonical = JSON.stringify({
    agentId: input.agentId,
    paneId: input.paneId,
    hostKind: input.hostKind,
    text: input.text,
    options: input.options.map((option) => ({
      number: option.number,
      text: option.text,
      acceptsText: option.acceptsText,
      widensPermissions: option.widensPermissions,
    })),
    ...(input.dialog === true ? { dialog: true } : {}),
  });
  return createHash("sha256").update(canonical, "utf8").digest("hex");
}

/** Why a text answer may not be typed, or undefined when it is acceptable. */
export function relayTextProblem(text: string): string | undefined {
  if (typeof text !== "string") return "the text is not a string";
  if (text.trim() === "") return "the text is empty";
  if (!text.isWellFormed()) return "the text is not well-formed UTF-16";
  if (UNSAFE.test(text))
    return "the text has a newline, control or format character";
  if (COMMAND_START.test(text))
    return "the text starts with a tab or one of / ! # ? @";
  if (Buffer.byteLength(text, "utf8") > RELAY_TEXT_MAX_BYTES)
    return `the text is longer than ${RELAY_TEXT_MAX_BYTES} bytes`;
  return undefined;
}

/** Checks an answer against the captured prompt; undefined means it may be typed. */
export function checkAnswer(
  prompt: CapturedPrompt,
  answer: PromptAnswer,
): RelayRefusal | undefined {
  if (answer.kind === "esc") return undefined;
  const option = prompt.options.find(
    (candidate) => candidate.number === answer.number,
  );
  if (option === undefined) return "no_such_option";
  if (answer.kind === "option") return undefined;
  if (!option.acceptsText) return "no_text_option";
  if (relayTextProblem(answer.text) !== undefined) return "text_refused";
  return undefined;
}
