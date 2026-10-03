import assert from "node:assert/strict";
import { openSqlite } from "../src/controller/sqlite.js";
import { test } from "node:test";
import type { ReportEvidence } from "../src/controller/core.js";
import {
  IntegrationError,
  integrate,
  squashMessage,
  recoverIntegrations,
  settleIntegration,
  type IntegrationDeps,
  type IntegrationGit,
} from "../src/integration.js";
import type { IntegrationMergeInput, MergeResult } from "../src/git.js";
import { close, ctx, harness, type Harness, type Member } from "./harness.js";

const BASE = "a".repeat(40);
const HEAD = "d".repeat(40);

function withRoles(h: Harness): void {
  h.core.syncRoleDefinitions(
    ctx(h.core, h.owner),
    [
      ["pm", "PM"],
      ["developer", "Developer"],
      ["reviewer", "Verifier"],
    ].map(([name, kind], index) => ({
      name: name!,
      kind: kind as "PM" | "Developer" | "Verifier",
      host: "claude",
      configHash: String.fromCharCode(97 + index).repeat(64),
    })),
  );
}

function member(
  h: Harness,
  name: string,
  role: "Developer" | "Verifier",
  roleName: string,
): Member {
  const seatId = `${name}-seat`;
  h.core.createSeat(ctx(h.core, h.owner), { seatId, name, role });
  const actor = h.core.createActor(ctx(h.core, h.owner), {
    displayName: name,
    role,
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

function reportBy(
  h: Harness,
  author: Member,
  commit: string,
  summary = `work of ${author.agentId}`,
): string {
  const branch = `capstan/${author.agentId}-g1`;
  h.core.recordAgentPane(ctx(h.core, h.owner), {
    agentId: author.agentId,
    workspaceId: null,
    paneId: null,
    worktreePath: null,
    branch,
    baseSha: BASE,
  });
  const evidence: ReportEvidence = {
    generation: 1,
    branch,
    baseSha: BASE,
    commitExists: true,
    branchTip: commit,
    isAncestorOfTip: true,
    isAncestorOfBase: false,
    checkedAt: "2026-10-01T00:00:00.000Z",
  };
  const { record } = h.core.recordAgentReport(ctx(h.core, author.credential), {
    commitSha: commit,
    summary,
    evidence,
  });
  assert.equal(record.state, "accepted");
  return record.reportId;
}

let reviewers = 0;

function review(
  h: Harness,
  subjectId: string,
  verdict: "pass" | "findings",
): void {
  const reviewer = member(h, `reviewer-${++reviewers}`, "Verifier", "reviewer");
  h.core.beginReview(ctx(h.core, h.pm.credential), {
    subjectId,
    reviewerRole: "reviewer",
    reviewerAgentId: reviewer.agentId,
  });
  h.core.completeReview(ctx(h.core, reviewer.credential), {
    verdict,
    text: `${verdict} from ${reviewer.agentId}`,
  });
}

interface FakeGit extends IntegrationGit {
  readonly calls: string[];
  readonly inputs: IntegrationMergeInput[];
  mergeResult: MergeResult;
  inHead: boolean;
}

function fakeGit(): FakeGit {
  const fake: FakeGit = {
    calls: [],
    inputs: [],
    mergeResult: { kind: "merged", headSha: HEAD },
    inHead: false,
    headCommit: async () => BASE,
    commitExists: async () => true,
    merge: async (input) => {
      fake.inputs.push(input);
      fake.calls.push(`merge ${input.merges.map((m) => m.reportId).join(",")}`);
      return fake.mergeResult;
    },
    branchTip: async () => null,
    isInHead: async () => fake.inHead,
    deleteBranch: async (branch, sha) => {
      fake.calls.push(`delete ${branch} ${sha}`);
      return true;
    },
  };
  return fake;
}

function deps(h: Harness, git: IntegrationGit) {
  return {
    core: h.core,
    git,
    context: (credential: string) => ctx(h.core, credential),
    credential: h.owner,
    log: () => undefined,
  };
}

/** Two developers with a reviewed (passed) report each. */
function reviewedPair(h: Harness): { ids: string[]; a: Member; b: Member } {
  withRoles(h);
  const a = h.developer;
  const b = member(h, "developer-2", "Developer", "developer");
  const ids = [reportBy(h, a, "1".repeat(40)), reportBy(h, b, "2".repeat(40))];
  for (const id of ids) review(h, id, "pass");
  return { ids, a, b };
}

test("an integration needs accepted reports whose latest review passed, named once, and runs one at a time", async () => {
  const h = await harness();
  try {
    withRoles(h);
    const unreviewed = reportBy(h, h.developer, "1".repeat(40));
    const git = fakeGit();
    await assert.rejects(
      integrate(deps(h, git), {
        reportIds: [unreviewed],
        requestedBy: "operator",
      }),
      /no passed review/,
    );
    review(h, unreviewed, "findings");
    await assert.rejects(
      integrate(deps(h, git), {
        reportIds: [unreviewed],
        requestedBy: "operator",
      }),
      /no passed review/,
    );
    review(h, unreviewed, "pass");
    await assert.rejects(
      integrate(deps(h, git), {
        reportIds: [unreviewed, unreviewed],
        requestedBy: "operator",
      }),
      /named twice/,
    );
    await assert.rejects(
      integrate(deps(h, git), { reportIds: ["nope"], requestedBy: "operator" }),
      /does not exist/,
    );
    assert.deepEqual(git.calls, [], "no refusal reached git");
    const record = await integrate(deps(h, git), {
      reportIds: [unreviewed],
      requestedBy: "operator",
    });
    assert.equal(record.state, "merged");
    assert.equal(record.headSha, HEAD);
    assert.equal(record.baseSha, BASE);
    assert.equal(record.branch, `capstan/integration/${record.integrationId}`);
  } finally {
    await close(h);
  }
});

test("a second integration is refused while one is merging", async () => {
  const h = await harness();
  try {
    const { ids } = reviewedPair(h);
    const slow = fakeGit();
    let release: () => void = () => undefined;
    const gate = new Promise<void>((resolve) => (release = resolve));
    slow.merge = async () => {
      await gate;
      return { kind: "merged", headSha: HEAD };
    };
    const first = integrate(deps(h, slow), {
      reportIds: [ids[0]!],
      requestedBy: "operator",
    });
    await new Promise((resolve) => setTimeout(resolve, 50));
    await assert.rejects(
      integrate(deps(h, fakeGit()), {
        reportIds: [ids[1]!],
        requestedBy: "operator",
      }),
      /already running/,
    );
    release();
    assert.equal((await first).state, "merged");
  } finally {
    await close(h);
  }
});

test("merges follow the order given and the record lists the reports in that order", async () => {
  const h = await harness();
  try {
    const { ids } = reviewedPair(h);
    const git = fakeGit();
    const record = await integrate(deps(h, git), {
      reportIds: [ids[1]!, ids[0]!],
      requestedBy: h.pm.agentId,
    });
    assert.deepEqual(
      record.reports.map((r) => r.reportId),
      [ids[1], ids[0]],
    );
    assert.equal(git.calls[0], `merge ${ids[1]},${ids[0]}`);
  } finally {
    await close(h);
  }
});

test("a conflict is recorded with the report and files, reported to the PM when the operator asked, and leaves no branch", async () => {
  const h = await harness();
  try {
    const { ids } = reviewedPair(h);
    const git = fakeGit();
    git.mergeResult = {
      kind: "conflicted",
      reportId: ids[1]!,
      files: ["src/a.ts", "evil\\x0aname"],
      omitted: 0,
    };
    const before = h.core.messagesFor(h.pm.agentId).length;
    const record = await integrate(deps(h, git), {
      reportIds: ids,
      requestedBy: "operator",
    });
    assert.equal(record.state, "conflicted");
    assert.equal(record.conflictReportId, ids[1]);
    assert.deepEqual(record.conflictFiles, ["src/a.ts", "evil\\x0aname"]);
    assert.equal(record.conflictFilesOmitted, 0);
    assert.equal(record.headSha, null);
    const messages = h.core.messagesFor(h.pm.agentId);
    assert.equal(messages.length, before + 1);
    const notice = messages.at(-1)!;
    assert.ok(notice.body.includes("blocked by a merge conflict"));
    assert.ok(notice.body.includes("src/a.ts, evil\\x0aname"));
    assert.equal(h.core.senderOf(notice.senderActorId).role, "controller");
    assert.ok(!git.calls.some((c) => c.startsWith("delete")));
    await assert.rejects(
      async () => h.core.checkReviewRequest(record.integrationId, "reviewer"),
      /merged, unconfirmed/,
    );
    const again = await integrate(deps(h, fakeGit()), {
      reportIds: ids,
      requestedBy: h.pm.agentId,
    });
    assert.equal(
      again.state,
      "merged",
      "a conflict does not block a later integration",
    );
    assert.equal(
      h.core.messagesFor(h.pm.agentId).length,
      before + 1,
      "the PM asked itself, so no extra notice",
    );
  } finally {
    await close(h);
  }
});

test("a git failure marks the integration failed with the reason", async () => {
  const h = await harness();
  try {
    const { ids } = reviewedPair(h);
    const git = fakeGit();
    git.merge = async () => {
      throw new Error("boom");
    };
    const record = await integrate(deps(h, git), {
      reportIds: ids,
      requestedBy: "operator",
    });
    assert.equal(record.state, "failed");
    assert.equal(record.failureReason, "git could not run the merge");
  } finally {
    await close(h);
  }
});

test("the integrated commit is reviewed by someone who authored none of the reports, and only a passed review confirms it", async () => {
  const h = await harness();
  try {
    const { ids } = reviewedPair(h);
    const git = fakeGit();
    const record = await integrate(deps(h, git), {
      reportIds: ids,
      requestedBy: h.pm.agentId,
    });
    const check = h.core.checkReviewRequest(record.integrationId, "reviewer");
    assert.equal(check.commitSha, HEAD);
    assert.equal(check.baseSha, BASE);
    assert.deepEqual(check.authorAgentIds, [
      h.developer.agentId,
      "developer-2",
    ]);
    const author = member(h, "author-reviewer", "Verifier", "reviewer");
    const authored = reportBy(h, author, "3".repeat(40));
    review(h, authored, "pass");
    const second = await integrate(deps(h, git), {
      reportIds: [authored],
      requestedBy: h.pm.agentId,
    });
    assert.throws(
      () =>
        h.core.beginReview(ctx(h.core, h.pm.credential), {
          subjectId: second.integrationId,
          reviewerRole: "reviewer",
          reviewerAgentId: author.agentId,
        }),
      /cannot be an author/,
    );
    const reviewer = member(h, "integration-reviewer", "Verifier", "reviewer");
    const started = h.core.beginReview(ctx(h.core, h.pm.credential), {
      subjectId: record.integrationId,
      reviewerRole: "reviewer",
      reviewerAgentId: reviewer.agentId,
    });
    assert.equal(started.integrationId, record.integrationId);
    assert.equal(started.reportId, null);
    assert.equal(started.authorAgentId, null);
    const [task] = h.core.messagesFor(reviewer.agentId);
    assert.ok(task!.body.includes(`for integration ${record.integrationId}`));
    assert.ok(
      task!.body.includes(`Report ${ids[0]} by ${h.developer.agentId}`),
    );
    assert.ok(task!.body.includes(`Report ${ids[1]} by developer-2`));
    git.inHead = true;
    await assert.rejects(
      settleIntegration(deps(h, git), {
        integrationId: record.integrationId,
        outcome: "confirmed",
      }),
      /review of this integration is open/,
    );
    h.core.completeReview(ctx(h.core, reviewer.credential), {
      verdict: "findings",
      text: "wrong",
    });
    await assert.rejects(
      settleIntegration(deps(h, git), {
        integrationId: record.integrationId,
        outcome: "confirmed",
      }),
      /latest review passed/,
    );
    review(h, record.integrationId, "pass");
    git.inHead = false;
    await assert.rejects(
      settleIntegration(deps(h, git), {
        integrationId: record.integrationId,
        outcome: "confirmed",
      }),
      (error: unknown) =>
        error instanceof IntegrationError && error.code === "not_in_head",
    );
    git.inHead = true;
    const settled = await settleIntegration(deps(h, git), {
      integrationId: record.integrationId,
      outcome: "confirmed",
    });
    assert.equal(settled.record.state, "confirmed");
    assert.equal(settled.branchRemoved, true);
    assert.ok(git.calls.includes(`delete ${record.branch} ${HEAD}`));
    await assert.rejects(
      settleIntegration(deps(h, git), {
        integrationId: record.integrationId,
        outcome: "discarded",
      }),
      /only a merged one/,
    );
  } finally {
    await close(h);
  }
});

test("a discard removes the branch without needing a review", async () => {
  const h = await harness();
  try {
    const { ids } = reviewedPair(h);
    const git = fakeGit();
    const record = await integrate(deps(h, git), {
      reportIds: ids,
      requestedBy: "operator",
    });
    const settled = await settleIntegration(deps(h, git), {
      integrationId: record.integrationId,
      outcome: "discarded",
    });
    assert.equal(settled.record.state, "discarded");
    assert.ok(git.calls.includes(`delete ${record.branch} ${HEAD}`));
  } finally {
    await close(h);
  }
});

test("an integration left running by a stopped daemon is cleaned up and marked failed at start", async () => {
  const h = await harness();
  try {
    const { ids } = reviewedPair(h);
    h.core.beginIntegration(ctx(h.core, h.owner), {
      integrationId: "cut-off",
      reportIds: ids,
      baseSha: BASE,
      branch: "capstan/integration/cut-off",
      requestedBy: "operator",
    });
    const git = fakeGit();
    git.branchTip = async () => HEAD;
    await recoverIntegrations(deps(h, git));
    const record = h.core.integration("cut-off");
    assert.equal(record.state, "failed");
    assert.match(record.failureReason!, /daemon stopped/);
    assert.deepEqual(git.calls, [`delete capstan/integration/cut-off ${HEAD}`]);
    assert.deepEqual(h.core.runningIntegrations(h.owner), []);
  } finally {
    await close(h);
  }
});

test("the ledger refuses to rewrite an integration or its reports and to move a state backwards", async () => {
  const h = await harness();
  try {
    const { ids } = reviewedPair(h);
    const record = await integrate(deps(h, fakeGit()), {
      reportIds: ids,
      requestedBy: "operator",
    });
    h.core.close();
    const db = openSqlite(`${h.stateDirectory}/controller.sqlite`);
    try {
      assert.throws(
        () => db.exec(`UPDATE integrations SET base_sha = '${"e".repeat(40)}'`),
        /immutable/,
      );
      assert.throws(
        () =>
          db.exec(
            `UPDATE integrations SET state = 'running', head_sha = NULL, completed_at = NULL`,
          ),
        /only moves forward/,
      );
      assert.throws(() => db.exec("DELETE FROM integrations"), /immutable/);
      assert.throws(
        () =>
          db.exec(
            `INSERT INTO integration_reports SELECT project_id, integration_id, 9, report_id FROM integration_reports LIMIT 1`,
          ),
        /running integration/,
      );
      assert.throws(
        () =>
          db.exec(
            `UPDATE integrations SET branch = 'capstan/integration/other'`,
          ),
        /immutable/,
      );
      assert.throws(
        () => db.exec("DELETE FROM integration_reports"),
        /immutable/,
      );
      assert.throws(
        () => db.exec(`UPDATE integration_reports SET position = 9`),
        /immutable/,
      );
      assert.equal(record.state, "merged");
      const conflicted = (omitted: string) =>
        db.exec(
          `INSERT INTO integrations(project_id, integration_id, sequence, base_sha, branch, requested_by, state, conflict_report_id, conflict_files_json, conflict_files_omitted, created_at, completed_at)
           SELECT project_id, 'zz', 99, base_sha, 'capstan/integration/zz', 'operator', 'conflicted', 'r', '["a"]', ${omitted}, created_at, created_at FROM integrations LIMIT 1`,
        );
      assert.throws(() => conflicted("NULL"), /CHECK constraint/);
      assert.throws(() => conflicted("-1"), /CHECK constraint/);
      conflicted("0");
    } finally {
      db.close();
    }
  } finally {
    await close(h);
  }
});

test("a report already confirmed in an integration cannot be integrated again", async () => {
  const h = await harness();
  try {
    const { ids } = reviewedPair(h);
    const git = fakeGit();
    const record = await integrate(deps(h, git), {
      reportIds: ids,
      requestedBy: h.pm.agentId,
    });
    review(h, record.integrationId, "pass");
    git.inHead = true;
    await settleIntegration(deps(h, git), {
      integrationId: record.integrationId,
      outcome: "confirmed",
    });
    await assert.rejects(
      integrate(deps(h, git), {
        reportIds: [ids[0]!],
        requestedBy: h.pm.agentId,
      }),
      /already integrated and confirmed/,
    );
    assert.equal(
      h.core.integration(record.integrationId).completedAt !== null,
      true,
    );
  } finally {
    await close(h);
  }
});

test("a row left running is cleared by the next integration, which then proceeds", async () => {
  const h = await harness();
  try {
    const { ids } = reviewedPair(h);
    h.core.beginIntegration(ctx(h.core, h.owner), {
      integrationId: "stuck",
      reportIds: [ids[0]!],
      baseSha: BASE,
      branch: "capstan/integration/stuck",
      requestedBy: "operator",
    });
    const git = fakeGit();
    const record = await integrate(deps(h, git), {
      reportIds: ids,
      requestedBy: "operator",
    });
    assert.equal(record.state, "merged");
    assert.equal(h.core.integration("stuck").state, "failed");
  } finally {
    await close(h);
  }
});

test("an outcome that cannot be recorded does not leave the integration running or keep the branch", async () => {
  const h = await harness();
  try {
    const { ids } = reviewedPair(h);
    const git = fakeGit();
    const real = h.core.finishIntegration.bind(h.core);
    let first = true;
    h.core.finishIntegration = (context, input) => {
      if (first) {
        first = false;
        throw new Error("disk full");
      }
      return real(context, input);
    };
    await assert.rejects(
      integrate(deps(h, git), { reportIds: ids, requestedBy: "operator" }),
      /disk full/,
    );
    assert.deepEqual(h.core.runningIntegrations(h.owner), []);
    assert.ok(
      git.calls.some((c) => c.startsWith("delete capstan/integration/")),
    );
  } finally {
    await close(h);
  }
});

test("a report with an open review, or already in an unsettled merged integration, is refused", async () => {
  const h = await harness();
  try {
    const { ids } = reviewedPair(h);
    const reviewer = member(h, "open-reviewer", "Verifier", "reviewer");
    h.core.beginReview(ctx(h.core, h.pm.credential), {
      subjectId: ids[0]!,
      reviewerRole: "reviewer",
      reviewerAgentId: reviewer.agentId,
    });
    const git = fakeGit();
    await assert.rejects(
      integrate(deps(h, git), { reportIds: ids, requestedBy: "operator" }),
      /review still open/,
    );
    h.core.completeReview(ctx(h.core, reviewer.credential), {
      verdict: "pass",
      text: "ok",
    });
    const first = await integrate(deps(h, git), {
      reportIds: ids,
      requestedBy: "operator",
    });
    await assert.rejects(
      integrate(deps(h, git), {
        reportIds: [ids[0]!],
        requestedBy: "operator",
      }),
      /unsettled integration/,
    );
    await settleIntegration(deps(h, git), {
      integrationId: first.integrationId,
      outcome: "discarded",
    });
    const again = await integrate(deps(h, git), {
      reportIds: [ids[0]!],
      requestedBy: "operator",
    });
    assert.equal(again.state, "merged");
  } finally {
    await close(h);
  }
});

test("a branch that could not be deleted at settle is swept by the next integration", async () => {
  const h = await harness();
  try {
    const { ids } = reviewedPair(h);
    const git = fakeGit();
    const record = await integrate(deps(h, git), {
      reportIds: [ids[0]!],
      requestedBy: "operator",
    });
    let deletable = false;
    git.deleteBranch = async (branch, sha) => {
      git.calls.push(`delete ${branch} ${sha} ${deletable}`);
      return deletable;
    };
    git.branchTip = async () => HEAD;
    const settled = await settleIntegration(deps(h, git), {
      integrationId: record.integrationId,
      outcome: "discarded",
    });
    assert.equal(settled.branchRemoved, false);
    deletable = true;
    await integrate(deps(h, git), {
      reportIds: [ids[1]!],
      requestedBy: "operator",
    });
    assert.ok(
      git.calls.includes(`delete ${record.branch} ${HEAD} true`),
      git.calls.join("\n"),
    );
    const deletes = () =>
      git.calls.filter((c) => c.startsWith(`delete ${record.branch} `)).length;
    const before = deletes();
    await integrate(deps(h, git), {
      reportIds: [ids[0]!],
      requestedBy: "operator",
    });
    assert.equal(deletes(), before, "a swept branch is not tried again");
  } finally {
    await close(h);
  }
});

function approvedPlan(h: Harness, title: string, packages: [string, Member][]) {
  const architect = member(
    h,
    `architect-${title.replace(/\W/g, "")}`,
    "Developer",
    "developer",
  );
  const { planId } = h.core.openPlan(ctx(h.core, h.pm.credential), {
    tier: "normal",
    title,
  });
  h.core.submitPlan(ctx(h.core, architect.credential), {
    planId,
    bodyJson: JSON.stringify({
      summary: "s",
      packages: packages.map(([id]) => ({ id, title: id })),
    }),
    baseSha: BASE,
    review: false,
  });
  for (const [packageId, who] of packages)
    h.core.assignPackage(ctx(h.core, h.pm.credential), {
      planId,
      packageId,
      agentId: who.agentId,
    });
}

async function squashOf(h: Harness, ids: string[]) {
  const git = fakeGit();
  await integrate(deps(h, git), { reportIds: ids, requestedBy: h.pm.agentId });
  return git.inputs[0]!;
}

test("the squash message takes the plan title when the reports are packages of one plan, and the first summary otherwise", async () => {
  const h = await harness();
  try {
    withRoles(h);
    const a = h.developer;
    const b = member(h, "developer-2", "Developer", "developer");
    approvedPlan(h, "Plan Alpha", [
      ["wp1", a],
      ["wp2", b],
    ]);
    const ids = [
      reportBy(h, a, "1".repeat(40), "fix(core): repair the thing"),
      reportBy(h, b, "2".repeat(40), "second summary"),
    ];
    for (const id of ids) review(h, id, "pass");
    const input = await squashOf(h, ids);
    assert.equal(input.subject, "fix: Plan Alpha");
    assert.equal(
      input.body,
      `Report ${ids[0]} (${a.agentId}): fix(core): repair the thing\nReport ${ids[1]} (developer-2): second summary`,
    );
    assert.doesNotMatch(input.body, /Co-Authored-By|Claude/);
  } finally {
    await close(h);
  }
});

test("without one plan the squash subject is the first summary line, typed feat unless it names a type", async () => {
  const h = await harness();
  try {
    withRoles(h);
    const a = h.developer;
    const b = member(h, "developer-2", "Developer", "developer");
    const c = member(h, "developer-3", "Developer", "developer");
    approvedPlan(h, "Plan One", [["wp1", a]]);
    approvedPlan(h, "Plan Two", [["wp1", b]]);
    const ids = [
      reportBy(h, a, "1".repeat(40), "plain summary " + "x".repeat(100)),
      reportBy(h, b, "2".repeat(40), "feat: other"),
      reportBy(h, c, "3".repeat(40), "docs(x): unplanned"),
    ];
    for (const id of ids) review(h, id, "pass");
    const two = await squashOf(h, [ids[0]!, ids[1]!]);
    assert.equal(two.subject, "feat: plain summary");
    const unplanned = await squashOf(h, [ids[2]!]);
    assert.equal(unplanned.subject, "docs: docs(x): unplanned");
    assert.match(
      unplanned.subject,
      /^(feat|fix|refactor|docs|test|chore|style|perf|ci)(\([^)]+\))?: .{1,}/,
    );
  } finally {
    await close(h);
  }
});

test("the integration review task does not claim one merge per report", async () => {
  const h = await harness();
  try {
    const { ids } = reviewedPair(h);
    const record = await integrate(deps(h, fakeGit()), {
      reportIds: ids,
      requestedBy: h.pm.agentId,
    });
    const reviewer = member(h, "squash-reviewer", "Verifier", "reviewer");
    h.core.beginReview(ctx(h.core, h.pm.credential), {
      subjectId: record.integrationId,
      reviewerRole: "reviewer",
      reviewerAgentId: reviewer.agentId,
    });
    const [task] = h.core.messagesFor(reviewer.agentId);
    assert.ok(
      task!.body.includes(
        "The commit to review combines the reports below, in this order.",
      ),
    );
    assert.ok(!task!.body.includes("each as its own merge"));
  } finally {
    await close(h);
  }
});

const subjectOf = (planTitle: string | null, summary: string) =>
  squashMessage({
    planTitle,
    reports: [{ reportId: "r1", agentId: "developer-1", summary }],
  }).subject;

test("squash subject: the type comes from the plan title, then the first summary, and defaults to feat", () => {
  assert.equal(
    subjectOf("fix: Plan title", "docs: summary"),
    "fix: fix: Plan title",
  );
  assert.equal(subjectOf("Plan title", "docs(x): summary"), "docs: Plan title");
  // No type anywhere: feat is the documented default.
  assert.equal(subjectOf("Plan title", "plain summary"), "feat: Plan title");
  assert.equal(subjectOf(null, "plain summary"), "feat: plain summary");
});

test("squash subject: a long title is cut at a word boundary within 72 characters, without an ellipsis", () => {
  const title = Array.from({ length: 20 }, (_, i) => `word${i}x`).join(" ");
  assert.ok(title.length > 100 - 20);
  const subject = subjectOf(title, "plain");
  assert.ok(subject.length <= 72);
  assert.doesNotMatch(subject, /\.\.\./);
  assert.ok(title.startsWith(subject.slice("feat: ".length)));
  assert.match(subject, /word\d+x$/);
  const rest = title.slice(subject.length - "feat: ".length);
  assert.match(rest, /^\s/);
});

test("squash subject: a single word longer than the room is cut hard", () => {
  const subject = subjectOf("y".repeat(80), "plain");
  assert.equal(subject, `feat: ${"y".repeat(66)}`);
  assert.equal(subject.length, 72);
});

interface CoverageSetup {
  readonly h: Harness;
  readonly git: FakeGit;
  readonly logs: string[];
  readonly integrationId: string;
  readonly covering: string;
  readonly other: string;
  readonly deps: IntegrationDeps;
}

/** Report `covering` is held by the head without being merged; `other` is not. The integration holds a third report. */
async function mergedForCoverage(
  h: Harness,
  withCoveredReports: boolean,
): Promise<CoverageSetup> {
  withRoles(h);
  const merged = reportBy(h, h.developer, "1".repeat(40));
  const covering = reportBy(
    h,
    member(h, "developer-2", "Developer", "developer"),
    "2".repeat(40),
  );
  const other = reportBy(
    h,
    member(h, "developer-3", "Developer", "developer"),
    "3".repeat(40),
  );
  review(h, merged, "pass");
  const git = fakeGit();
  const logs: string[] = [];
  if (withCoveredReports)
    git.coveredReports = async (_head, reports) => {
      git.calls.push(`covered ${reports.map((r) => r.reportId).join(",")}`);
      return reports.flatMap((r) =>
        r.reportId === covering
          ? [{ reportId: r.reportId, how: "tree" as const }]
          : [],
      );
    };
  const d: IntegrationDeps = {
    ...deps(h, git),
    log: (event) => void logs.push(event),
  };
  const record = await integrate(d, {
    reportIds: [merged],
    requestedBy: "operator",
  });
  review(h, record.integrationId, "pass");
  git.inHead = true;
  return {
    h,
    git,
    logs,
    integrationId: record.integrationId,
    covering,
    other,
    deps: d,
  };
}

const candidateIds = (c: CoverageSetup): string[] | undefined =>
  c.h.core
    .coverageCandidates(c.h.owner, c.integrationId)
    ?.reports.map((r) => r.reportId);

test("confirming an integration records the reports its head holds, and not the others", async () => {
  const h = await harness();
  try {
    const c = await mergedForCoverage(h, true);
    assert.deepEqual(
      candidateIds(c),
      undefined,
      "a merged integration covers nothing yet",
    );
    await settleIntegration(c.deps, {
      integrationId: c.integrationId,
      outcome: "confirmed",
    });
    assert.deepEqual(candidateIds(c), [c.other]);
    assert.ok(c.logs.includes("integration_coverage_recorded"));
    // The same judgement again changes nothing.
    await recoverIntegrations(c.deps, "all");
    await recoverIntegrations(c.deps, "all");
    assert.deepEqual(candidateIds(c), [c.other]);
  } finally {
    await close(h);
  }
});

test("a discarded integration covers nothing", async () => {
  const h = await harness();
  try {
    const c = await mergedForCoverage(h, true);
    await settleIntegration(c.deps, {
      integrationId: c.integrationId,
      outcome: "discarded",
    });
    await recoverIntegrations(c.deps, "all");
    assert.deepEqual(candidateIds(c), undefined);
    assert.equal(
      c.git.calls.some((call) => call.startsWith("covered")),
      false,
    );
  } finally {
    await close(h);
  }
});

test("a git failure at confirm is logged and does not block it; the next start records the coverage", async () => {
  const h = await harness();
  try {
    const c = await mergedForCoverage(h, true);
    const working = c.git.coveredReports!;
    c.git.coveredReports = async () => {
      throw new Error("git is down");
    };
    const settled = await settleIntegration(c.deps, {
      integrationId: c.integrationId,
      outcome: "confirmed",
    });
    assert.equal(settled.record.state, "confirmed");
    assert.ok(c.logs.includes("integration_coverage_failed"));
    assert.deepEqual(candidateIds(c), [c.covering, c.other]);
    c.git.coveredReports = working;
    await recoverIntegrations(c.deps, "all");
    assert.deepEqual(candidateIds(c), [c.other]);
  } finally {
    await close(h);
  }
});

test("a skipped report is logged by the git layer and left uncovered", async () => {
  const h = await harness();
  try {
    const c = await mergedForCoverage(h, true);
    c.git.coveredReports = async (_head, reports, options) => {
      for (const r of reports)
        options?.onSkipped?.(r.reportId, "its commit does not exist");
      return [];
    };
    await settleIntegration(c.deps, {
      integrationId: c.integrationId,
      outcome: "confirmed",
    });
    assert.equal(
      c.logs.filter((e) => e === "integration_coverage_report_skipped").length,
      2,
    );
    assert.deepEqual(candidateIds(c), [c.covering, c.other]);
  } finally {
    await close(h);
  }
});

test("a confirmed integration whose coverage lacked a report gets its merge row at the next start, once", async () => {
  const h = await harness();
  try {
    const c = await mergedForCoverage(h, true);
    c.git.coveredReports = async () => [];
    await settleIntegration(c.deps, {
      integrationId: c.integrationId,
      outcome: "confirmed",
    });
    assert.deepEqual(candidateIds(c), [c.covering, c.other]);
    c.git.coveredReports = async (_head, reports) =>
      reports.map((r) => ({ reportId: r.reportId, how: "merge" as const }));
    await recoverIntegrations(c.deps, "all");
    assert.deepEqual(candidateIds(c), []);
    await recoverIntegrations(c.deps, "all");
    assert.deepEqual(candidateIds(c), []);
  } finally {
    await close(h);
  }
});

test("coverage candidates name the heads of other integrations that held the report, and skip one without a head", async () => {
  const h = await harness();
  try {
    const c = await mergedForCoverage(h, true);
    const db = openSqlite(`${h.stateDirectory}/controller.sqlite`);
    try {
      const projectId = (
        db.prepare("SELECT project_id FROM projects").get() as {
          project_id: string;
        }
      ).project_id;
      const add = (
        id: string,
        sequence: number,
        final: "discarded" | "failed",
      ) => {
        db.prepare(
          `INSERT INTO integrations(project_id, integration_id, sequence, base_sha, branch, requested_by, state, created_at)
           VALUES (?, ?, ?, ?, ?, 'operator', 'running', '2026-01-01T00:00:00.000Z')`,
        ).run(
          projectId,
          id,
          sequence,
          "a".repeat(40),
          `capstan/integration/${id}`,
        );
        db.prepare(
          "INSERT INTO integration_reports(project_id, integration_id, position, report_id) VALUES (?, ?, 1, ?)",
        ).run(projectId, id, c.other);
        if (final === "failed")
          db.prepare(
            "UPDATE integrations SET state = 'failed', failure_reason = 'x', completed_at = '2026-01-01T00:00:01.000Z' WHERE integration_id = ?",
          ).run(id);
        else {
          db.prepare(
            "UPDATE integrations SET state = 'merged', head_sha = ?, completed_at = '2026-01-01T00:00:01.000Z' WHERE integration_id = ?",
          ).run("b".repeat(40), id);
          db.prepare(
            "UPDATE integrations SET state = 'discarded' WHERE integration_id = ?",
          ).run(id);
        }
      };
      add("earlier-failed", 90, "failed");
      add("earlier-discarded", 91, "discarded");
    } finally {
      db.close();
    }
    await settleIntegration(c.deps, {
      integrationId: c.integrationId,
      outcome: "confirmed",
    });
    const other = h.core
      .coverageCandidates(h.owner, c.integrationId)
      ?.reports.find((r) => r.reportId === c.other);
    assert.deepEqual(other?.integrationHeads, ["b".repeat(40)]);
  } finally {
    await close(h);
  }
});
