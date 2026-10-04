/** Name, label, shell-quoting and safe-text checks the adapter applies before anything reaches Herdr. */
import { InvalidArgumentError } from "./adapter-errors.js";

export const MAX_TEXT_BYTES = 16 * 1024;
export const NAME_PATTERN = /^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$/;

/** True for a name Herdr and this adapter accept for an agent. */
export function isAgentName(value: unknown): value is string {
  return typeof value === "string" && NAME_PATTERN.test(value);
}

export const BRANCH_PATTERN = /^[A-Za-z0-9][A-Za-z0-9._/-]{0,99}$/;
export const WORKSPACE_PATTERN = /^w[0-9A-Za-z]+$/;
export const PANE_PATTERN = /^w[0-9A-Za-z]+:p[0-9A-Za-z]+$/;
export const TAB_PATTERN = /^w[0-9A-Za-z]+:t[0-9A-Za-z]+$/;
export const SIMPLE_VALUE = /^[A-Za-z0-9_@%+=:,./-]*$/;
/** The same characters the controller refuses in a message body. */
export const UNSAFE_TEXT =
  /[\p{Cc}\p{Cf}\p{Zl}\p{Zp}\p{Noncharacter_Code_Point}]/u;
/** HOME, PATH and TERM are shell-quoted, so only blank or unsafe-to-show text is refused. */
function isQuotableValue(value: unknown): value is string {
  return (
    typeof value === "string" && value.trim() !== "" && !UNSAFE_TEXT.test(value)
  );
}
const ALLOWED_TEXT_CHARACTERS = /[\n\t\u200c\u200d]/g;
/** In Claude Code a first character of / ! # ? or @ (or a tab) acts on the input box instead of adding text. */
export const COMMAND_START = /^(?:\t|\s*[/!#?@])/;

export function isSafeText(text: unknown): text is string {
  return (
    typeof text === "string" &&
    text.trim() !== "" &&
    text.isWellFormed() &&
    !UNSAFE_TEXT.test(text.replace(ALLOWED_TEXT_CHARACTERS, ""))
  );
}
export const ENVIRONMENT_KEY = /^[A-Z_][A-Z0-9_]*$/;
export const CONTROL_CHARACTERS = /\p{Cc}/u;

export function requireMatch(
  value: unknown,
  pattern: RegExp,
  label: string,
): string {
  if (typeof value !== "string" || !pattern.test(value))
    throw new InvalidArgumentError(`${label} is not acceptable`);
  return value;
}

/** A display label: printable text on one line, at most 64 characters. */
export function requireLabel(value: unknown): string {
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

export function requireQuotable(value: unknown, label: string): string {
  if (!isQuotableValue(value))
    throw new InvalidArgumentError(`${label} is not acceptable`);
  return value;
}

export function shellQuote(value: string): string {
  return `'${value.replaceAll("'", "'\\''")}'`;
}
