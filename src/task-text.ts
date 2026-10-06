/** Validation of the task ref and title a worker is spawned with; shared by the spawn command, the launcher and the ledger. */
export const TASK_REF_PATTERN =
  /^[A-Za-z0-9][A-Za-z0-9._:-]{0,127}(\/[A-Za-z0-9][A-Za-z0-9._:-]{0,127})?$/;

export const MAX_TASK_TITLE = 200;

/** A spawn title for storage: runs of tab, CR and LF folded to one space, other control characters refused, whitespace folded, cut to 200 characters. */
export function normalizeTaskTitle(raw: string): string {
  const folded = raw.replace(/[\t\r\n]+/gu, " ");
  if (/[\p{Cc}\p{Cf}\p{Zl}\p{Zp}]/u.test(folded))
    throw new TypeError("the title must not contain control characters");
  const title = Array.from(folded.replace(/\s+/gu, " ").trim())
    .slice(0, MAX_TASK_TITLE)
    .join("")
    .trim();
  if (title === "") throw new TypeError("the title must not be empty");
  return title;
}
