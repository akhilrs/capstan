import assert from "node:assert/strict";
import { test } from "node:test";
import type { CapstanConfig } from "../src/config/capstan-config.js";
import {
  MAX_REVIEW_ROUNDS,
  MAX_REVIEW_TEXT_BYTES,
  type ReportEvidence,
} from "../src/controller/core.js";
import { startReportRelay } from "../src/reports.js";
import { chooseReviewerRole, reviewText } from "../src/reviews.js";
import { close, ctx, harness, type Harness, type Member } from "./harness.js";

const BASE = "a".repeat(40);
const COMMIT = "b".repeat(40);
const BRANCH = "capstan/developer-1-g1";

function evidence(): ReportEvidence {
  return {
    generation: 1,
    branch: BRANCH,
    baseSha: BASE,
    commitExists: true,
    branchTip: COMMIT,
    isAncestorOfTip: true,
    isAncestorOfBase: false,
    checkedAt: "2026-10-01T00:00:00.000Z",
  };
}

function withRoles(h: Harness): void {
  h.core.syncRoleDefinitions(
    ctx(h.core, h.owner),
    [
      ["pm", "PM"],
      ["developer", "Developer"],
      ["developer2", "Developer"],
      ["pm2", "PM"],
      ["reviewer", "Verifier"],
      ["tester", "Verifier"],
    ].map(([name, kind], index) => ({
      name: name!,
      kind: kind as "PM" | "Developer" | "Verifier",
      host: "claude",
      configHash: String.fromCharCode(97 + index).repeat(64),
    })),
  );
}

function verifier(h: Harness, name: string, roleName = "reviewer"): Member {
  const seatId = `${name}-seat`;
  h.core.createSeat(ctx(h.core, h.owner), { seatId, name, role: "Verifier" });
  const actor = h.core.createActor(ctx(h.core, h.owner), {
    displayName: name,
    role: "Verifier",
    seatId,
  });
  h.core.registerAgent(ctx(h.core, h.owner), {
    agentId: name,
    roleName,
    seatId,
    actorId: actor.actorId,
  });
  return {
    agentId: name,
    credential: actor.credential,
    actorId: actor.actorId,
  };
}

/** An accepted report by the harness developer; the pane row gives it a branch and a base. */
function acceptedReport(h: Harness, summary = "added the thing"): string {
  h.core.recordAgentPane(ctx(h.core, h.owner), {
    agentId: h.developer.agentId,
    workspaceId: null,
    paneId: null,
    worktreePath: null,
    branch: BRANCH,
    baseSha: BASE,
  });
  const { record } = h.core.recordAgentReport(
    ctx(h.core, h.developer.credential),
    {
      commitSha: COMMIT,
      summary,
      evidence: evidence(),
    },
  );
  assert.equal(record.state, "accepted");
  return record.reportId;
}

function begin(h: Harness, reportId: string, reviewerAgentId: string) {
  return h.core.beginReview(ctx(h.core, h.pm.credential), {
    reportId,
    reviewerRole: "reviewer",
    reviewerAgentId,
  });
}

test("a review starts with a fresh reviewer, records the author and the reviewer as different sessions and queues the task from the controller", async () => {
  const h = await harness();
  try {
    withRoles(h);
    const reportId = acceptedReport(h);
    const reviewer = verifier(h, "reviewer-1");
    const review = begin(h, reportId, reviewer.agentId);
    assert.equal(review.state, "started");
    assert.equal(review.round, 1);
    assert.equal(review.commitSha, COMMIT);
    assert.equal(review.baseSha, BASE);
    assert.equal(review.authorAgentId, h.developer.agentId);
    assert.equal(review.authorActorId, h.developer.actorId);
    assert.equal(review.reviewerAgentId, reviewer.agentId);
    assert.equal(review.reviewerActorId, reviewer.actorId);
    assert.notEqual(review.reviewerActorId, review.authorActorId);
    const [task] = h.core.messagesFor(reviewer.agentId);
    assert.ok(
      task!.body.startsWith(
        `Review request ${review.reviewId} (round 1) for report ${reportId}\n`,
      ),
    );
    assert.ok(task!.body.includes(`Commit to review: ${COMMIT}`));
    assert.ok(task!.body.includes(`git diff ${BASE} ${COMMIT}`));
    assert.ok(task!.body.includes('not verified: "added the thing"'));
    assert.equal(h.core.senderOf(task!.senderActorId).role, "controller");
  } finally {
    await close(h);
  }
});

test("a review is refused for an unknown or rejected report, a second open review, a wrong reviewer and the author", async () => {
  const h = await harness();
  try {
    withRoles(h);
    const reviewer = verifier(h, "reviewer-1");
    assert.throws(
      () => begin(h, "no-such-report", reviewer.agentId),
      /report does not exist/,
    );
    const reportId = acceptedReport(h);
    const rejected = h.core.recordAgentReport(
      ctx(h.core, h.developer.credential),
      {
        commitSha: "c".repeat(40),
        summary: "x",
        evidence: { ...evidence(), commitExists: false },
      },
    ).record;
    assert.throws(
      () => begin(h, rejected.reportId, reviewer.agentId),
      /only an accepted report/,
    );
    assert.throws(
      () =>
        h.core.beginReview(ctx(h.core, h.pm.credential), {
          reportId,
          reviewerRole: "developer",
          reviewerAgentId: reviewer.agentId,
        }),
      /active Verifier role/,
    );
    assert.throws(
      () =>
        h.core.beginReview(ctx(h.core, h.pm.credential), {
          reportId,
          reviewerRole: "tester",
          reviewerAgentId: reviewer.agentId,
        }),
      /not an active agent of the requested Verifier role/,
    );
    assert.throws(
      () => begin(h, reportId, h.developer.agentId),
      /not an active agent of the requested/,
    );
    begin(h, reportId, reviewer.agentId);
    const second = verifier(h, "reviewer-2");
    assert.throws(() => begin(h, reportId, second.agentId), /already open/);
    for (const credential of [h.developer.credential, h.owner])
      assert.throws(
        () =>
          h.core.beginReview(ctx(h.core, credential), {
            reportId,
            reviewerRole: "reviewer",
            reviewerAgentId: second.agentId,
          }),
        /does not have|not allowed|capability|Authorization/i,
      );
  } finally {
    await close(h);
  }
});

test("a Verifier author can never review their own report, in code and in the ledger", async () => {
  const h = await harness();
  try {
    withRoles(h);
    const author = verifier(h, "reviewer-1");
    const { default: Database } = await import("better-sqlite3");
    h.core.recordAgentPane(ctx(h.core, h.owner), {
      agentId: author.agentId,
      workspaceId: null,
      paneId: null,
      worktreePath: null,
      branch: "capstan/reviewer-1-g1",
      baseSha: BASE,
    });
    const { record } = h.core.recordAgentReport(
      ctx(h.core, author.credential),
      {
        commitSha: COMMIT,
        summary: "tests added",
        evidence: { ...evidence(), branch: "capstan/reviewer-1-g1" },
      },
    );
    assert.equal(record.state, "accepted");
    assert.throws(
      () => begin(h, record.reportId, author.agentId),
      /cannot be the author/,
    );
    const other = verifier(h, "reviewer-2");
    const ok = begin(h, record.reportId, other.agentId);
    assert.equal(ok.authorAgentId, author.agentId);
    h.core.close();
    const db = new Database(`${h.stateDirectory}/controller.sqlite`);
    try {
      assert.throws(
        () => db.exec(`UPDATE reviews SET reviewer_actor_id = author_actor_id`),
        /immutable/,
      );
      assert.throws(
        () =>
          db
            .prepare(
              `INSERT INTO reviews SELECT project_id, 'forged', 99, 9, subject_report_id, commit_sha, base_sha, author_agent_id, author_actor_id, requested_by_actor_id, reviewer_role, author_agent_id, author_actor_id, 'started', NULL, NULL, NULL, created_at, NULL FROM reviews`,
            )
            .run(),
        /CHECK constraint/,
      );
    } finally {
      db.close();
    }
  } finally {
    await close(h).catch(() => undefined);
  }
});

test("a verdict is written once by the reviewer, bounded, and announced to the PM by the controller", async () => {
  const h = await harness();
  try {
    withRoles(h);
    const reportId = acceptedReport(h);
    const reviewer = verifier(h, "reviewer-1");
    const stranger = verifier(h, "tester-1", "tester");
    const started = begin(h, reportId, reviewer.agentId);
    assert.throws(
      () =>
        h.core.completeReview(ctx(h.core, stranger.credential), {
          verdict: "pass",
          text: "ok",
        }),
      /no review in progress/,
    );
    for (const bad of [
      { verdict: "approve", text: "x" },
      { verdict: "pass", text: "" },
      { verdict: "pass", text: "bell\u0007" },
      { verdict: "pass", text: "two\r\nlines" },
      { verdict: "pass", text: "x".repeat(MAX_REVIEW_TEXT_BYTES + 1) },
      { verdict: "pass", text: "bad \ud800" },
    ])
      assert.throws(
        () =>
          h.core.completeReview(ctx(h.core, reviewer.credential), bad as never),
        TypeError,
        JSON.stringify(bad).slice(0, 50),
      );
    const done = h.core.completeReview(ctx(h.core, reviewer.credential), {
      verdict: "findings",
      text: 'line one\nthe "second" line',
    });
    assert.equal(done.state, "findings");
    assert.equal(done.verdictText, 'line one\nthe "second" line');
    assert.notEqual(done.notifiedMessageId, null);
    const notice = h.core
      .messagesFor(h.pm.agentId)
      .find((m) => m.body.startsWith("Review "))!;
    assert.ok(
      notice.body.startsWith(
        `Review ${started.reviewId} of report ${reportId}, round 1: FINDINGS\n`,
      ),
    );
    assert.ok(notice.body.includes("Different sessions."));
    assert.ok(
      notice.body.includes('"line one\\nthe \\"second\\" line"'),
      "the text is JSON-quoted",
    );
    assert.equal(h.core.senderOf(notice.senderActorId).role, "controller");
    assert.throws(
      () =>
        h.core.completeReview(ctx(h.core, reviewer.credential), {
          verdict: "pass",
          text: "again",
        }),
      /no review in progress/,
    );
  } finally {
    await close(h);
  }
});

test("only rounds that ended in a verdict use the budget, a finished review is final, and an ended reviewer cancels its review", async () => {
  const h = await harness();
  try {
    withRoles(h);
    const reportId = acceptedReport(h);
    const lost = verifier(h, "reviewer-0");
    const first = begin(h, reportId, lost.agentId);
    h.core.endAgent(ctx(h.core, h.owner), lost.agentId);
    const cancelled = h.core
      .reviews(h.owner, 10)
      .find((r) => r.reviewId === first.reviewId)!;
    assert.equal(cancelled.state, "cancelled");
    assert.equal(cancelled.failureReason, "agent_ended");
    for (let i = 1; i <= MAX_REVIEW_ROUNDS; i += 1) {
      const reviewer = verifier(h, `reviewer-r${i}`);
      const review = begin(h, reportId, reviewer.agentId);
      assert.equal(
        review.round,
        i + 1,
        "the cancelled round keeps its number but not its budget",
      );
      h.core.completeReview(ctx(h.core, reviewer.credential), {
        verdict: i === MAX_REVIEW_ROUNDS ? "pass" : "findings",
        text: `round ${i}`,
      });
    }
    const extra = verifier(h, "reviewer-x");
    assert.throws(() => begin(h, reportId, extra.agentId), /review limit/);
    const { default: Database } = await import("better-sqlite3");
    h.core.close();
    const db = new Database(`${h.stateDirectory}/controller.sqlite`);
    try {
      for (const sql of [
        "UPDATE reviews SET verdict_text = 'changed' WHERE state = 'findings'",
        "UPDATE reviews SET state = 'passed' WHERE state = 'findings'",
        "UPDATE reviews SET sequence = 99",
        "UPDATE reviews SET notified_message_id = 'again' WHERE notified_message_id IS NOT NULL",
        "DELETE FROM reviews",
      ])
        assert.throws(
          () => db.exec(sql),
          /final|immutable|announced once/,
          sql,
        );
    } finally {
      db.close();
    }
  } finally {
    await close(h).catch(() => undefined);
  }
});

test("a verdict given while no PM is active is announced later, and a finished review with a live reviewer is listed for release", async () => {
  const h = await harness();
  try {
    withRoles(h);
    const reportId = acceptedReport(h);
    const reviewer = verifier(h, "reviewer-1");
    begin(h, reportId, reviewer.agentId);
    h.core.endAgent(ctx(h.core, h.owner), h.pm.agentId);
    const done = h.core.completeReview(ctx(h.core, reviewer.credential), {
      verdict: "pass",
      text: "fine",
    });
    assert.equal(done.notifiedMessageId, null);
    assert.deepEqual(
      h.core.reviewsToRelease(h.owner).map((r) => r.reviewId),
      [done.reviewId],
    );
    assert.deepEqual(
      h.core.unannouncedReviews(h.owner).map((r) => r.reviewId),
      [done.reviewId],
    );
    const events: string[] = [];
    const pm = h.addMember("pm2", "PM");
    const relay = startReportRelay({
      core: h.core,
      credential: h.owner,
      intervalMs: 10,
      log: (e) => events.push(e),
    });
    try {
      for (
        let waited = 0;
        waited < 2000 && !events.includes("review_announced");
        waited += 20
      )
        await new Promise((resolve) => setTimeout(resolve, 20));
    } finally {
      relay.stop();
    }
    assert.ok(events.includes("review_announced"));
    assert.equal(
      h.core.messagesFor(pm.agentId).filter((m) => m.body.startsWith("Review "))
        .length,
      1,
    );
    h.core.endAgent(ctx(h.core, h.owner), reviewer.agentId);
    assert.deepEqual(h.core.reviewsToRelease(h.owner), []);
  } finally {
    await close(h);
  }
});

test("reviewText folds line breaks, replaces control characters and cuts long text with a marker within the limit", () => {
  assert.equal(reviewText("a\r\nb\rc\u0007d​e"), "a\nb\nc d e");
  assert.equal(reviewText("  x  "), "x");
  const long = reviewText("é".repeat(5000));
  assert.ok(Buffer.byteLength(long, "utf8") <= MAX_REVIEW_TEXT_BYTES);
  assert.ok(long.endsWith(" [text cut]"));
  assert.equal(reviewText("short"), "short");
});

test("the reviewer role is the one asked for, else reviewer, else the only Verifier role", () => {
  const config = (roles: Array<[string, string]>) =>
    ({
      roles: roles.map(([name, kind]) => ({ name, kind })),
    }) as unknown as CapstanConfig;
  assert.equal(
    chooseReviewerRole(
      config([
        ["pm", "PM"],
        ["reviewer", "Verifier"],
        ["tester", "Verifier"],
      ]),
      undefined,
    ),
    "reviewer",
  );
  assert.equal(
    chooseReviewerRole(
      config([
        ["pm", "PM"],
        ["checker", "Verifier"],
      ]),
      undefined,
    ),
    "checker",
  );
  assert.equal(
    chooseReviewerRole(
      config([
        ["reviewer", "Verifier"],
        ["tester", "Verifier"],
      ]),
      "tester",
    ),
    "tester",
  );
  assert.throws(
    () =>
      chooseReviewerRole(
        config([
          ["pm", "PM"],
          ["a", "Verifier"],
          ["b", "Verifier"],
        ]),
        undefined,
      ),
    /several Verifier roles/,
  );
  assert.throws(
    () => chooseReviewerRole(config([["pm", "PM"]]), undefined),
    /no Verifier role/,
  );
  assert.throws(
    () =>
      chooseReviewerRole(
        config([
          ["developer", "Developer"],
          ["reviewer", "Verifier"],
        ]),
        "developer",
      ),
    /not a Verifier role/,
  );
});
