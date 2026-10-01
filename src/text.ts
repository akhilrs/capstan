/**
 * Normalizes text that one agent wrote and another will read: CR, CRLF, NEL
 * and the Unicode line and paragraph separators become LF, control and format
 * characters other than LF become spaces, and the result is trimmed. It never
 * cuts; callers refuse text that is too long.
 */
export function normalizeText(text: string): string {
  return text
    .replace(/\r\n?|\u0085|\u2028|\u2029/g, "\n")
    .replace(
      /[\p{Cc}\p{Cf}\p{Cs}\p{Zl}\p{Zp}\p{Noncharacter_Code_Point}]/gu,
      (c) => (c === "\n" ? c : " "),
    )
    .trim();
}
