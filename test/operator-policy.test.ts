import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { test } from "node:test";
import {
  MAX_OPERATOR_COMMAND_BYTES,
  OPERATOR_ALWAYS_APPROVAL,
  OPERATOR_AUTO_ALLOWLIST,
  autoApproveRuleProblem,
  autoDecision,
  classifyCommand,
  commandHash,
  hashPrefix,
  matchesAutoApprove,
  normalizeCommand,
  normalizeReason,
  tokenize,
} from "../src/operator-policy.js";

const EXACT = ["ls -l", "git rev-parse --short HEAD", "pwd", "cstan status"];

test("ls -l and git rev-parse --short HEAD match their exact rules", () => {
  const ls = matchesAutoApprove("ls -l", EXACT, []);
  assert.equal(ls.auto, true);
  assert.equal(ls.rule, "ls -l");
  assert.equal(
    matchesAutoApprove("git rev-parse --short HEAD", EXACT, []).auto,
    true,
  );
});

const NOT_AUTO = [
  "git status",
  "git diff",
  "git show HEAD:.env",
  "git log --ext-diff",
  "git status; rm x",
  "ls $(x)",
  "ls > f",
  'sh -c "ls"',
  "FOO=1 ls",
  "git -c core.pager=x rev-parse",
  "git push",
  "rm -rf x",
  "git reset --hard",
  "ls\nrm x",
  "ls\u0000",
  "ls \u001b[31m",
  "git diff --output=/p",
  "git grep -Ocmd",
  "npm run build --script-shell=x",
  "npm test",
  "make",
  "/bin/rm x",
  "./rm x",
  "git clean -xdf",
  "git push --force=1",
  "git push --exec=x",
  "git branch -D x",
  "git branch -d x",
  "git branch d",
  "x --delete",
];

test("commands off the allowlist are never auto-approved, even when written as rules", () => {
  const prefixes = NOT_AUTO.map((text) => text.split(" ")[0]!);
  for (const text of NOT_AUTO) {
    assert.equal(
      matchesAutoApprove(text, [text], prefixes).auto,
      false,
      JSON.stringify(text),
    );
    assert.notEqual(autoApproveRuleProblem(text), null, JSON.stringify(text));
  }
});

test("the empty rule lists auto-approve nothing", () => {
  for (const text of ["ls -l", "pwd", "git rev-parse --short HEAD"])
    assert.equal(matchesAutoApprove(text, [], []).auto, false);
});

test("an exact rule matches the whole command and nothing longer or spaced differently", () => {
  assert.equal(matchesAutoApprove("ls -l src", ["ls -l"], []).auto, false);
  assert.equal(matchesAutoApprove("ls  -l", ["ls -l"], []).auto, false);
  assert.equal(matchesAutoApprove("ls -la", ["ls -l"], []).auto, false);
});

test("a prefix rule accepts only safe positional tokens after whole leading tokens", () => {
  assert.equal(matchesAutoApprove("ls -l src/lib", [], ["ls -l"]).auto, true);
  assert.equal(matchesAutoApprove("ls src", [], ["ls"]).auto, true);
  assert.equal(matchesAutoApprove("ls -l -a", [], ["ls -l"]).auto, false);
  assert.equal(matchesAutoApprove("ls -l a=b", [], ["ls -l"]).auto, false);
  assert.equal(matchesAutoApprove("ls -l ~/x", [], ["ls -l"]).auto, false);
  assert.equal(matchesAutoApprove("ls -lx", [], ["ls -l"]).auto, false);
  assert.equal(matchesAutoApprove("lsx", [], ["ls"]).auto, false);
  assert.equal(
    matchesAutoApprove(
      "git rev-parse --short HEAD main",
      [],
      ["git rev-parse --short HEAD"],
    ).auto,
    false,
    "a git rule accepts no argument beyond the allowlist",
  );
  assert.equal(
    matchesAutoApprove("git ls-files src", [], ["git ls-files"]).auto,
    false,
  );
});

test("-d and d are different tokens and both git branch forms need approval", () => {
  const dash = tokenize("git branch -d x");
  const word = tokenize("git branch d");
  assert.ok(!dash.words.includes("d"));
  assert.ok(dash.letters.includes("d"));
  assert.ok(word.words.includes("d"));
  assert.ok(!word.letters.includes("d"));
  assert.ok(classifyCommand("git branch -d x").alwaysApproval.length > 0);
  assert.ok(classifyCommand("git branch d").alwaysApproval.length > 0);
});

test("the denylist catches aliases, --flag=value forms and combined short flags", () => {
  const rows: Array<[string, string]> = [
    ["/bin/rm x", "rm"],
    ["./rm x", "rm"],
    ["x --force=1", "force"],
    ["x --delete", "delete"],
    ["rm -rf x", "-r"],
    ["git clean -xdf", "-x"],
    ["git clean -xdf", "-d"],
    ["ls -fd", "-f"],
    ["git branch -D x", "-D"],
    ["git diff -Ocmd", "-O"],
    ["x --output=/p", "output"],
    ["x --ext-diff", "ext-diff"],
    ["x --script-shell=x", "script-shell"],
    ["x -o=y", "-o"],
  ];
  for (const [text, hit] of rows)
    assert.ok(
      classifyCommand(text).alwaysApproval.includes(hit),
      `${text} should hit ${hit}`,
    );
});

test("every denylist word is flagged by classifyCommand", () => {
  for (const word of OPERATOR_ALWAYS_APPROVAL)
    assert.ok(classifyCommand(`x ${word}`).alwaysApproval.includes(word), word);
});

test("the allowlist never accepts a denylist word or a dangerous letter", () => {
  for (const entry of OPERATOR_AUTO_ALLOWLIST) {
    const text = [entry.command, entry.subcommand, ...entry.options]
      .filter((part) => part !== undefined)
      .join(" ");
    assert.deepEqual(classifyCommand(text).alwaysApproval, [], text);
    assert.equal(autoApproveRuleProblem(text), null, text);
  }
});

test("simple means one line of the safe character set", () => {
  assert.equal(classifyCommand("ls -l /a/b_c.d").simple, true);
  assert.equal(classifyCommand("ls\nls").simple, false);
  assert.equal(classifyCommand("ls;ls").simple, false);
  assert.equal(classifyCommand("ls $(x)").simple, false);
  assert.equal(classifyCommand("ls *").simple, false);
  assert.equal(classifyCommand("ls\tx").simple, false);
});

test("a restart is never auto-approved", () => {
  assert.equal(autoDecision("restart", "ls -l", ["ls -l"], []).auto, false);
  assert.equal(autoDecision("command", "ls -l", ["ls -l"], []).auto, true);
});

test("commandHash is sha256 of the canonical payload and differs for one added space", () => {
  const payload = { kind: "command", command: "ls -l", forceRestart: false };
  const expected = createHash("sha256")
    .update(JSON.stringify(["command", "ls -l", false]), "utf8")
    .digest("hex");
  assert.equal(commandHash(payload), expected);
  assert.equal(commandHash({ ...payload }), expected);
  assert.notEqual(commandHash({ ...payload, command: "ls -l " }), expected);
  assert.equal(hashPrefix(expected), expected.slice(0, 12));
  assert.equal(hashPrefix(expected).length, 12);
});

test("commandHash differs when the force flag or the kind differs", () => {
  const restart = { kind: "restart", command: "", forceRestart: false };
  assert.notEqual(
    commandHash(restart),
    commandHash({ ...restart, forceRestart: true }),
  );
  assert.notEqual(
    hashPrefix(commandHash(restart)),
    hashPrefix(commandHash({ ...restart, forceRestart: true })),
  );
  assert.notEqual(
    commandHash(restart),
    commandHash({ ...restart, kind: "command" }),
  );
});

test("normalizeCommand returns the text as sent and rejects empty, long and control text", () => {
  const sent = "ls -l\n\tx ";
  assert.deepEqual(normalizeCommand(sent), { ok: true, text: sent });
  assert.deepEqual(normalizeCommand(""), { ok: false, code: "empty" });
  assert.deepEqual(normalizeCommand("  \n"), { ok: false, code: "empty" });
  assert.deepEqual(normalizeCommand("a".repeat(MAX_OPERATOR_COMMAND_BYTES)), {
    ok: true,
    text: "a".repeat(MAX_OPERATOR_COMMAND_BYTES),
  });
  assert.deepEqual(
    normalizeCommand("a".repeat(MAX_OPERATOR_COMMAND_BYTES + 1)),
    { ok: false, code: "too_long" },
  );
  for (const bad of ["a\u0000b", "a\u001bb", "a\u0085b", "a\u007fb", "a\rb"])
    assert.deepEqual(normalizeCommand(bad), { ok: false, code: "non_ascii" });
});

test("command and reason refuse bidi overrides, zero-width characters and homoglyphs", () => {
  for (const bad of ["ls ‮", "l​s", "ѕs", "café"]) {
    assert.deepEqual(normalizeCommand(bad), { ok: false, code: "non_ascii" });
    assert.deepEqual(normalizeReason(bad), { ok: false, code: "non_ascii" });
  }
  assert.deepEqual(normalizeReason("check\nthe\ttree"), {
    ok: true,
    text: "check\nthe\ttree",
  });
  assert.equal(normalizeReason("").ok, false);
});
