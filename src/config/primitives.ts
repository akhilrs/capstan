/** The small readers and validators every capstan.toml section uses. */
import path from "node:path";
import {
  CONFIG_FILE_NAME,
  ConfigError,
  ENV_NAME_PATTERN,
  MAX_PASSED_ENV_NAMES,
  RESERVED_ENV_NAMES,
  RESERVED_ENV_PREFIXES,
} from "./types.js";

export const MAX_FILE_BYTES = 64 * 1024;
export const MAX_PROMPT_CHARS = MAX_FILE_BYTES;
export const MAX_LIST_ENTRIES = 64;
export const MAX_DENY_ENTRIES = 128;
export const MAX_ENTRY_CHARS = 200;
export const NAME_PATTERN = /^[a-z][a-z0-9-]{0,31}$/;
export const SESSION_PATTERN = /^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$/;
export const COMMAND_PATTERN = /^(?:\.\/)?[A-Za-z0-9_/][A-Za-z0-9._/-]{0,199}$/;
export const UNSAFE_CHARACTERS = /[\p{Cc}\p{Cf}\p{Zl}\p{Zp}]/u;
export const CREDENTIAL_SHAPES: readonly RegExp[] = [
  /\bsk-[A-Za-z0-9_-]{8,}/,
  /\bgh[pousr]_[A-Za-z0-9]{8,}/,
  /\bAKIA[A-Z0-9]{8,}/,
  /\bBearer[ \t]+(?=[A-Za-z0-9._~+/=-]*\d)[A-Za-z0-9._~+/=-]{16,}/,
  /-----BEGIN [A-Z ]*PRIVATE KEY-----/,
];

export type Table = Record<string, unknown>;

export const PRINTABLE_LINE = /^[\x20-\x7e]*$/;

export function isExecutablePath(command: string): boolean {
  return (
    COMMAND_PATTERN.test(command) &&
    !command.endsWith("/") &&
    ![".", ".."].includes(path.basename(command))
  );
}

export function validatedNames(
  table: Table,
  at: string,
  noun: string,
): string[] {
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

export function rejectUnknownKeys(
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

export function isTable(value: unknown): value is Table {
  if (value === null || typeof value !== "object" || Array.isArray(value))
    return false;
  const prototype = Object.getPrototypeOf(value);
  return prototype === null || prototype === Object.prototype;
}

export function optionalTable(value: unknown, at: string): Table {
  if (value === undefined) return {};
  return requiredTable(value, at);
}

export function requiredTable(value: unknown, at: string): Table {
  if (!isTable(value)) throw new ConfigError(`${at} must be a table`);
  return value;
}

export function requiredString(
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

export function assertSafeText(
  value: string,
  at: string,
  multiline: boolean,
): void {
  const checked = multiline ? value.replace(/[\n\t\u200c\u200d]/g, "") : value;
  if (UNSAFE_CHARACTERS.test(checked))
    throw new ConfigError(
      `${at} contains control, format or line-separator characters`,
    );
}

export function optionalString(
  value: unknown,
  at: string,
  maxChars: number,
): string | null {
  return value === undefined ? null : requiredString(value, at, maxChars);
}

export function optionalBoolean(
  value: unknown,
  at: string,
  fallback: boolean,
): boolean {
  if (value === undefined) return fallback;
  if (typeof value !== "boolean")
    throw new ConfigError(`${at} must be true or false`);
  return value;
}

export function optionalInteger(
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

export function enumValue<const T extends readonly string[]>(
  value: unknown,
  at: string,
  allowed: T,
): T[number] {
  if (typeof value !== "string" || !allowed.includes(value))
    throw new ConfigError(`${at} must be one of ${allowed.join(", ")}`);
  return value;
}

/** The names are written by the operator; values never appear in the file. */
export function passedEnvironmentNames(value: unknown): string[] {
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

export function stringList(
  value: unknown,
  at: string,
  maxEntries = MAX_LIST_ENTRIES,
): string[] {
  if (value === undefined) return [];
  if (!Array.isArray(value))
    throw new ConfigError(`${at} must be an array of strings`);
  if (value.length > maxEntries)
    throw new ConfigError(`${at} exceeds ${maxEntries} entries`);
  return value.map((entry, index) => {
    const text = requiredString(entry, `${at}[${index}]`, MAX_ENTRY_CHARS);
    guardCredentialShape(text, `${at}[${index}]`);
    if (text.startsWith("-"))
      throw new ConfigError(`${at}[${index}] must not start with a dash`);
    return text;
  });
}

export function guardCredentialShape(text: string, at: string): void {
  if (CREDENTIAL_SHAPES.some((shape) => shape.test(text)))
    throw new ConfigError(
      `${at} looks like a credential; ${CONFIG_FILE_NAME} must not hold secrets`,
    );
}
