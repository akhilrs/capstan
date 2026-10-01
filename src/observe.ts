/**
 * What `cstan observe` returns: another agent's recent screen, cleaned so the
 * reader gets plain text. The text is the observed agent's own output and stays
 * unverified; this module only removes terminal control sequences and
 * characters, and caps the size.
 */
import { normalizeText } from "./text.js";

export const OBSERVE_DEFAULT_LINES = 40;
export const OBSERVE_MAX_LINES = 120;
export const OBSERVE_MAX_BYTES = 8000;
export const OBSERVE_RATE_LIMIT = 30;
const CUT_MARKER = "[earlier text cut]\n";

/** `lines` as the caller wrote it: ASCII digits, no leading zero, 1 to 120. Undefined means the default. */
export function parseObserveLines(text: string | undefined): number | null {
  if (text === undefined) return OBSERVE_DEFAULT_LINES;
  if (!/^[1-9][0-9]{0,2}$/.test(text)) return null;
  const lines = Number(text);
  return lines <= OBSERVE_MAX_LINES ? lines : null;
}

const ESC = "\u001b";
const SEQUENCES: readonly RegExp[] = [
  // OSC, ended by BEL or ST (ESC \ or the 8-bit ST), or cut off at the end.
  new RegExp(
    `(?:${ESC}\\]|\\u009d)[^\\u0007${ESC}\\u009c]*(?:\\u0007|${ESC}\\\\|\\u009c|$)`,
    "g",
  ),
  // DCS, SOS, PM and APC, ended by ST or cut off at the end.
  new RegExp(
    `(?:${ESC}[PX^_]|[\\u0090\\u0098\\u009e\\u009f])[\\s\\S]*?(?:${ESC}\\\\|\\u009c|$)`,
    "g",
  ),
  // CSI, 7-bit and 8-bit, complete or cut off at the end.
  new RegExp(`(?:${ESC}\\[|\\u009b)[0-?]*[ -/]*(?:[@-~]|$)`, "g"),
  // Two-character escapes and escapes with intermediates.
  new RegExp(`${ESC}[ -/]*[0-~]?`, "g"),
];

export function stripTerminalSequences(raw: string): string {
  let text = raw;
  for (const pattern of SEQUENCES) text = text.replace(pattern, "");
  return text;
}

/** The part of `text` that fits `maxBytes` UTF-8 bytes, taken from the end on a user-perceived character boundary. */
function tail(text: string, maxBytes: number): string {
  const segments = [
    ...new Intl.Segmenter(undefined, { granularity: "grapheme" }).segment(text),
  ].map((s) => s.segment);
  let bytes = 0;
  let start = segments.length;
  while (start > 0) {
    const size = Buffer.byteLength(segments[start - 1]!, "utf8");
    if (bytes + size > maxBytes) break;
    bytes += size;
    start -= 1;
  }
  return segments.slice(start).join("");
}

/** Plain text of a screen: sequences removed, line endings folded, control and format characters made spaces (LF kept), cut from the top when long. */
export function sanitizeScreen(raw: string): string {
  const clean = normalizeText(stripTerminalSequences(raw));
  if (Buffer.byteLength(clean, "utf8") <= OBSERVE_MAX_BYTES) return clean;
  const room = OBSERVE_MAX_BYTES - Buffer.byteLength(CUT_MARKER, "utf8");
  return `${CUT_MARKER}${tail(clean, room).trimStart()}`;
}
