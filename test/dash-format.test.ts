import assert from "node:assert/strict";
import { test } from "node:test";
import {
  age,
  cellWidth,
  clean,
  commitShort,
  fit,
  pushSample,
  sparkline,
  truncate,
} from "../src/dash/format.js";

test("clean removes control, format and separator characters", () => {
  assert.equal(clean("a\u001b[31mb‮c\nd"), "a [31mb c d");
  assert.equal(clean(undefined), "");
});

test("width counts wide characters twice and truncation never splits one", () => {
  assert.equal(cellWidth("abc"), 3);
  assert.equal(cellWidth("日本"), 4);
  assert.equal(truncate("abcdef", 4), "abc…");
  assert.equal(truncate("日本語テキスト", 5), "日本…");
  assert.equal(truncate("abc", 3), "abc");
  assert.equal(truncate("abc", 0), "");
  assert.equal(cellWidth(fit("日本", 7)), 7);
  assert.equal(cellWidth(fit("日本語テキスト", 7)), 7);
  assert.equal(fit("ab", 4), "ab  ");
  assert.equal(truncate("ééé", 2), "é…");
});

test("age picks a unit and refuses unreadable times", () => {
  const now = Date.parse("2026-01-02T00:00:00Z");
  assert.equal(age("2026-01-01T23:59:30Z", now), "30s");
  assert.equal(age("2026-01-01T23:50:00Z", now), "10m");
  assert.equal(age("2026-01-01T20:00:00Z", now), "4h");
  assert.equal(age("2025-12-30T00:00:00Z", now), "3d");
  assert.equal(age("2026-01-02T00:00:10Z", now), "0s");
  assert.equal(age("nonsense", now), "-");
  assert.equal(age(null, now), "-");
});

test("sparkline scales to the largest value, keeps the newest samples and handles all zeros", () => {
  assert.equal(sparkline([0, 0, 0], 10), "▁▁▁");
  assert.equal(sparkline([0, 7], 10), "▁█");
  assert.equal(sparkline([1, 2, 3, 4, 5], 3), sparkline([3, 4, 5], 3));
  assert.equal(sparkline([], 5), "");
  assert.deepEqual(pushSample([1, 2, 3], 4, 3), [2, 3, 4]);
});

test("commitShort shortens to seven characters", () => {
  assert.equal(commitShort("0123456789abcdef"), "0123456");
  assert.equal(commitShort(null), "-");
});
