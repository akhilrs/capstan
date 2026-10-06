/** Validation of the task ref and title a worker is spawned with; shared by the spawn command, the launcher and the ledger. */
export const TASK_REF_PATTERN =
  /^[A-Za-z0-9][A-Za-z0-9._:-]{0,127}(\/[A-Za-z0-9][A-Za-z0-9._:-]{0,127})?$/;

export const MAX_TASK_TITLE = 200;

/** A spawn title for storage: refused when it holds control characters, whitespace folded, cut to 200 characters. */
export function normalizeTaskTitle(raw: string): string {
  if (/[\p{Cc}\p{Cf}\p{Zl}\p{Zp}]/u.test(raw))
    throw new TypeError("the title must not contain control characters");
  const title = Array.from(raw.replace(/\s+/gu, " ").trim())
    .slice(0, MAX_TASK_TITLE)
    .join("")
    .trim();
  if (title === "") throw new TypeError("the title must not be empty");
  return title;
}
