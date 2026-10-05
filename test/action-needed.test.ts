import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import test from "node:test";

/* Every place the controller queues a notice must decide whether the PM has to act. The decision is a required
   argument (the compiler rejects a call without it); this test also rejects one that is not written down at the
   call site, so a new notice kind cannot inherit a default or be classified by guessing at its text. */

const controllerDirectory = path.resolve(
  import.meta.dirname,
  "..",
  "..",
  "src",
  "controller",
);

/** The queueing entry points and the number of arguments each takes, the last being the action decision. */
const QUEUEING_CALLS: Readonly<Record<string, number>> = {
  insertQueuedMessage: 6,
  noticeToPm: 3,
  noticeToAgent: 4,
  queuePlanNotice: 4,
  "#queuePmNotice": 6,
};

/** The operator and restart paths reach the PM through notifyPm, which carries the same decision. */
const NOTIFY_FILES = ["restart.ts", "operator.ts"];

function topLevelArguments(source: string, open: number): string[] {
  const args: string[] = [];
  let depth = 0;
  let start = open + 1;
  let quote: string | null = null;
  for (let i = open; i < source.length; i += 1) {
    const char = source[i]!;
    if (quote !== null) {
      if (char === "\\") i += 1;
      else if (char === quote) quote = null;
      continue;
    }
    if (char === '"' || char === "'" || char === "`") quote = char;
    else if ("([{".includes(char)) depth += 1;
    else if (")]}".includes(char)) {
      depth -= 1;
      if (depth === 0) {
        const last = source.slice(start, i).trim();
        if (last !== "") args.push(last);
        return args;
      }
    } else if (char === "," && depth === 1) {
      args.push(source.slice(start, i).trim());
      start = i + 1;
    }
  }
  throw new Error("unbalanced call");
}

test("every controller notice call states whether it needs the PM's action", () => {
  let calls = 0;
  for (const file of fs.readdirSync(controllerDirectory)) {
    if (!file.endsWith(".ts")) continue;
    const source = fs.readFileSync(
      path.join(controllerDirectory, file),
      "utf8",
    );
    for (const [name, arity] of Object.entries(QUEUEING_CALLS)) {
      const pattern = new RegExp(`\\.${name.replace("#", "#?")}\\(`, "g");
      for (const match of source.matchAll(pattern)) {
        const args = topLevelArguments(
          source,
          match.index + match[0].length - 1,
        );
        const line = source.slice(0, match.index).split("\n").length;
        const where = `${file}:${line} ${name}`;
        assert.equal(
          args.length,
          arity,
          `${where} has ${args.length} arguments`,
        );
        // A literal at the call site, or a pass-through of the caller's own parameter.
        assert.match(
          args[arity - 1]!,
          /^(true|false|actionNeeded|input\.actionNeeded === true)$/,
          `${where} must pass an explicit action decision`,
        );
        calls += 1;
      }
    }
  }
  for (const file of NOTIFY_FILES) {
    const source = fs.readFileSync(
      path.join(controllerDirectory, "..", file),
      "utf8",
    );
    for (const match of source.matchAll(/\.notifyPm\??\.?\(/g)) {
      const args = topLevelArguments(source, match.index + match[0].length - 1);
      const where = `${file}:${source.slice(0, match.index).split("\n").length} notifyPm`;
      assert.equal(args.length, 2, `${where} has ${args.length} arguments`);
      assert.match(
        args[1]!,
        /^(true|false|result\.outcome !== "ok")$/,
        `${where} must pass an explicit action decision`,
      );
      calls += 1;
    }
  }
  assert.ok(calls >= 30, `only ${calls} notice calls were found`);
});
