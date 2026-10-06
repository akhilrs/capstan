import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { test } from "node:test";
import {
  adHocBranchName,
  checkCommitMessage,
  formatSubject,
  integrationBranchName,
  isTaskId,
  parseBranch,
  parseCommitSubject,
  reviewBranchName,
  slugify,
  withSuffix,
  workerBranchName,
} from "../src/conventions.js";

const LEDGER = /^[A-Za-z0-9][A-Za-z0-9._/-]{0,99}$/;
const HOSTILE = [
  "Pause / resume: the PM!",
  "Ünïcödé ñ 日本語 🚀",
  "a/b/../c",
  "..",
  "x@{y}",
  "foo.lock",
  ".hidden",
  "a".repeat(300),
  "",
  "///",
  "-- -- --",
  "end.",
  "ß ø æ",
  "feat/x y",
  "~^:?*[\\",
];

function gitOk(name: string): boolean {
  try {
    execFileSync("git", ["check-ref-format", "--branch", name], {
      stdio: "pipe",
    });
    return true;
  } catch {
    return false;
  }
}

test("every builder yields valid refs for hostile input", () => {
  for (const t of HOSTILE) {
    const names = [
      workerBranchName({ type: "feat", taskId: t, slug: t }),
      workerBranchName({ type: t, taskId: "PM-91", slug: t }),
      adHocBranchName(t, t),
      reviewBranchName(t, t),
      integrationBranchName({
        integrationId: t,
        planId: t,
        planTitle: t,
        attempt: 3,
      }),
      withSuffix(workerBranchName({ type: "feat", taskId: t, slug: t }), 12),
      withSuffix("a".repeat(100), 2),
    ];
    for (const n of names) {
      assert.match(n, LEDGER, n);
      assert.ok(gitOk(n), `git rejects ${n}`);
      assert.ok(!n.endsWith(".lock") && !n.includes("..") && !n.includes("@{"));
    }
  }
});

test("slug and builder examples", () => {
  assert.equal(slugify("Pause / resume: the PM!"), "pause-resume-the-pm");
  assert.equal(slugify(""), "work");
  assert.equal(slugify("Ünï"), "uni");
  assert.ok(slugify("a".repeat(300)).length <= 40);
  assert.equal(
    workerBranchName({ type: "feat", taskId: "PM-91", slug: "pause-resume" }),
    "feat/PM-91-pause-resume",
  );
  assert.equal(
    adHocBranchName("developer-3", "developer"),
    "chore/developer-3-developer",
  );
  assert.equal(withSuffix("feat/x", 1), "feat/x");
  assert.equal(withSuffix("feat/x", 2), "feat/x-2");
  assert.equal(withSuffix("a".repeat(100), 3).length, 100);
});

test("parseBranch reads legacy and new names", () => {
  assert.deepEqual(parseBranch("capstan/developer-1-g3"), {
    kind: "legacy-worker",
    agentId: "developer-1",
    generation: 3,
  });
  assert.deepEqual(parseBranch("capstan/integration/int-x"), {
    kind: "legacy-integration",
    integrationId: "int-x",
  });
  assert.deepEqual(parseBranch("main"), { kind: "other" });
  const w = workerBranchName({ type: "fix", taskId: "PM-9", slug: "a b" });
  assert.deepEqual(parseBranch(w), {
    kind: "conventional",
    type: "fix",
    rest: w.slice(4),
  });
  const a = adHocBranchName("developer-3", "x");
  assert.deepEqual(parseBranch(a), {
    kind: "conventional",
    type: "chore",
    rest: a.slice(6),
  });
  const r = reviewBranchName("reviewer-2", "developer-3");
  assert.deepEqual(parseBranch(r), {
    kind: "conventional",
    type: "chore",
    rest: r.slice(6),
  });
  const i = integrationBranchName({
    integrationId: "int-1",
    planId: "plan-2",
    planTitle: "My plan",
    attempt: 2,
  });
  assert.deepEqual(parseBranch(i), { kind: "integration", rest: i.slice(12) });
});

test("parseCommitSubject", () => {
  for (const s of ["feat: x", "fix(core)!: x", "revert: x"])
    assert.equal(parseCommitSubject(s).ok, true, s);
  for (const s of [
    "Feat: x",
    "feat:x",
    "feat(): x",
    "feat: ",
    "update stuff",
    "wip(feat): x",
  ]) {
    assert.equal(parseCommitSubject(s).ok, false, s);
  }
  assert.deepEqual(parseCommitSubject("fix(core)!: do it"), {
    ok: true,
    type: "fix",
    scope: "core",
    breaking: true,
    description: "do it",
  });
});

test("checkCommitMessage", () => {
  const rules = (m: string, p = 1) =>
    checkCommitMessage(m, p).map((f) => f.rule);
  assert.deepEqual(rules("feat: x"), []);
  assert.deepEqual(
    rules("feat: x\n\nbody\n\nCo-authored-by: Ann <ann@example.com>"),
    [],
  );
  assert.deepEqual(
    rules("feat: x\n\nCO-AUTHORED-BY: claude <noreply@anthropic.com>"),
    ["claude-co-author"],
  );
  assert.deepEqual(rules("feat: x\n\nClaude-Session: abc"), ["claude-session"]);
  assert.deepEqual(
    rules(
      "feat: x\n\nGenerated with [Claude Code](https://claude.com/claude-code)",
    ),
    ["claude-code-footer"],
  );
  assert.deepEqual(rules("feat: x\n\n\u{1F916} made by a robot"), [
    "claude-code-footer",
  ]);
  assert.deepEqual(rules("feat: x\nbody"), ["body-separation"]);
  assert.deepEqual(rules("Merge branch 'a'", 2), []);
  assert.deepEqual(rules("Merge x\n\nClaude-Session: a", 2), [
    "claude-session",
  ]);
  assert.deepEqual(rules("nope"), ["subject-format"]);
});

test("formatSubject always parses and fits", () => {
  const long = "word ".repeat(60);
  for (const d of [
    long,
    "x".repeat(200),
    "",
    "  spaced   out ",
    "日本語".repeat(40),
  ]) {
    for (const max of [72, 30, 12]) {
      for (const input of [
        { type: "feat", description: d },
        { type: "bogus", scope: "My Scope!", breaking: true, description: d },
      ]) {
        const s = formatSubject(input, max);
        assert.equal(parseCommitSubject(s).ok, true, s);
        assert.ok(s.length <= max, s);
      }
    }
  }
  assert.equal(
    formatSubject({ type: "feat", description: "add x" }),
    "feat: add x",
  );
  assert.ok(!formatSubject({ type: "feat", description: long }).endsWith(" "));
});

test("formatSubject cuts at a word boundary and drops dangling words and punctuation", () => {
  assert.equal(
    formatSubject({
      type: "test",
      scope: "dash",
      description:
        "end-to-end parity, redraw, key and performance checks of the dash binary",
    }),
    "test(dash): end-to-end parity, redraw, key and performance checks",
  );
  assert.equal(
    formatSubject({
      type: "docs",
      description:
        "document the cstan-dash install and dashboard selection in the README and site",
    }),
    "docs: document the cstan-dash install and dashboard selection",
  );
  const long = formatSubject({ type: "feat", description: "x".repeat(200) });
  assert.equal(long.length, 72);
  assert.equal(long, `feat: ${"x".repeat(66)}`);
  const exact = `feat: ${"a".repeat(30)} ${"b".repeat(35)} of`.slice(0, 72);
  assert.equal(exact.length, 72);
  assert.equal(
    formatSubject({ type: "feat", description: exact.slice(6) }),
    exact,
  );
  const jp = formatSubject({
    type: "feat",
    description: `${"日本語 ".repeat(30)}の`,
  });
  assert.ok(jp.length <= 72 && !jp.endsWith(" "), jp);
  assert.ok(jp.endsWith("日本語"), jp);
  const emoji = formatSubject({
    type: "feat",
    description: "\u{1F916}".repeat(80),
  });
  assert.ok(emoji.length <= 72 && !/[\ud800-\udbff]$/.test(emoji), emoji);
});

test("isTaskId", () => {
  for (const t of ["PM-91", "plan-16", "plan-16/conventions"])
    assert.equal(isTaskId(t), true, t);
  for (const t of ["", "pm91", "PM-", "feat/x", "PM-91 x"])
    assert.equal(isTaskId(t), false, t);
});
