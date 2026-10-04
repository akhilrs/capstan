/** Text helpers for launcher notes and refusals. */
/** Escape sequences (CSI with any parameter bytes, OSC, DCS and the other string forms even when unterminated, the 8-bit C1 forms, and two-byte escapes), control and format characters, lone surrogates, line separators and runs of blanks are removed or become one space. The text is cut at a grapheme boundary and kept within `maxLength` UTF-16 units, so combining marks cannot stretch it; a first grapheme longer than that leaves nothing. */
export function oneLine(text: string, maxLength: number): string {
  const clean = text
    .replace(
      /(?:\u001b\]|\u009d)[^\u0007\u001b\u009c]*(?:\u0007|\u001b\\|\u009c)?/g,
      "",
    )
    .replace(
      /(?:\u001b[PX^_]|[\u0090\u0098\u009e\u009f])[^\u001b\u009c]*(?:\u001b\\|\u009c)?/g,
      "",
    )
    .replace(/(?:\u001b\[|\u009b)[0-?]*[ -/]*[@-~]/g, "")
    .replace(/\u001b[ -/]*[0-~]/g, "")
    .replace(/\p{Cs}/gu, "")
    .replace(/[\p{Cc}\p{Cf}\p{Zl}\p{Zp}\s]+/gu, " ")
    .trim();
  let result = "";
  for (const part of new Intl.Segmenter(undefined, {
    granularity: "grapheme",
  }).segment(clean)) {
    if (result.length + part.segment.length > maxLength) break;
    result += part.segment;
  }
  return result.trim();
}
