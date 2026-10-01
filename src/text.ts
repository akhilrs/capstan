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

/**
 * One line: normalized, whitespace folded, trimmed and cut to at most
 * `maxPoints` code points with an ellipsis. `empty` stands for text that is
 * blank afterwards.
 */
export function oneLine(
  text: string,
  maxPoints: number,
  empty: string,
): string {
  const folded = normalizeText(text).replace(/\s+/g, " ").trim();
  if (folded === "") return empty;
  const points = Array.from(folded);
  return points.length > maxPoints
    ? `${points.slice(0, maxPoints - 1).join("")}\u2026`
    : folded;
}
