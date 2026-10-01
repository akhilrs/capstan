const LEFTOVER_CONTROL =
  /[\u0000-\u0008\u000b\u000c\u000e-\u001f\u007f-\u009f]/;
const ESCAPE_SEQUENCE =
  /(?:\u001b(?:\][^\u0007\u001b]*(?:\u0007|\u001b\\)|\[[0-?]*[ -/]*[@-~]|[ -/]*[0-Z\\^-~])|\u009b[0-?]*[ -/]*[@-~])/;
const ESCAPE_SEQUENCES = new RegExp(ESCAPE_SEQUENCE.source, "g");
const RULE_LINE = /^─{10,}$/;
const PROMPT_SYMBOLS: readonly string[] = ["❯", "$", "#", "%"];

export const TRUST_YES = "Yes, I trust this folder";
export const TRUST_NO = "No, exit";
export const CODEX_TRUST_YES = "Trust and continue";
export const CODEX_TRUST_NO = "Quit";

/** The two option texts of a host's trust dialog, or undefined for a host that shows none. */
export function trustTexts(
  kind: string,
): { readonly yes: string; readonly no: string } | undefined {
  if (kind === "claude") return { yes: TRUST_YES, no: TRUST_NO };
  if (kind === "codex") return { yes: CODEX_TRUST_YES, no: CODEX_TRUST_NO };
  return undefined;
}

export function parseTrustDialogOf(
  kind: string,
  plainScreen: string,
): TrustDialog | undefined {
  if (kind === "claude") return parseTrustDialog(plainScreen);
  if (kind === "codex") return parseCodexTrustDialog(plainScreen);
  return undefined;
}

export function stripAnsi(text: string): string {
  return text.replace(ESCAPE_SEQUENCES, "");
}

function splitLines(text: string): string[] {
  return text.split(/\r\n|\n|\r/);
}

interface StyledCharacter {
  readonly character: string;
  readonly dim: boolean;
}

/** Visible characters of one raw line with the dim attribute the terminal would draw them with. */
function styledCharacters(line: string): StyledCharacter[] {
  const result: StyledCharacter[] = [];
  let dim = false;
  let index = 0;
  while (index < line.length) {
    const sequence = new RegExp(`^${ESCAPE_SEQUENCE.source}`).exec(
      line.slice(index),
    );
    if (sequence) {
      const sgr = /^\u001b\[([0-9;:]*)m$/.exec(sequence[0]);
      if (sgr) {
        const params = (sgr[1] ?? "").split(";");
        for (let cursor = 0; cursor < params.length; cursor += 1) {
          const code = params[cursor] === "" ? 0 : Number(params[cursor]);
          if (code === 0 || code === 22) dim = false;
          else if (code === 2) dim = true;
          else if (code === 38 || code === 48 || code === 58) {
            cursor +=
              params[cursor + 1] === "5"
                ? 2
                : params[cursor + 1] === "2"
                  ? 4
                  : 0;
          }
        }
      }
      index += sequence[0].length;
      continue;
    }
    const character = String.fromCodePoint(line.codePointAt(index)!);
    result.push({ character, dim });
    index += character.length;
  }
  return result;
}

function isBlank(character: string): boolean {
  return /^[\s ]$/u.test(character);
}

/**
 * The text typed on the input line of an agent or shell, "" when it is empty,
 * or undefined when the screen does not show an input line that can be read.
 * The screen must be an ANSI read, because an empty Claude input shows a dim
 * placeholder that a plain read cannot tell from typed text.
 */
export function extractInputLine(
  kind: string,
  ansiScreen: string,
): string | undefined {
  const raw = splitLines(ansiScreen);
  const plain = raw.map(stripAnsi);
  if (kind === "shell") {
    let last = plain.length - 1;
    while (last >= 0 && plain[last]!.trim() === "") last -= 1;
    if (last < 0) return undefined;
    const match = /^❯(?:[  ](.*))?$/u.exec(plain[last]!.trimEnd());
    if (LEFTOVER_CONTROL.test(plain[last]!)) return undefined;
    return match ? (match[1] ?? "").trimEnd() : undefined;
  }
  if (kind === "codex") return codexInputLine(raw, plain);
  if (kind === "omp") return ompInputLine(plain);
  if (kind !== "claude") return undefined;

  const rules: number[] = [];
  plain.forEach((line, index) => {
    if (RULE_LINE.test(line.trim())) rules.push(index);
  });
  if (rules.length < 2) return undefined;
  const bottom = rules[rules.length - 1]!;
  const top = rules[rules.length - 2]!;
  if (bottom - top < 2) return undefined;
  if (plain[top]!.trim().length !== plain[bottom]!.trim().length)
    return undefined;
  if (plain.slice(top, bottom + 1).some((line) => LEFTOVER_CONTROL.test(line)))
    return undefined;
  const first = plain[top + 1]!;
  if (!/^\s*❯(?:[  ]|$)/u.test(first)) return undefined;

  const characters = styledCharacters(raw[top + 1]!);
  const marker = characters.findIndex((entry) => entry.character === "❯");
  let start = marker + 1;
  if (
    characters[start] &&
    (characters[start]!.character === " " ||
      characters[start]!.character === " ")
  )
    start += 1;
  const remainder = characters.slice(start);
  const typed = remainder.filter((entry) => !isBlank(entry.character));
  const placeholder = typed.length > 0 && typed.every((entry) => entry.dim);
  const firstText =
    placeholder || typed.length === 0
      ? ""
      : remainder
          .map((entry) => entry.character)
          .join("")
          .trimEnd();
  const continuation = raw.slice(top + 2, bottom).map((line, offset) => {
    const visible = styledCharacters(line).filter(
      (entry) => !isBlank(entry.character),
    );
    return visible.length > 0 && visible.every((entry) => entry.dim)
      ? ""
      : plain[top + 2 + offset]!.trimEnd();
  });
  return [firstText, ...continuation].join("\n").trimEnd();
}

/** True when the line sets a foreground colour (SGR 38), not a background or underline colour that takes the same sub-parameters. */
function setsForeground(line: string): boolean {
  for (const sequence of line.matchAll(/\u001b\[([0-9;:]*)m/g)) {
    const params = (sequence[1] ?? "").split(";");
    for (let cursor = 0; cursor < params.length; cursor += 1) {
      const code = params[cursor] === "" ? 0 : Number(params[cursor]);
      if (code === 38) return true;
      if (code === 48 || code === 58)
        cursor +=
          params[cursor + 1] === "5" ? 2 : params[cursor + 1] === "2" ? 4 : 0;
    }
  }
  return false;
}

/**
 * Codex draws the input as `› text` (a dim placeholder when empty), then any
 * further typed lines, then a status line that starts with a coloured span.
 */
function codexInputLine(
  raw: readonly string[],
  plain: readonly string[],
): string | undefined {
  if (plain.some((line) => line.trim().startsWith("enter continue")))
    return undefined;
  let marker = -1;
  plain.forEach((line, index) => {
    if (/^›(?: |$)/u.test(line.trimEnd())) marker = index;
  });
  if (marker < 0) return undefined;
  let status = -1;
  for (let index = marker + 1; index < raw.length; index += 1)
    if (setsForeground(raw[index]!)) {
      status = index;
      break;
    }
  if (status < 0) return undefined;
  if (plain.slice(marker, status).some((line) => LEFTOVER_CONTROL.test(line)))
    return undefined;
  const characters = styledCharacters(raw[marker]!);
  const at = characters.findIndex((entry) => entry.character === "›");
  if (at < 0) return undefined;
  const remainder = characters.slice(at + 2);
  const typed = remainder.filter((entry) => !isBlank(entry.character));
  const placeholder = typed.length > 0 && typed.every((entry) => entry.dim);
  const first =
    placeholder || typed.length === 0
      ? ""
      : remainder
          .map((entry) => entry.character)
          .join("")
          .trimEnd();
  const rest = plain
    .slice(marker + 1, status)
    .map((line) => line.replace(/^ {2}/, "").trimEnd());
  return [first, ...rest].join("\n").trimEnd();
}

/** OMP's input is the last line, `╰─ text`; nothing is drawn when it is empty. */
function ompInputLine(plain: readonly string[]): string | undefined {
  let last = plain.length - 1;
  while (last >= 0 && plain[last]!.trim() === "") last -= 1;
  if (last < 0 || LEFTOVER_CONTROL.test(plain[last]!)) return undefined;
  const match = /^╰─(?: (.*))?$/u.exec(plain[last]!.trimEnd());
  return match ? (match[1] ?? "").trimEnd() : undefined;
}

/** A pane the adapter has just created shows the operator's own prompt: only a last line that is exactly one prompt symbol proves that nothing is typed. */
export function freshPromptReady(plainScreen: string): boolean {
  const lines = splitLines(plainScreen);
  let last = lines.length - 1;
  while (last >= 0 && lines[last]!.trim() === "") last -= 1;
  return (
    last >= 0 &&
    !LEFTOVER_CONTROL.test(lines[last]!) &&
    PROMPT_SYMBOLS.includes(lines[last]!.trim())
  );
}

export interface TrustOption {
  readonly text: string;
  readonly selected: boolean;
}

export type TrustDialog =
  | {
      readonly kind: "dialog";
      readonly path: string;
      readonly options: readonly TrustOption[];
      readonly selectedIndex: number | undefined;
      readonly confirmIsLastLine: boolean;
    }
  | { readonly kind: "wrapped_path" };

export function parseTrustDialog(plainScreen: string): TrustDialog | undefined {
  const lines = splitLines(plainScreen);
  let title = -1;
  lines.forEach((line, index) => {
    if (line.trim() === "Accessing workspace:") title = index;
  });
  if (title < 0) return undefined;
  let pathIndex = title + 1;
  while (pathIndex < lines.length && lines[pathIndex]!.trim() === "")
    pathIndex += 1;
  if (pathIndex >= lines.length) return undefined;
  if (lines[pathIndex + 1] !== undefined && lines[pathIndex + 1]!.trim() !== "")
    return { kind: "wrapped_path" };
  let confirm = -1;
  lines.forEach((line, index) => {
    if (index > pathIndex && line.trim().startsWith("Enter to confirm"))
      confirm = index;
  });
  if (confirm < 0) return undefined;
  let cursor = confirm - 1;
  while (cursor > pathIndex && lines[cursor]!.trim() === "") cursor -= 1;
  const options: TrustOption[] = [];
  while (cursor > pathIndex && lines[cursor]!.trim() !== "") {
    const trimmed = lines[cursor]!.trim();
    const selected = trimmed.startsWith("❯");
    options.unshift({ text: trimmed.replace(/^❯\s*/u, ""), selected });
    cursor -= 1;
  }
  const selectedIndexes = options.flatMap((option, index) =>
    option.selected ? [index] : [],
  );
  let lastNonEmpty = lines.length - 1;
  while (lastNonEmpty >= 0 && lines[lastNonEmpty]!.trim() === "")
    lastNonEmpty -= 1;
  return {
    kind: "dialog",
    path: lines[pathIndex]!.trim(),
    options,
    selectedIndex:
      selectedIndexes.length === 1 ? selectedIndexes[0] : undefined,
    confirmIsLastLine: lastNonEmpty === confirm,
  };
}

/** Codex: `Folder access`, the path, a question, `› 1. Trust and continue`, `2. Quit`, `enter continue · esc quit`. */
export function parseCodexTrustDialog(
  plainScreen: string,
): TrustDialog | undefined {
  const lines = splitLines(plainScreen);
  let title = -1;
  lines.forEach((line, index) => {
    if (line.trim() === "Folder access") title = index;
  });
  if (title < 0) return undefined;
  let pathIndex = title + 1;
  while (pathIndex < lines.length && lines[pathIndex]!.trim() === "")
    pathIndex += 1;
  if (pathIndex >= lines.length) return undefined;
  if (lines[pathIndex + 1] !== undefined && lines[pathIndex + 1]!.trim() !== "")
    return { kind: "wrapped_path" };
  let confirm = -1;
  lines.forEach((line, index) => {
    if (index > pathIndex && line.trim().startsWith("enter continue"))
      confirm = index;
  });
  if (confirm < 0) return undefined;
  const options: TrustOption[] = [];
  for (let index = pathIndex + 1; index < confirm; index += 1) {
    const trimmed = lines[index]!.trim();
    const match = /^(›\s*)?\d+\.\s+(.+)$/u.exec(trimmed);
    if (match)
      options.push({ text: match[2]!, selected: match[1] !== undefined });
  }
  const selectedIndexes = options.flatMap((option, index) =>
    option.selected ? [index] : [],
  );
  let lastNonEmpty = lines.length - 1;
  while (lastNonEmpty >= 0 && lines[lastNonEmpty]!.trim() === "")
    lastNonEmpty -= 1;
  return {
    kind: "dialog",
    path: lines[pathIndex]!.trim(),
    options,
    selectedIndex:
      selectedIndexes.length === 1 ? selectedIndexes[0] : undefined,
    confirmIsLastLine: lastNonEmpty === confirm,
  };
}
