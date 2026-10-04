import assert from "node:assert/strict";
import { openSqlite } from "../src/controller/sqlite.js";
import { test } from "node:test";
import {
  ControllerError,
  MAX_REJECTED_REPORTS,
  MAX_REPORT_SUMMARY_BYTES,
  type ReportEvidence,
} from "../src/controller/core.js";
import { GitCheckError } from "../src/git.js";
import { AuthorizationError } from "../src/controller/auth.js";
import {
  REPORT_RATE_LIMIT,
  REPORT_RATE_WINDOW_MS,
  ReportRateLimiter,
  oneLineSummary,
} from "../src/reports.js";
import { startReportRelay } from "../src/reports.js";
import { call, close, ctx, harness, type Harness } from "./harness.js";

const BASE = "a".repeat(40);
const COMMIT = "b".repeat(40);
const BRANCH = "capstan/developer-1-g1";

function evidence(overrides: Partial<ReportEvidence> = {}): ReportEvidence {
  return {
    generation: 1,
    branch: BRANCH,
    baseSha: BASE,
    commitExists: true,
    branchTip: COMMIT,
    isAncestorOfTip: true,
    isAncestorOfBase: false,
    checkedAt: "2026-10-01T00:00:00.000Z",
    ...overrides,
  };
}

function recordPane(
  h: Harness,
  branch: string | null,
  baseSha: string | null,
): void {
  h.core.recordAgentPane(ctx(h.core, h.owner), {
    agentId: h.developer.agentId,
    workspaceId: null,
    paneId: null,
    worktreePath: null,
    branch,
    baseSha,
  });
}

function report(
  h: Harness,
  commitSha = COMMIT,
  extra: Partial<ReportEvidence> = {},
  summary = "added the thing",
) {
  return h.core.recordAgentReport(ctx(h.core, h.developer.credential), {
    commitSha,
    summary,
    evidence: evidence(extra),
  });
}

test("an accepted report is recorded with the claimed identity and the checked evidence, and the PM is told by the controller", async () => {
  const h = await harness();
  try {
    recordPane(h, BRANCH, BASE);
    const { record, duplicate } = report(h);
    assert.equal(duplicate, false);
    assert.equal(record.state, "accepted");
    assert.equal(record.reason, null);
    assert.equal(record.actorId, h.developer.actorId, "the claimed identity");
    assert.equal(record.agentId, h.developer.agentId);
    assert.equal(record.generation, 1);
    assert.equal(record.branch, BRANCH);
    assert.deepEqual(record.evidence, evidence());
    assert.notEqual(record.notifiedMessageId, null);
    const [notice] = h.core.messagesFor(h.pm.agentId);
    assert.equal(notice!.messageId, record.notifiedMessageId);
    assert.ok(notice!.body.startsWith(`Verified report ${record.reportId}\n`));
    assert.ok(notice!.body.includes(`Commit: ${COMMIT}`));
    assert.ok(notice!.body.includes(`Branch: ${BRANCH}`));
    assert.ok(
      notice!.body.includes(
        'Summary written by the worker, not verified: "added the thing"',
      ),
    );
    assert.equal(h.core.senderOf(notice!.senderActorId).role, "controller");
    assert.equal(h.core.senderOf(notice!.senderActorId).agentId, null);
  } finally {
    await close(h);
  }
});

test("each way a claim can fail is recorded as a rejection with its reason and no notice", async () => {
  const h = await harness();
  try {
    const cases: Array<[string, () => void, Partial<ReportEvidence>]> = [
      ["no_branch", () => recordPane(h, null, BASE), {}],
      ["no_base", () => recordPane(h, BRANCH, null), {}],
      [
        "agent_changed",
        () => recordPane(h, BRANCH, BASE),
        { branch: "capstan/other-g1" },
      ],
      ["agent_changed", () => recordPane(h, BRANCH, BASE), { generation: 2 }],
      [
        "agent_changed",
        () => recordPane(h, BRANCH, BASE),
        { baseSha: "c".repeat(40) },
      ],
      [
        "commit_missing",
        () => recordPane(h, BRANCH, BASE),
        { commitExists: false },
      ],
      [
        "not_new_on_branch",
        () => recordPane(h, BRANCH, BASE),
        { isAncestorOfBase: true },
      ],
      [
        "not_on_branch",
        () => recordPane(h, BRANCH, BASE),
        { isAncestorOfTip: false },
      ],
    ];
    for (const [reason, setup, extra] of cases) {
      setup();
      const { record } = report(h, COMMIT, extra);
      assert.equal(
        record.state,
        "rejected",
        `${reason} ${JSON.stringify(extra)}`,
      );
      assert.equal(record.reason, reason, JSON.stringify(extra));
      assert.equal(record.notifiedMessageId, null);
    }
    recordPane(h, BRANCH, BASE);
    assert.equal(
      report(h, BASE).record.reason,
      "not_new_on_branch",
      "the base commit itself is never new work",
    );
    assert.deepEqual(h.core.messagesFor(h.pm.agentId), []);
    assert.equal(h.core.agentReports(h.owner, 50).length, cases.length + 1);
  } finally {
    await close(h);
  }
});

test("a repeated accepted report writes nothing new, a rejected one does not block a later valid claim, and rejections are capped", async () => {
  const h = await harness();
  try {
    recordPane(h, BRANCH, BASE);
    const first = report(h, COMMIT, { isAncestorOfTip: false });
    assert.equal(first.record.state, "rejected");
    const second = report(h);
    assert.equal(second.record.state, "accepted");
    const third = report(h);
    assert.equal(third.duplicate, true);
    assert.equal(third.record.reportId, second.record.reportId);
    assert.equal(h.core.agentReports(h.owner, 50).length, 2);
    assert.equal(h.core.messagesFor(h.pm.agentId).length, 1);
    for (let i = 2; i <= MAX_REJECTED_REPORTS; i += 1)
      report(h, i.toString(16).padStart(40, "0"), { commitExists: false });
    assert.throws(
      () => report(h, "d".repeat(40), { commitExists: false }),
      (error: unknown) =>
        error instanceof ControllerError && /report limit/.test(error.message),
    );
    assert.equal(
      h.core.agentReports(h.owner, 500).filter((r) => r.state === "rejected")
        .length,
      MAX_REJECTED_REPORTS,
    );
    assert.equal(
      report(h, "e".repeat(40)).record.state,
      "accepted",
      "a correct report still gets through at the cap",
    );
  } finally {
    await close(h);
  }
});

test("only a worker can report: the PM and the operator lack the capability, and bad input is refused before anything is written", async () => {
  const h = await harness();
  try {
    recordPane(h, BRANCH, BASE);
    for (const credential of [h.pm.credential, h.owner])
      assert.throws(
        () =>
          h.core.recordAgentReport(ctx(h.core, credential), {
            commitSha: COMMIT,
            summary: "x",
            evidence: evidence(),
          }),
        AuthorizationError,
      );
    for (const bad of [
      { commitSha: "B".repeat(40), summary: "x" },
      { commitSha: "b".repeat(39), summary: "x" },
      { commitSha: COMMIT, summary: "" },
      { commitSha: COMMIT, summary: "two\nlines" },
      { commitSha: COMMIT, summary: "bell\u0007" },
      { commitSha: COMMIT, summary: "x".repeat(MAX_REPORT_SUMMARY_BYTES + 1) },
    ])
      assert.throws(
        () =>
          h.core.recordAgentReport(ctx(h.core, h.developer.credential), {
            ...bad,
            evidence: evidence(),
          }),
        TypeError,
        JSON.stringify(bad).slice(0, 60),
      );
    assert.throws(
      () =>
        h.core.recordAgentReport(ctx(h.core, h.developer.credential), {
          commitSha: COMMIT,
          summary: "x",
          evidence: { ...evidence(), branchTip: "nothex" },
        }),
      TypeError,
    );
    assert.equal(h.core.agentReports(h.owner, 50).length, 0);
  } finally {
    await close(h);
  }
});

test("with no PM the report is kept unannounced; the relay path queues exactly one notice later", async () => {
  const h = await harness();
  try {
    recordPane(h, BRANCH, BASE);
    h.core.endAgent(ctx(h.core, h.owner), h.pm.agentId);
    const { record } = report(h);
    assert.equal(record.state, "accepted");
    assert.equal(record.notifiedMessageId, null);
    assert.deepEqual(
      h.core.unannouncedReports(h.owner).map((r) => r.reportId),
      [record.reportId],
    );
    assert.equal(
      h.core.announceReport(ctx(h.core, h.owner), record.reportId).announced,
      false,
      "still no PM",
    );
    const pm = h.addMember("pm2", "PM");
    assert.equal(
      h.core.announceReport(ctx(h.core, h.owner), record.reportId).announced,
      true,
    );
    assert.equal(
      h.core.announceReport(ctx(h.core, h.owner), record.reportId).announced,
      false,
      "announced once",
    );
    assert.equal(h.core.messagesFor(pm.agentId).length, 1);
    assert.deepEqual(h.core.unannouncedReports(h.owner), []);
  } finally {
    await close(h);
  }
});

test("reports are immutable, announced once, listed newest first and found by inspect", async () => {
  const h = await harness();
  try {
    recordPane(h, BRANCH, BASE);
    const a = report(h, COMMIT).record;
    const b = report(h, "c".repeat(40), { isAncestorOfTip: false }).record;
    assert.deepEqual(
      h.core.agentReports(h.owner, 10).map((r) => r.reportId),
      [b.reportId, a.reportId],
    );
    const inspected = h.core.inspect(a.reportId) as { kind: string };
    assert.equal(inspected.kind, "report");
    h.core.close();
    const db = openSqlite(`${h.stateDirectory}/controller.sqlite`);
    try {
      for (const sql of [
        "UPDATE agent_reports SET summary = 'changed'",
        "UPDATE agent_reports SET state = 'accepted' WHERE state = 'rejected'",
        "UPDATE agent_reports SET notified_message_id = 'again' WHERE notified_message_id IS NOT NULL",
        "DELETE FROM agent_reports",
      ])
        assert.throws(() => db.exec(sql), /immutable|announced once/, sql);
    } finally {
      db.close();
    }
  } finally {
    await close(h).catch(() => undefined);
  }
});

test("a summary is cut to one line within its byte limit at a character boundary", () => {
  assert.equal(oneLineSummary("  a\r\nb\tc  \u0007d ", 100), "a b c d");
  assert.equal(oneLineSummary("\u0000\u001b", 100), "");
  assert.equal(
    oneLineSummary("\ufeff\u00a0 ", 100),
    "",
    "a byte-order mark and blanks alone leave nothing",
  );
  const accented = "e\u0301";
  const cut = oneLineSummary(accented.repeat(20), 10);
  assert.ok(Buffer.byteLength(cut, "utf8") <= 10);
  assert.equal(cut.length % accented.length, 0, "no letter loses its accent");
  assert.equal(cut.length, 6, "three whole letters fit in ten bytes");
  assert.equal(
    oneLineSummary("a\u200db", 100),
    "a b",
    "format characters, the joiner included, are replaced like the core does",
  );
  assert.equal(oneLineSummary("é".repeat(600), 1000).length, 500);
});

test("the rate limiter allows ten in a minute per key and forgets old attempts", () => {
  const limiter = new ReportRateLimiter();
  for (let i = 0; i < REPORT_RATE_LIMIT; i += 1)
    assert.ok(limiter.allow("a", i));
  assert.equal(limiter.allow("a", 10), false);
  assert.ok(limiter.allow("b", 10), "another agent has its own allowance");
  assert.ok(limiter.allow("a", REPORT_RATE_WINDOW_MS + 5));
});

test("the report command checks the caller, the arguments and the git facts, and answers with the recorded verdict", async () => {
  const seen: Array<{ branch: string; baseSha: string | null; sha: string }> =
    [];
  const h = await harness({
    commands: {
      inspectCommit: async (input) => {
        seen.push(input);
        return {
          commitExists: input.sha === COMMIT,
          branchTip: COMMIT,
          isAncestorOfTip: input.sha === COMMIT,
          isAncestorOfBase: false,
        };
      },
    },
  });
  try {
    recordPane(h, BRANCH, BASE);
    const ok = await call(h, h.developer.credential, "report", [
      COMMIT.toUpperCase(),
      "  did it \n",
    ]);
    assert.ok(ok.ok, JSON.stringify(ok));
    assert.equal(
      (ok as { result: { state: string; announced: boolean } }).result.state,
      "accepted",
    );
    assert.equal(
      (ok as { result: { announced: boolean } }).result.announced,
      true,
    );
    assert.deepEqual(seen[0], { branch: BRANCH, baseSha: BASE, sha: COMMIT });
    const duplicate = await call(h, h.developer.credential, "report", [
      COMMIT,
      "again",
    ]);
    assert.ok(duplicate.ok);
    assert.equal(
      (duplicate as { result: { duplicate: boolean } }).result.duplicate,
      true,
    );
    assert.equal(seen.length, 1, "a duplicate is not looked up again");
    const wrong = await call(h, h.developer.credential, "report", [
      "e".repeat(40),
      "x",
    ]);
    assert.ok(!wrong.ok);
    assert.equal(wrong.code, "rejected");
    assert.match(wrong.message, /^report_rejected: that commit does not exist/);
    assert.equal(
      (await call(h, h.pm.credential, "report", [COMMIT, "x"])).ok,
      false,
    );
    assert.equal(
      (
        (await call(h, h.pm.credential, "report", [COMMIT, "x"])) as {
          code: string;
        }
      ).code,
      "forbidden",
    );
    assert.equal(
      ((await call(h, h.owner, "report", [COMMIT, "x"])) as { code: string })
        .code,
      "forbidden",
    );
    for (const args of [
      [],
      [COMMIT],
      [COMMIT, "x", "y"],
      ["abc", "x"],
      [COMMIT, "   "],
      [`${COMMIT} `, "x"],
    ])
      assert.equal(
        (
          (await call(h, h.developer.credential, "report", args)) as {
            code: string;
          }
        ).code,
        "invalid_request",
        JSON.stringify(args),
      );
    const status = await call(h, h.owner, "status");
    const reports = (
      status as { result: { reports: Array<{ state: string }> } }
    ).result.reports;
    assert.deepEqual(reports.map((r) => r.state).sort(), [
      "accepted",
      "rejected",
    ]);
  } finally {
    await close(h);
  }
});

test("a worker is limited to ten report commands a minute and a git failure is not recorded as a rejection", async () => {
  let fail = false;
  const { GitCheckError } = await import("../src/git.js");
  const h = await harness({
    commands: {
      now: () => 1_000_000,
      inspectCommit: async () => {
        if (fail) throw new GitCheckError("git is unavailable");
        return {
          commitExists: false,
          branchTip: null,
          isAncestorOfTip: false,
          isAncestorOfBase: false,
        };
      },
    },
  });
  try {
    recordPane(h, BRANCH, BASE);
    fail = true;
    const broken = await call(h, h.developer.credential, "report", [
      COMMIT,
      "x",
    ]);
    assert.equal(broken.ok, false);
    assert.equal((broken as { code: string }).code, "error");
    assert.equal(
      h.core.agentReports(h.owner, 50).length,
      0,
      "nothing recorded when git failed",
    );
    fail = false;
    for (let i = 0; i < REPORT_RATE_LIMIT - 1; i += 1)
      await call(h, h.developer.credential, "report", [
        i.toString(16).padStart(40, "0"),
        "x",
      ]);
    const limited = await call(h, h.developer.credential, "report", [
      "f".repeat(40),
      "x",
    ]);
    assert.equal((limited as { code: string }).code, "rejected");
    assert.match((limited as { message: string }).message, /^rate_limited/);
  } finally {
    await close(h);
  }
});

test("the relay announces a report that was accepted while no PM was active, once", async () => {
  const h = await harness();
  try {
    recordPane(h, BRANCH, BASE);
    h.core.endAgent(ctx(h.core, h.owner), h.pm.agentId);
    const { record } = report(h);
    const events: string[] = [];
    const relay = startReportRelay({
      core: h.core,
      credential: h.owner,
      intervalMs: 20,
      log: (event) => events.push(event),
    });
    try {
      await new Promise((resolve) => setTimeout(resolve, 100));
      assert.deepEqual(events, [], "no PM yet, nothing to announce");
      const pm = h.addMember("pm2", "PM");
      for (let waited = 0; waited < 2000 && events.length === 0; waited += 20)
        await new Promise((resolve) => setTimeout(resolve, 20));
      assert.equal(events.filter((e) => e === "report_announced").length, 1);
      await new Promise((resolve) => setTimeout(resolve, 100));
      assert.equal(h.core.messagesFor(pm.agentId).length, 1, "one notice only");
      assert.notEqual(
        h.core.acceptedReportFor(h.developer.agentId, 1, COMMIT)
          ?.notifiedMessageId,
        null,
      );
      assert.equal(record.state, "accepted");
    } finally {
      relay.stop();
    }
  } finally {
    await close(h);
  }
});

test("a repeated accepted report and an idle relay write nothing to the ledger", async () => {
  const h = await harness({
    commands: {
      inspectCommit: async () => ({
        commitExists: true,
        branchTip: COMMIT,
        isAncestorOfTip: true,
        isAncestorOfBase: false,
      }),
    },
  });
  const writes = (): number => {
    const db = openSqlite(`${h.stateDirectory}/controller.sqlite`, {
      readOnly: true,
    });
    try {
      return (
        (
          db.prepare("SELECT COUNT(*) AS n FROM mutation_requests").get() as {
            n: number;
          }
        ).n +
        (
          db.prepare("SELECT COUNT(*) AS n FROM controller_events").get() as {
            n: number;
          }
        ).n
      );
    } finally {
      db.close();
    }
  };
  try {
    recordPane(h, BRANCH, BASE);
    assert.ok(
      (await call(h, h.developer.credential, "report", [COMMIT, "first"])).ok,
    );
    const before = writes();
    for (let i = 0; i < 5; i += 1)
      assert.ok(
        (await call(h, h.developer.credential, "report", [COMMIT, "again"])).ok,
      );
    assert.equal(writes(), before, "five repeats wrote nothing");
    h.core.endAgent(ctx(h.core, h.owner), h.pm.agentId);
    report(h, "c".repeat(40));
    const afterSecond = writes();
    const relay = startReportRelay({
      core: h.core,
      credential: h.owner,
      intervalMs: 10,
      log: () => undefined,
    });
    try {
      await new Promise((resolve) => setTimeout(resolve, 150));
    } finally {
      relay.stop();
    }
    assert.equal(writes(), afterSecond, "with no PM the relay wrote nothing");
  } finally {
    await close(h);
  }
});

test("the rate limiter forgets keys whose attempts are all old", () => {
  const limiter = new ReportRateLimiter();
  for (let i = 0; i < 300; i += 1) limiter.allow(`agent-${i}`, 0);
  assert.ok(limiter.allow("agent-new", REPORT_RATE_WINDOW_MS + 1));
  assert.ok(
    limiter.allow("agent-0", REPORT_RATE_WINDOW_MS + 2),
    "an old key starts fresh",
  );
});

test("lone surrogates are dropped from a summary, a huge first character still leaves text, and repeats count against the limit", async () => {
  assert.equal(oneLineSummary("a\ud800b", 100), "a b");
  const flood = "e" + "\u0301".repeat(2000);
  const kept = oneLineSummary(flood, 100);
  assert.ok(kept.length > 0 && Buffer.byteLength(kept, "utf8") <= 100);
  const h = await harness();
  try {
    recordPane(h, BRANCH, BASE);
    assert.throws(
      () =>
        h.core.recordAgentReport(ctx(h.core, h.developer.credential), {
          commitSha: COMMIT,
          summary: "bad \ud800 text",
          evidence: evidence(),
        }),
      TypeError,
    );
  } finally {
    await close(h);
  }
  const h2 = await harness({
    commands: {
      now: () => 5_000_000,
      inspectCommit: async () => ({
        commitExists: true,
        branchTip: COMMIT,
        isAncestorOfTip: true,
        isAncestorOfBase: false,
      }),
    },
  });
  try {
    recordPane(h2, BRANCH, BASE);
    assert.ok(
      (await call(h2, h2.developer.credential, "report", [COMMIT, "x"])).ok,
    );
    for (let i = 1; i < REPORT_RATE_LIMIT; i += 1)
      assert.ok(
        (await call(h2, h2.developer.credential, "report", [COMMIT, "x"])).ok,
      );
    const limited = await call(h2, h2.developer.credential, "report", [
      COMMIT,
      "x",
    ]);
    assert.equal((limited as { code: string }).code, "rejected");
    assert.match((limited as { message: string }).message, /^rate_limited/);
  } finally {
    await close(h2);
  }
});

test("with two active PMs the core cannot choose one, so the relay stays idle instead of writing a refusal every tick", async () => {
  const h = await harness();
  try {
    recordPane(h, BRANCH, BASE);
    h.core.endAgent(ctx(h.core, h.owner), h.pm.agentId);
    report(h);
    h.core.syncRoleDefinitions(
      ctx(h.core, h.owner),
      [
        ["pm", "PM"],
        ["developer", "Developer"],
        ["developer2", "Developer"],
        ["pm2", "PM"],
        ["pm3", "PM"],
      ].map(([name, kind], index) => ({
        name: name!,
        kind: kind as "PM" | "Developer",
        host: "claude",
        configHash: String.fromCharCode(97 + index).repeat(64),
      })),
    );
    h.addMember("pm2", "PM");
    h.addMember("pm3", "PM");
    const events: string[] = [];
    const relay = startReportRelay({
      core: h.core,
      credential: h.owner,
      intervalMs: 10,
      log: (e) => events.push(e),
    });
    try {
      await new Promise((resolve) => setTimeout(resolve, 100));
    } finally {
      relay.stop();
    }
    assert.deepEqual(events, []);
  } finally {
    await close(h);
  }
});

test("the report command refuses new commits that break the commit rules, writes no row, and accepts them once fixed", async () => {
  let messages: { sha: string; parents: number; message: string }[] = [];
  let limitSeen = 0;
  const h = await harness({
    commands: {
      inspectCommit: async () => ({
        commitExists: true,
        branchTip: COMMIT,
        isAncestorOfTip: true,
        isAncestorOfBase: false,
      }),
      newCommitMessages: async (input) => {
        limitSeen = input.limit;
        return messages;
      },
    },
  });
  const rows = async () =>
    (
      (await call(h, h.owner, "status")) as {
        result: { reports: unknown[] };
      }
    ).result.reports.length;
  const sendReport = () =>
    call(h, h.developer.credential, "report", [COMMIT, "x"]);
  try {
    recordPane(h, BRANCH, BASE);
    messages = [{ sha: "c".repeat(40), parents: 1, message: "update stuff" }];
    const bad = await sendReport();
    assert.ok(!bad.ok);
    assert.equal(bad.code, "rejected");
    assert.match(
      bad.message,
      /commit_message: cccccccc "update stuff" breaks subject-format: /,
    );
    assert.match(bad.message, /git commit --amend/);
    assert.equal(await rows(), 0, "no ledger row for a refusal");
    messages = [
      { sha: "c".repeat(40), parents: 1, message: "fix: update stuff" },
    ];
    messages = [
      {
        sha: "c".repeat(40),
        parents: 1,
        message:
          "fix: a\n\nco-authored-by: Claude Opus <noreply@anthropic.com>",
      },
    ];
    const co = await sendReport();
    assert.ok(!co.ok);
    assert.match(co.message, /breaks claude-co-author/);
    messages = [
      {
        sha: "c".repeat(40),
        parents: 1,
        message: "fix: a\n\nClaude-Session: x",
      },
      {
        sha: "d".repeat(40),
        parents: 1,
        message:
          "fix: b\n\n\u{1F916} Generated with [Claude Code](https://claude.com/claude-code)",
      },
    ];
    const both = await sendReport();
    assert.ok(!both.ok);
    assert.match(both.message, /claude-session/);
    assert.match(both.message, /claude-code-footer/);
    messages = [
      { sha: "c".repeat(40), parents: 2, message: "Merge branch 'x' into y" },
    ];
    assert.ok((await sendReport()).ok, "a default merge subject is accepted");
    assert.equal(limitSeen, 200);
  } finally {
    await close(h);
  }
  const h2 = await harness({
    commands: {
      inspectCommit: async () => ({
        commitExists: true,
        branchTip: COMMIT,
        isAncestorOfTip: true,
        isAncestorOfBase: false,
      }),
      newCommitMessages: async () => ({ tooMany: true }),
    },
  });
  try {
    recordPane(h2, BRANCH, BASE);
    const many = await call(h2, h2.developer.credential, "report", [
      COMMIT,
      "x",
    ]);
    assert.ok(!many.ok);
    assert.match(many.message, /more than 200 new commits/);
  } finally {
    await close(h2);
  }
});

test("a git failure while reading commit messages is the could-not-check error, not acceptance", async () => {
  const h = await harness({
    commands: {
      inspectCommit: async () => ({
        commitExists: true,
        branchTip: COMMIT,
        isAncestorOfTip: true,
        isAncestorOfBase: false,
      }),
      newCommitMessages: async () => {
        throw new GitCheckError("boom");
      },
    },
  });
  try {
    recordPane(h, BRANCH, BASE);
    const out = await call(h, h.developer.credential, "report", [COMMIT, "x"]);
    assert.ok(!out.ok);
    assert.equal(out.code, "error");
    assert.match(out.message, /could not check the commit/);
  } finally {
    await close(h);
  }
});
