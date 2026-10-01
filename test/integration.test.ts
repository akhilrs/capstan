import assert from "node:assert/strict";
import { test } from "node:test";
import type { ReportEvidence } from "../src/controller/core.js";
import {
  IntegrationError,
  integrate,
  recoverIntegrations,
  settleIntegration,
  type IntegrationGit,
} from "../src/integration.js";
import type { MergeResult } from "../src/git.js";
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

function reportBy(h: Harness, author: Member, commit: string): string {
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
    summary: `work of ${author.agentId}`,
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
  mergeResult: MergeResult;
  inHead: boolean;
}

function fakeGit(): FakeGit {
  const fake: FakeGit = {
    calls: [],
    mergeResult: { kind: "merged", headSha: HEAD },
    inHead: false,
    headCommit: async () => BASE,
    commitExists: async () => true,
    merge: async (input) => {
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
      files: ["src/a.ts", "src/b.ts"],
    };
    const before = h.core.messagesFor(h.pm.agentId).length;
    const record = await integrate(deps(h, git), {
      reportIds: ids,
      requestedBy: "operator",
    });
    assert.equal(record.state, "conflicted");
    assert.equal(record.conflictReportId, ids[1]);
    assert.deepEqual(record.conflictFiles, ["src/a.ts", "src/b.ts"]);
    assert.equal(record.headSha, null);
    const messages = h.core.messagesFor(h.pm.agentId);
    assert.equal(messages.length, before + 1);
    const notice = messages.at(-1)!;
    assert.ok(notice.body.includes("blocked by a merge conflict"));
    assert.ok(notice.body.includes("src/a.ts, src/b.ts"));
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
    const { default: Database } = await import("better-sqlite3");
    h.core.close();
    const db = new Database(`${h.stateDirectory}/controller.sqlite`);
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
        () => db.exec("DELETE FROM integration_reports"),
        /immutable/,
      );
      assert.throws(
        () => db.exec(`UPDATE integration_reports SET position = 9`),
        /immutable/,
      );
      assert.equal(record.state, "merged");
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
