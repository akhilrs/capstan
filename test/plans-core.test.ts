import assert from "node:assert/strict";
import { test } from "node:test";
import Database from "better-sqlite3";
import type { ReportEvidence } from "../src/controller/core.js";
import { integrate, type IntegrationGit } from "../src/integration.js";
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

function git(): IntegrationGit {
  return {
    headCommit: async () => BASE,
    commitExists: async () => true,
    merge: async () => ({ kind: "merged", headSha: HEAD }),
    branchTip: async () => null,
    isInHead: async () => false,
    deleteBranch: async () => true,
  };
}

async function integrateReports(
  h: Harness,
  reportIds: string[],
): Promise<string> {
  const record = await integrate(
    {
      core: h.core,
      git: git(),
      context: (credential: string) => ctx(h.core, credential),
      credential: h.owner,
      log: () => undefined,
    },
    { reportIds, requestedBy: "operator" },
  );
  assert.equal(record.state, "merged");
  return record.integrationId;
}

function body(...ids: string[]): string {
  return JSON.stringify({
    summary: "s",
    packages: ids.map((id) => ({ id, title: id })),
  });
}

interface Team {
  readonly h: Harness;
  readonly architect: Member;
  readonly dev1: Member;
  readonly dev2: Member;
}

async function team(clock?: () => Date): Promise<Team> {
  const h = await harness(clock === undefined ? {} : { clock });
  withRoles(h);
  return {
    h,
    architect: member(h, "architect", "Developer", "developer"),
    dev1: h.developer,
    dev2: member(h, "dev-two", "Developer", "developer"),
  };
}

function open(
  h: Harness,
  tier: "normal" | "high_risk" = "normal",
  supersedes?: string,
): string {
  return h.core.openPlan(ctx(h.core, h.pm.credential), {
    tier,
    title: "a plan",
    ...(supersedes === undefined ? {} : { supersedesPlanId: supersedes }),
  }).planId;
}

function submit(
  t: Team,
  planId: string,
  json: string,
  review = false,
  who: Member = t.architect,
) {
  return t.h.core.submitPlan(ctx(t.h.core, who.credential), {
    planId,
    bodyJson: json,
    baseSha: BASE,
    review,
  });
}

function approved(t: Team, ...ids: string[]): string {
  const planId = open(t.h);
  submit(t, planId, body(...ids));
  return planId;
}

function progress(t: Team, planId: string, packageId: string): string {
  return t.h.core
    .planRecord(t.h.owner, planId)!
    .packages.find((p) => p.packageId === packageId)!.progress;
}

function assign(t: Team, planId: string, packageId: string, who: Member) {
  return t.h.core.assignPackage(ctx(t.h.core, t.h.pm.credential), {
    planId,
    packageId,
    agentId: who.agentId,
  });
}

function raw<T>(h: Harness, use: (db: Database.Database) => T): T {
  h.core.close();
  const db = new Database(`${h.stateDirectory}/controller.sqlite`);
  try {
    return use(db);
  } finally {
    db.close();
  }
}

test("only the PM or the operator opens a plan, and only an approved plan can be superseded", async () => {
  const t = await team();
  try {
    const { h } = t;
    const first = h.core.openPlan(ctx(h.core, h.pm.credential), {
      tier: "high_risk",
      title: "first",
    });
    assert.equal(first.planId, "plan-1");
    assert.equal(first.state, "draft");
    assert.equal(first.tier, "high_risk");
    assert.equal(
      h.core.openPlan(ctx(h.core, h.owner), { tier: "normal", title: "two" })
        .planId,
      "plan-2",
    );
    assert.throws(
      () =>
        h.core.openPlan(ctx(h.core, t.architect.credential), {
          tier: "normal",
          title: "x",
        }),
      /only the PM or the operator/,
    );
    const verifier = member(h, "rev", "Verifier", "reviewer");
    assert.throws(
      () =>
        h.core.openPlan(ctx(h.core, verifier.credential), {
          tier: "normal",
          title: "x",
        }),
      /lacks plan:write/,
    );
    assert.throws(
      () =>
        h.core.openPlan(ctx(h.core, h.pm.credential), {
          tier: "small" as "normal",
          title: "x",
        }),
      /normal or high_risk/,
    );
    assert.throws(() => open(h, "normal", "plan-9"), /does not exist/);
    assert.throws(() => open(h, "normal", "plan-1"), /only an approved plan/);
  } finally {
    await close(t.h);
  }
});

test("a normal plan is approved at once with one package row per package, and the first submitter is its architect", async () => {
  const t = await team();
  try {
    const { h } = t;
    const planId = open(h);
    const record = submit(t, planId, body("wp2", "wp1"));
    assert.equal(record.state, "approved");
    assert.equal(record.currentRevision, 1);
    assert.equal(record.approvedRevision, 1);
    assert.equal(record.architectAgentId, "architect");
    const detail = h.core.planRecord(h.owner, planId)!;
    assert.deepEqual(
      detail.packages.map((p) => [p.packageId, p.assigneeAgentId, p.progress]),
      [
        ["wp1", null, "unassigned"],
        ["wp2", null, "unassigned"],
      ],
    );
    assert.equal(detail.revision?.bodyJson, body("wp2", "wp1"));
    assert.equal(detail.revision?.baseSha, BASE);
    assert.throws(
      () => submit(t, planId, body("wp1")),
      /is approved, not a draft/,
    );
    assert.deepEqual(
      h.core.listPlans(h.owner).map((p) => [p.planId, p.state]),
      [[planId, "approved"]],
    );
    assert.equal(h.core.planRecord(h.owner, "plan-9"), undefined);
  } finally {
    await close(t.h);
  }
});

test("a submit needs a developer-kind agent and a body with unique, well-formed package ids", async () => {
  const t = await team();
  try {
    const { h } = t;
    const planId = open(h);
    assert.throws(
      () => submit(t, planId, body("wp1"), false, h.pm),
      /developer-kind agent/,
    );
    const bad: Array<[string, RegExp]> = [
      ["not json", /must be JSON/],
      ["[]", /1 to 20 packages/],
      [JSON.stringify({ packages: [] }), /1 to 20 packages/],
      [body("wp1", "wp1"), /unique/],
      [body("WP"), /lowercase/],
      [body("1a"), /lowercase/],
      [JSON.stringify({ packages: [{ title: "x" }] }), /lowercase/],
      [
        JSON.stringify({ packages: [{ id: "wp1" }], pad: "x".repeat(40_000) }),
        /at most 32768 bytes/,
      ],
      [
        body(...Array.from({ length: 21 }, (_, i) => `wp${i}`)),
        /1 to 20 packages/,
      ],
    ];
    for (const [json, message] of bad)
      assert.throws(() => submit(t, planId, json), message);
    assert.equal(h.core.planRecord(h.owner, planId)!.plan.state, "draft");
    assert.throws(
      () =>
        t.h.core.submitPlan(ctx(h.core, t.architect.credential), {
          planId,
          bodyJson: body("wp1"),
          baseSha: "nope",
          review: false,
        }),
      /full lowercase sha1/,
    );
  } finally {
    await close(t.h);
  }
});

test("a reviewed plan waits in review, takes revisions one by one and keeps its architect", async () => {
  const t = await team();
  try {
    const { h } = t;
    const planId = open(h, "high_risk");
    const record = submit(t, planId, body("wp1"), true);
    assert.equal(record.state, "in_review");
    assert.equal(record.approvedRevision, null);
    assert.equal(
      h.core.planRecord(h.owner, planId)!.packages.length,
      0,
      "no package rows before approval",
    );
    assert.throws(
      () => submit(t, planId, body("wp1")),
      /in_review, not a draft/,
    );
    raw(h, (db) =>
      db.exec("UPDATE plans SET state = 'draft' WHERE plan_id = 'plan-1'"),
    );
    const reopened = await ReopenedCore.of(h);
    try {
      assert.throws(
        () =>
          reopened.core.submitPlan(ctx(reopened.core, t.dev1.credential), {
            planId,
            bodyJson: body("wp1"),
            baseSha: BASE,
            review: true,
          }),
        /belongs to the architect architect/,
      );
      const second = reopened.core.submitPlan(
        ctx(reopened.core, t.architect.credential),
        { planId, bodyJson: body("wp1", "wp2"), baseSha: BASE, review: false },
      );
      assert.equal(second.currentRevision, 2);
      assert.equal(second.approvedRevision, 2);
      const detail = reopened.core.planRecord(h.owner, planId)!;
      assert.equal(detail.revision?.revision, 2);
      assert.deepEqual(
        detail.packages.map((p) => p.packageId),
        ["wp1", "wp2"],
      );
    } finally {
      reopened.core.close();
    }
  } finally {
    await close(t.h);
  }
});

/** A second core over the same state directory, for a test that edited the ledger with raw SQL. */
class ReopenedCore {
  private constructor(readonly core: Harness["core"]) {}
  static async of(h: Harness): Promise<ReopenedCore> {
    const { ControllerCore } = await import("../src/controller/core.js");
    return new ReopenedCore(
      await ControllerCore.open({
        stateDirectory: h.stateDirectory,
        project: h.info,
      }),
    );
  }
}

test("a superseding plan retires the old one at approval, and a lost race rolls the approval back", async () => {
  const t = await team();
  try {
    const { h } = t;
    const a = approved(t, "wp1");
    const b = open(h, "normal", a);
    const c = open(h, "normal", a);
    submit(t, b, body("wp1", "wp2"));
    const old = h.core.planRecord(h.owner, a)!;
    assert.equal(old.plan.state, "superseded");
    assert.equal(old.plan.approvedRevision, 1);
    assert.equal(h.core.planRecord(h.owner, b)!.plan.supersedesPlanId, a);
    assert.throws(
      () => submit(t, c, body("wp9")),
      /no longer approved and cannot be superseded/,
    );
    const lost = h.core.planRecord(h.owner, c)!;
    assert.equal(lost.plan.state, "draft", "the failed approval left no trace");
    assert.equal(lost.plan.currentRevision, 0);
    assert.equal(lost.packages.length, 0);
  } finally {
    await close(t.h);
  }
});

test("the ledger refuses plan updates that skip a state, rewrite history or add stray rows", async () => {
  const t = await team();
  try {
    const { h } = t;
    const a = approved(t, "wp1");
    const draft = open(h);
    const reviewed = open(h, "high_risk");
    submit(t, reviewed, body("wp1"), true);
    assign(t, a, "wp1", t.dev1);
    const r1 = reportBy(h, t.dev1, "1".repeat(40));
    review(h, r1, "pass");
    const integrationId = await integrateReports(h, [r1]);
    review(h, integrationId, "pass");
    h.core.recordSignoff(ctx(h.core, t.architect.credential), {
      planId: a,
      integrationId,
      summary: "ok",
    });
    raw(h, (db) => {
      const fails = (sql: string, message: RegExp): void =>
        assert.throws(() => db.exec(sql), message, sql);
      fails(
        `UPDATE plans SET state = 'superseded' WHERE plan_id = '${draft}'`,
        /only moves forward/,
      );
      fails(
        `UPDATE plans SET state = 'draft' WHERE plan_id = '${draft}'`,
        /only moves forward/,
      );
      fails(
        `UPDATE plans SET state = 'draft' WHERE plan_id = '${a}'`,
        /only moves forward/,
      );
      fails(
        `UPDATE plans SET state = 'in_review' WHERE plan_id = '${reviewed}'`,
        /only moves forward/,
      );
      fails(
        `UPDATE plans SET state = 'approved', approved_revision = 1 WHERE plan_id = '${draft}'`,
        /FOREIGN KEY|CHECK/,
      );
      fails(
        `UPDATE plans SET current_revision = 5 WHERE plan_id = '${draft}'`,
        /counts up by one/,
      );
      fails(
        `UPDATE plans SET approved_revision = NULL WHERE plan_id = '${a}'`,
        /is final/,
      );
      fails(
        `UPDATE plans SET tier = 'high_risk' WHERE plan_id = '${a}'`,
        /immutable/,
      );
      fails(`UPDATE plans SET title = 'x' WHERE plan_id = '${a}'`, /immutable/);
      fails(
        `UPDATE plans SET architect_agent_id = 'dev-two' WHERE plan_id = '${a}'`,
        /is final/,
      );
      fails(`DELETE FROM plans WHERE plan_id = '${a}'`, /immutable/);
      fails(
        `UPDATE plan_revisions SET body_json = '{}' WHERE plan_id = '${a}'`,
        /immutable/,
      );
      fails(`DELETE FROM plan_revisions WHERE plan_id = '${a}'`, /immutable/);
      const copy = (planId: string, revision: number): string =>
        `INSERT INTO plan_revisions SELECT project_id, '${planId}', ${revision}, base_sha, body_json, body_sha, author_agent_id, author_actor_id, created_at FROM plan_revisions WHERE plan_id = '${a}'`;
      fails(copy(a, 2), /next revision/);
      fails(copy(draft, 2), /next revision/);
      fails(copy(reviewed, 2), /next revision/);
      fails(
        `INSERT INTO plan_packages(project_id, plan_id, package_id) SELECT project_id, '${draft}', 'wp1' FROM plans LIMIT 1`,
        /approved revision/,
      );
      fails(
        `INSERT INTO plan_packages(project_id, plan_id, package_id) SELECT project_id, '${a}', 'wp9' FROM plans LIMIT 1`,
        /approved revision/,
      );
      fails(
        `UPDATE plan_packages SET package_id = 'wp9' WHERE plan_id = '${a}'`,
        /immutable/,
      );
      fails(`DELETE FROM plan_packages WHERE plan_id = '${a}'`, /immutable/);
      fails(`UPDATE plan_signoffs SET summary = 'x'`, /immutable/);
      fails(`DELETE FROM plan_signoffs`, /immutable/);
      fails(
        `UPDATE plan_packages SET assignee_agent_id = NULL WHERE plan_id = '${a}'`,
        /CHECK/,
      );
      assert.deepEqual(db.pragma("foreign_key_check"), []);
      db.exec(`UPDATE plans SET state = 'draft' WHERE plan_id = '${reviewed}'`);
    });
  } finally {
    await close(t.h);
  }
});

test("a package is assigned once, to an active developer that is not the architect", async () => {
  const t = await team();
  try {
    const { h } = t;
    const draft = open(h);
    assert.throws(
      () => assign(t, draft, "wp1", t.dev1),
      /packages are assigned once it is approved/,
    );
    const planId = approved(t, "wp1", "wp2");
    assert.throws(() => assign(t, "plan-9", "wp1", t.dev1), /does not exist/);
    assert.throws(() => assign(t, planId, "wp9", t.dev1), /has no package wp9/);
    assert.throws(
      () => assign(t, planId, "wp1", t.architect),
      /architect cannot hold a package/,
    );
    assert.throws(
      () => assign(t, planId, "wp1", { ...t.dev1, agentId: "pm-agent" }),
      /not an active developer-kind agent/,
    );
    assert.throws(
      () => assign(t, planId, "wp1", { ...t.dev1, agentId: "nobody" }),
      /not an active developer-kind agent/,
    );
    assert.throws(
      () =>
        h.core.assignPackage(ctx(h.core, t.architect.credential), {
          planId,
          packageId: "wp1",
          agentId: t.dev1.agentId,
        }),
      /only the PM or the operator/,
    );
    const bound = assign(t, planId, "wp1", t.dev1);
    assert.equal(bound.assigneeAgentId, "developer-agent");
    assert.equal(bound.progress, "assigned");
    assert.ok(bound.assignmentMessageId !== null);
    assert.equal(
      h.core.message(bound.assignmentMessageId)!.recipientAgentId,
      "developer-agent",
    );
    assert.throws(
      () => assign(t, planId, "wp1", t.dev2),
      /already assigned to developer-agent/,
    );
    assert.throws(() => assign(t, planId, "wp1", t.dev1), /already assigned/);
    assert.throws(
      () => assign(t, planId, "wp2", t.dev1),
      /already holds package wp1/,
    );
    h.core.endAgent(ctx(h.core, h.owner), t.dev1.agentId);
    const rebound = assign(t, planId, "wp1", t.dev2);
    assert.equal(rebound.assigneeAgentId, "dev-two");
    assert.equal(progress(t, planId, "wp2"), "unassigned");
  } finally {
    await close(t.h);
  }
});

test("package progress follows the assignee's latest report through review and integration", async () => {
  const t = await team();
  try {
    const { h } = t;
    const planId = approved(t, "wp1", "wp2");
    assert.equal(progress(t, planId, "wp1"), "unassigned");
    assign(t, planId, "wp1", t.dev1);
    assign(t, planId, "wp2", t.dev2);
    assert.equal(progress(t, planId, "wp1"), "assigned");
    const first = reportBy(h, t.dev1, "1".repeat(40));
    assert.equal(progress(t, planId, "wp1"), "reported");
    assert.equal(
      progress(t, planId, "wp2"),
      "assigned",
      "other packages are unaffected",
    );
    review(h, first, "findings");
    assert.equal(progress(t, planId, "wp1"), "findings");
    const second = reportBy(h, t.dev1, "2".repeat(40));
    assert.equal(
      progress(t, planId, "wp1"),
      "reported",
      "a newer report restarts the cycle",
    );
    review(h, second, "pass");
    assert.equal(progress(t, planId, "wp1"), "reviewed");
    await integrateReports(h, [second]);
    assert.equal(progress(t, planId, "wp1"), "integrated");
    assert.equal(progress(t, planId, "wp2"), "assigned");
    reportBy(h, t.dev2, "3".repeat(40));
    assert.equal(progress(t, planId, "wp2"), "reported");
  } finally {
    await close(t.h);
  }
});

test("package progress ignores reports before the assignment and counts a report for one package only", async () => {
  let tick = Date.parse("2026-10-01T00:00:00.000Z");
  const t = await team(() => new Date(tick));
  const advance = () => {
    tick += 1000;
  };
  try {
    const { h } = t;
    const first = approved(t, "wp1");
    const second = approved(t, "wp1");
    const early = reportBy(h, t.dev1, "1".repeat(40));
    advance();
    assign(t, first, "wp1", t.dev1);
    assert.equal(
      progress(t, first, "wp1"),
      "assigned",
      "a report accepted before the assignment does not count",
    );
    advance();
    const own = reportBy(h, t.dev1, "2".repeat(40));
    assert.notEqual(own, early);
    assert.equal(progress(t, first, "wp1"), "reported");
    advance();
    assign(t, second, "wp1", t.dev1);
    assert.equal(
      progress(t, second, "wp1"),
      "assigned",
      "the report of the first assignment does not count for the second",
    );
    assert.equal(progress(t, first, "wp1"), "reported");
    advance();
    reportBy(h, t.dev1, "3".repeat(40));
    assert.equal(progress(t, second, "wp1"), "reported");
    assert.equal(
      progress(t, first, "wp1"),
      "reported",
      "the first package keeps the report that preceded the second assignment",
    );
  } finally {
    await close(t.h);
  }
});

test("a sign-off needs the architect, a merged integration of this plan's packages and a passed integration review", async () => {
  const t = await team();
  try {
    const { h } = t;
    const planId = approved(t, "wp1");
    const other = approved(t, "wp1");
    assign(t, planId, "wp1", t.dev1);
    const report = reportBy(h, t.dev1, "1".repeat(40));
    review(h, report, "pass");
    const integrationId = await integrateReports(h, [report]);
    const signoff = (
      who: Member,
      plan: string,
      integration: string,
      summary = "looks right",
    ) =>
      h.core.recordSignoff(ctx(h.core, who.credential), {
        planId: plan,
        integrationId: integration,
        summary,
      });
    assert.throws(
      () => signoff(t.dev1, planId, integrationId),
      /only the plan's architect/,
    );
    assert.throws(
      () => signoff(t.architect, "plan-9", integrationId),
      /does not exist/,
    );
    assert.throws(
      () => signoff(t.architect, planId, "nope"),
      /integration nope does not exist/,
    );
    assert.throws(
      () => signoff(t.architect, other, integrationId),
      /not a package of plan/,
      "an integration of another plan's work is refused",
    );
    assert.throws(
      () => signoff(t.architect, planId, integrationId),
      /no passed review/,
    );
    review(h, integrationId, "findings");
    assert.throws(
      () => signoff(t.architect, planId, integrationId),
      /no passed review/,
    );
    review(h, integrationId, "pass");
    assert.throws(
      () => signoff(t.architect, planId, integrationId, "  "),
      /visible text/,
    );
    const done = signoff(t.architect, planId, integrationId);
    assert.deepEqual(
      [done.integrationId, done.architectAgentId, done.summary],
      [integrationId, "architect", "looks right"],
    );
    assert.deepEqual(
      h.core.planRecord(h.owner, planId)!.signoffs.map((s) => s.integrationId),
      [integrationId],
    );
    assert.throws(
      () => signoff(t.architect, planId, integrationId),
      /already signed off/,
    );
  } finally {
    await close(t.h);
  }
});

test("a sign-off refuses a report outside the plan's packages", async () => {
  const t = await team();
  try {
    const { h } = t;
    const planId = approved(t, "wp1");
    assign(t, planId, "wp1", t.dev1);
    const own = reportBy(h, t.dev1, "1".repeat(40));
    const stray = reportBy(h, t.dev2, "2".repeat(40));
    review(h, own, "pass");
    review(h, stray, "pass");
    const integrationId = await integrateReports(h, [own, stray]);
    review(h, integrationId, "pass");
    assert.throws(
      () =>
        h.core.recordSignoff(ctx(h.core, t.architect.credential), {
          planId,
          integrationId,
          summary: "x",
        }),
      new RegExp(`report ${stray} of the integration is not a package`),
    );
  } finally {
    await close(t.h);
  }
});

test("a sign-off needs a merged integration", async () => {
  const t = await team();
  try {
    const { h } = t;
    const planId = approved(t, "wp1");
    assign(t, planId, "wp1", t.dev1);
    const report = reportBy(h, t.dev1, "1".repeat(40));
    review(h, report, "pass");
    const integrationId = await integrateReports(h, [report]);
    review(h, integrationId, "pass");
    h.core.settleIntegration(ctx(h.core, h.owner), {
      integrationId,
      outcome: "discarded",
    });
    assert.throws(
      () =>
        h.core.recordSignoff(ctx(h.core, t.architect.credential), {
          planId,
          integrationId,
          summary: "x",
        }),
      /is discarded, not merged/,
    );
  } finally {
    await close(t.h);
  }
});

test("migration 0022 adds the plan tables and grants the plan capabilities to existing actors", async () => {
  const t = await team();
  try {
    const { h } = t;
    const planId = approved(t, "wp1");
    assert.ok(planId);
    h.core.close();
    const databasePath = `${h.stateDirectory}/controller.sqlite`;
    const db = new Database(databasePath);
    try {
      for (const table of [
        "plan_signoffs",
        "plan_packages",
        "plan_revisions",
        "plans",
      ])
        db.exec(`DROP TABLE ${table}`);
      db.exec(
        "DELETE FROM capability_grants WHERE capability LIKE 'plan:%' OR (capability = 'review:request' AND actor_id IN (SELECT actor_id FROM actors WHERE role = 'Developer'))",
      );
      db.exec(
        "DELETE FROM role_capabilities WHERE capability LIKE 'plan:%' OR (role = 'Developer' AND capability = 'review:request')",
      );
      db.exec("DELETE FROM schema_migrations WHERE version >= 22");
    } finally {
      db.close();
    }
    const reopened = await ReopenedCore.of(h);
    try {
      assert.deepEqual(reopened.core.listPlans(h.owner), []);
      const again = reopened.core.openPlan(
        ctx(reopened.core, h.pm.credential),
        {
          tier: "normal",
          title: "after migration",
        },
      );
      assert.equal(again.planId, "plan-1");
      reopened.core.submitPlan(ctx(reopened.core, t.architect.credential), {
        planId: again.planId,
        bodyJson: body("wp1"),
        baseSha: BASE,
        review: false,
      });
      assert.equal(
        reopened.core.planRecord(h.owner, again.planId)!.plan.state,
        "approved",
        "the architect developer was granted plan:write by the migration",
      );
    } finally {
      reopened.core.close();
    }
    const check = new Database(databasePath);
    try {
      assert.deepEqual(check.pragma("foreign_key_check"), []);
      assert.deepEqual(
        check
          .prepare(
            "SELECT version, name FROM schema_migrations WHERE version = 22",
          )
          .all(),
        [{ version: 22, name: "0022_plans.sql" }],
      );
    } finally {
      check.close();
    }
  } finally {
    await close(t.h);
  }
});

test("a sign-off refuses an integration without reports", async () => {
  const t = await team();
  try {
    const { h } = t;
    const planId = approved(t, "wp1");
    assign(t, planId, "wp1", t.dev1);
    const report = reportBy(h, t.dev1, "1".repeat(40));
    review(h, report, "pass");
    const integrationId = await integrateReports(h, [report]);
    review(h, integrationId, "pass");
    const db = new Database(`${h.stateDirectory}/controller.sqlite`);
    try {
      db.exec("DROP TRIGGER immutable_integration_reports_delete");
      db.exec("DELETE FROM integration_reports");
    } finally {
      db.close();
    }
    assert.throws(
      () =>
        h.core.recordSignoff(ctx(h.core, t.architect.credential), {
          planId,
          integrationId,
          summary: "empty",
        }),
      /has no reports of plan/,
    );
  } finally {
    await close(t.h);
  }
});
