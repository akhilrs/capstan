import assert from "node:assert/strict";
import { test } from "node:test";
import { MAX_TASK_TITLE, normalizeTaskTitle } from "../src/task-text.js";

test("a multi-line title is folded into one line", () => {
  assert.equal(
    normalizeTaskTitle("  Fix\r\n\tthe\n\npoller \n"),
    "Fix the poller",
  );
});

test("other control characters are still refused", () => {
  for (const bad of ["bad\u0007", "a\u001bb", "a b", "a‎b"])
    assert.throws(() => normalizeTaskTitle(bad), /control characters/);
});

test("a title that is empty after folding is refused", () => {
  assert.throws(() => normalizeTaskTitle("\n\t\r "), /must not be empty/);
});

test("the cut applies after folding", () => {
  assert.equal(
    normalizeTaskTitle(`${"a".repeat(MAX_TASK_TITLE - 1)}\n\nb`),
    "a".repeat(MAX_TASK_TITLE - 1),
  );
});
