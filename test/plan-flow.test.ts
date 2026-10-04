import assert from "node:assert/strict";
import { test } from "node:test";
import type { CapstanConfig } from "../src/config/capstan-config.js";
import type { ReportEvidence } from "../src/controller/core.js";
import { buildRolePrompt } from "../src/prompts.js";
import {
  call,
  close,
  ctx,
  harness,
  type Harness,
  type Member,
} from "./harness.js";

const BASE = "a".repeat(40);
const HEAD = "d".repeat(40);

const architectConfig = {
  enabled: true,
  role: "architect",
  planReview: "high_risk",
  reviewerRole: null,
  maxPackages: 8,
  countTowardWorkerLimit: false,
  highRiskTriggers: ["schema or migrations"],
} as const;

const config = {
  architect: architectConfig,
  roles: [
    { name: "pm", kind: "PM" },
    { name: "developer", kind: "Developer" },
    { name: "architect", kind: "Developer" },
    { name: "reviewer", kind: "Verifier" },
  ],
} as unknown as CapstanConfig;

interface Stub {
  inHead: boolean;
}

function member(
  h: Harness,
  name: string,
  role: "Developer" | "Verifier",
  roleName: string,
): Member {
  h.core.createSeat(ctx(h.core, h.owner), {
    seatId: `${name}-seat`,
    name,
    role,
  });
  const actor = h.core.createActor(ctx(h.core, h.owner), {
    displayName: name,
    role,
    seatId: `${name}-seat`,
  });
  h.core.registerAgent(ctx(h.core, h.owner), {
    agentId: name,
    roleName,
    seatId: `${name}-seat`,
    actorId: actor.actorId,
  });
  return {
    agentId: name,
    credential: actor.credential,
    actorId: actor.actorId,
  };
}

interface Team {
  readonly h: Harness;
  readonly stub: Stub;
  readonly architect: Member;
  readonly devA: Member;
  readonly devB: Member;
  readonly reviewers: Member[];
}

async function withTeam(run: (t: Team) => Promise<void>): Promise<void> {
  const stub: Stub = { inHead: false };
  const reviewers: Member[] = [];
  const holder: { h?: Harness } = {};
  const h = await harness({
    commands: {
      config,
      commitExists: async () => true,
      integrationGit: {
        headCommit: async () => BASE,
        commitExists: async () => true,
        merge: async () => ({ kind: "merged", headSha: HEAD }),
        branchTip: async () => null,
        isInHead: async () => stub.inHead,
        deleteBranch: async () => true,
      },
      launcher: {
        spawn: async () => {
          const reviewer = member(
            holder.h!,
            `reviewer-${reviewers.length + 1}`,
            "Verifier",
            "reviewer",
          );
          reviewers.push(reviewer);
          return { state: "started", agentId: reviewer.agentId };
        },
        release: async (agentId: string) => ({ state: "released", agentId }),
        status: () => ({}),
      } as never,
    },
  });
  holder.h = h;
  try {
    h.core.syncRoleDefinitions(
      ctx(h.core, h.owner),
      [
        ["pm", "PM"],
        ["pm2", "PM"],
        ["developer", "Developer"],
        ["architect", "Developer"],
        ["reviewer", "Verifier"],
      ].map(([name, kind], index) => ({
        name: name!,
        kind: kind as "PM" | "Developer" | "Verifier",
        host: "claude",
        configHash: String.fromCharCode(97 + index).repeat(64),
      })),
    );
    await run({
      h,
      stub,
      architect: member(h, "architect", "Developer", "architect"),
      devA: member(h, "dev-a", "Developer", "developer"),
      devB: member(h, "dev-b", "Developer", "developer"),
      reviewers,
    });
  } finally {
    await close(h);
  }
}

function planBody(...ids: string[]): string {
  return JSON.stringify({
    summary: "split the work",
    packages: ids.map((id) => ({
      id,
      title: `package ${id}`,
      owns: [`src/${id}/`],
      interfaces: [`exports ${id}()`],
      acceptance: [`${id} works`],
      estimate_hours: 2,
    })),
  });
}

function resultOf<T>(response: Awaited<ReturnType<typeof call>>): T {
  assert.ok(response.ok, JSON.stringify(response));
  return (response as unknown as { result: T }).result;
}

function codeOf(response: Awaited<ReturnType<typeof call>>): string {
  return response.ok ? "ok" : response.code;
}

function bodies(h: Harness, agentId: string): string[] {
  return h.core.messagesFor(agentId).map((m) => m.body);
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
  return h.core.recordAgentReport(ctx(h.core, author.credential), {
    commitSha: commit,
    summary: `work of ${author.agentId}`,
    evidence,
  }).record.reportId;
}

async function verdict(
  t: Team,
  reviewer: Member,
  kind: "pass" | "findings",
): Promise<void> {
  resultOf(await call(t.h, reviewer.credential, "review", [kind, `${kind}`]));
}

async function passReview(
  t: Team,
  requester: Member,
  subjectId: string,
): Promise<void> {
  resultOf(
    await call(t.h, requester.credential, "request-review", [subjectId]),
  );
  await verdict(t, t.reviewers.at(-1)!, "pass");
}

async function openAndSubmit(
  t: Team,
  tier: "normal" | "high-risk",
  ...ids: string[]
): Promise<string> {
  const { planId } = resultOf<{ planId: string }>(
    await call(t.h, t.h.pm.credential, "plan", ["open", tier, "a plan"]),
  );
  resultOf(
    await call(t.h, t.architect.credential, "plan", [
      "submit",
      planId,
      planBody(...ids),
    ]),
  );
  return planId;
}

async function assign(
  t: Team,
  planId: string,
  packageId: string,
  dev: Member,
): Promise<void> {
  resultOf(
    await call(t.h, t.h.pm.credential, "plan", [
      "assign",
      planId,
      packageId,
      dev.agentId,
    ]),
  );
}

/** Two packages assigned, reported, reviewed and integrated; returns the passed integration. */
async function integratedPlan(
  t: Team,
  planId: string,
): Promise<{ integrationId: string; reportIds: string[] }> {
  await assign(t, planId, "wp1", t.devA);
  await assign(t, planId, "wp2", t.devB);
  const reportIds = [
    reportBy(t.h, t.devA, "b".repeat(40)),
    reportBy(t.h, t.devB, "c".repeat(40)),
  ];
  for (const reportId of reportIds) await passReview(t, t.architect, reportId);
  const { integrationId, state } = resultOf<{
    integrationId: string;
    state: string;
  }>(await call(t.h, t.architect.credential, "integrate", reportIds));
  assert.equal(state, "merged");
  await passReview(t, t.architect, integrationId);
  return { integrationId, reportIds };
}

async function signoffAndConfirm(
  t: Team,
  planId: string,
  integrationId: string,
): Promise<void> {
  const { h } = t;
  const before = bodies(h, h.pm.agentId).length;
  resultOf(
    await call(h, t.architect.credential, "plan", [
      "signoff",
      planId,
      integrationId,
      "all packages integrated",
    ]),
  );
  const notices = bodies(h, h.pm.agentId)
    .slice(before)
    .filter((b) => b.startsWith(`Plan ${planId} signed off`));
  assert.equal(notices.length, 1);
  assert.ok(notices[0]!.includes(h.core.integration(integrationId).branch));
  assert.ok(notices[0]!.includes(HEAD));
  assert.ok(notices[0]!.includes(`the user merges it into the project's HEAD`));
  assert.ok(
    notices[0]!.includes(`run cstan integrate confirm ${integrationId}`),
  );
  assert.equal(
    h.core.planRecord(h.owner, planId)!.signoffs[0]!.integrationId,
    integrationId,
  );

  // The user has not merged yet: nobody can confirm.
  assert.equal(
    codeOf(
      await call(h, t.architect.credential, "integrate", [
        "confirm",
        integrationId,
      ]),
    ),
    "forbidden",
  );
  assert.equal(
    (await call(h, h.pm.credential, "integrate", ["confirm", integrationId]))
      .ok,
    false,
  );
  assert.equal(h.core.integration(integrationId)!.state, "merged");

  // The user merges; the architect still cannot confirm, the PM can.
  t.stub.inHead = true;
  assert.equal(
    codeOf(
      await call(h, t.architect.credential, "integrate", [
        "confirm",
        integrationId,
      ]),
    ),
    "forbidden",
  );
  assert.equal(h.core.integration(integrationId)!.state, "merged");
  resultOf(
    await call(h, h.pm.credential, "integrate", ["confirm", integrationId]),
  );
  assert.equal(h.core.integration(integrationId)!.state, "confirmed");
}

test("small tier: no plan and no architect, the PM reviews, integrates and confirms, and its prompt still says to merge itself", async () => {
  await withTeam(async (t) => {
    const { h } = t;
    const reportId = reportBy(h, t.devA, "b".repeat(40));
    assert.ok(
      bodies(h, h.pm.agentId).some((b) => b.startsWith("Verified report")),
    );
    await passReview(t, h.pm, reportId);
    const { integrationId } = resultOf<{ integrationId: string }>(
      await call(h, h.pm.credential, "integrate", [reportId]),
    );
    await passReview(t, h.pm, integrationId);
    t.stub.inHead = true;
    resultOf(
      await call(h, h.pm.credential, "integrate", ["confirm", integrationId]),
    );
    assert.equal(h.core.integration(integrationId)!.state, "confirmed");

    assert.deepEqual(h.core.listPlans(h.owner), []);
    assert.deepEqual(h.core.statusSnapshot().plans, []);
    assert.deepEqual(bodies(h, t.architect.agentId), []);
    assert.equal(
      bodies(h, h.pm.agentId).filter((b) => b.startsWith("Plan ")).length,
      0,
    );
    const prompt = buildRolePrompt({
      roleName: "pm",
      kind: "PM",
      agentId: h.pm.agentId,
      waitTimeoutSeconds: 90,
      rolePrompt: null,
      architect: {
        ...architectConfig,
        highRiskTriggers: [...architectConfig.highRiskTriggers],
      },
    });
    for (const needle of [
      "small (a single file",
      "you merge the integration branch into the project's HEAD yourself",
    ])
      assert.ok(prompt.includes(needle), needle);
  });
});

test("normal tier: plan, assign, report, review, integrate, integration review, sign-off, user merge, PM confirm", async () => {
  await withTeam(async (t) => {
    const { h } = t;
    const planId = await openAndSubmit(t, "normal", "wp1", "wp2");
    assert.equal(t.reviewers.length, 0, "a normal plan has no review");
    const { integrationId, reportIds } = await integratedPlan(t, planId);

    // Reports and reviews went to the architect, not the PM.
    const architectBodies = bodies(h, t.architect.agentId);
    assert.equal(
      architectBodies.filter((b) => b.startsWith("Verified report")).length,
      2,
    );
    assert.ok(
      architectBodies.some((b) => b.includes(`Work package: ${planId}/wp1`)),
    );
    assert.equal(
      bodies(h, h.pm.agentId).filter((b) => b.startsWith("Verified report"))
        .length,
      0,
    );
    assert.deepEqual(
      h.core.planRecord(h.owner, planId)!.packages.map((p) => p.progress),
      ["integrated", "integrated"],
    );
    assert.equal(reportIds.length, 2);

    await signoffAndConfirm(t, planId, integrationId);

    const status = h.core.statusSnapshot().plans;
    assert.equal(status.length, 1);
    assert.equal(status[0]!.planId, planId);
    assert.equal(status[0]!.state, "approved");
    assert.deepEqual(status[0]!.packages, { integrated: 2 });
    assert.deepEqual(
      status[0]!.signoffs.map((s) => s.integrationId),
      [integrationId],
    );
  });
});

test("high-risk tier: one findings round on the plan review, then the same flow", async () => {
  await withTeam(async (t) => {
    const { h } = t;
    const { planId } = resultOf<{ planId: string }>(
      await call(h, h.pm.credential, "plan", ["open", "high-risk", "risky"]),
    );
    resultOf(
      await call(h, t.architect.credential, "plan", [
        "submit",
        planId,
        planBody("wp1", "wp2"),
      ]),
    );
    assert.equal(h.core.listPlans(h.owner)[0]!.state, "in_review");
    await verdict(t, t.reviewers[0]!, "findings");
    assert.equal(h.core.listPlans(h.owner)[0]!.state, "draft");
    assert.ok(
      bodies(h, t.architect.agentId).some(
        (b) => b.startsWith("Review ") && b.includes("FINDINGS"),
      ),
    );
    resultOf(
      await call(h, t.architect.credential, "plan", [
        "submit",
        planId,
        planBody("wp1", "wp2"),
      ]),
    );
    await verdict(t, t.reviewers[1]!, "pass");
    assert.equal(h.core.listPlans(h.owner)[0]!.state, "approved");
    assert.equal(h.core.listPlans(h.owner)[0]!.approvedRevision, 2);
    assert.equal(
      bodies(h, h.pm.agentId).filter((b) =>
        b.startsWith(`Plan ${planId} approved`),
      ).length,
      1,
    );

    const { integrationId } = await integratedPlan(t, planId);
    await signoffAndConfirm(t, planId, integrationId);
  });
});

test("plan show carries the timestamps that measure each stage in order", async () => {
  await withTeam(async (t) => {
    const { h } = t;
    const planId = await openAndSubmit(t, "normal", "wp1", "wp2");
    const { integrationId } = await integratedPlan(t, planId);
    resultOf(
      await call(h, t.architect.credential, "plan", [
        "signoff",
        planId,
        integrationId,
        "done",
      ]),
    );
    const shown = resultOf<{
      plan: { createdAt: string; updatedAt: string };
      revision: { createdAt: string };
      packages: { assignedAt: string }[];
      signoffs: { createdAt: string }[];
    }>(await call(h, h.pm.credential, "plan", ["show", planId]));
    const stamps = [
      shown.plan.createdAt,
      shown.revision.createdAt,
      shown.packages[0]!.assignedAt,
      shown.signoffs[0]!.createdAt,
    ].map((text) => {
      const time = Date.parse(text);
      assert.ok(Number.isFinite(time), text);
      return time;
    });
    assert.deepEqual(
      stamps,
      [...stamps].sort((a, b) => a - b),
    );
  });
});

test("sign-off is refused for a non-architect, an unmerged or unreviewed integration, a foreign report, and a repeat", async () => {
  await withTeam(async (t) => {
    const { h } = t;
    const planId = await openAndSubmit(t, "normal", "wp1", "wp2");
    await assign(t, planId, "wp1", t.devA);
    await assign(t, planId, "wp2", t.devB);
    const reportA = reportBy(h, t.devA, "b".repeat(40));
    const reportB = reportBy(h, t.devB, "c".repeat(40));
    for (const reportId of [reportA, reportB])
      await passReview(t, t.architect, reportId);
    const signoff = (who: Member, integrationId: string): Promise<string> =>
      call(h, who.credential, "plan", [
        "signoff",
        planId,
        integrationId,
        "ok",
      ]).then(codeOf);

    const discarded = resultOf<{ integrationId: string }>(
      await call(h, t.architect.credential, "integrate", [reportA]),
    ).integrationId;
    resultOf(
      await call(h, t.architect.credential, "integrate", [
        "discard",
        discarded,
      ]),
    );
    assert.equal(
      await signoff(t.architect, discarded),
      "rejected",
      "not merged",
    );

    const first = resultOf<{ integrationId: string }>(
      await call(h, t.architect.credential, "integrate", [reportA, reportB]),
    ).integrationId;
    assert.equal(
      await signoff(t.architect, first),
      "rejected",
      "no review yet",
    );
    resultOf(await call(h, t.architect.credential, "request-review", [first]));
    await verdict(t, t.reviewers.at(-1)!, "findings");
    assert.equal(await signoff(t.architect, first), "rejected", "findings");
    await passReview(t, t.architect, first);
    for (const who of [t.devA, t.devB])
      assert.equal(await signoff(who, first), "forbidden");
    assert.equal(
      codeOf(
        await call(h, h.pm.credential, "plan", [
          "signoff",
          planId,
          first,
          "ok",
        ]),
      ),
      "forbidden",
    );
    assert.equal(await signoff(t.architect, "nope"), "rejected");
    assert.equal(await signoff(t.architect, first), "ok");
    assert.equal(await signoff(t.architect, first), "rejected", "a repeat");

    for (const args of [
      [],
      [planId],
      [planId, first],
      [planId, first, "ok", "x"],
    ])
      assert.equal(
        codeOf(
          await call(h, t.architect.credential, "plan", ["signoff", ...args]),
        ),
        "invalid_request",
      );
  });
});

test("sign-off is refused for an integration that holds a report outside the plan", async () => {
  await withTeam(async (t) => {
    const { h } = t;
    const planId = await openAndSubmit(t, "normal", "wp1");
    await assign(t, planId, "wp1", t.devA);
    const inside = reportBy(h, t.devA, "b".repeat(40));
    const outside = reportBy(h, t.devB, "c".repeat(40));
    for (const reportId of [inside, outside])
      await passReview(t, t.architect, reportId);
    const { integrationId } = resultOf<{ integrationId: string }>(
      await call(h, t.architect.credential, "integrate", [inside, outside]),
    );
    await passReview(t, t.architect, integrationId);
    const refused = await call(h, t.architect.credential, "plan", [
      "signoff",
      planId,
      integrationId,
      "ok",
    ]);
    assert.equal(refused.ok, false);
    assert.match(
      (refused as { message: string }).message,
      new RegExp(`report ${outside} .* is not a package of plan ${planId}`),
    );
    assert.deepEqual(h.core.planRecord(h.owner, planId)!.signoffs, []);
  });
});

test("a restarted PM's summary lists open plans and merged integrations until they are confirmed", async () => {
  await withTeam(async (t) => {
    const { h } = t;
    const planId = await openAndSubmit(t, "normal", "wp1", "wp2");
    const { integrationId } = await integratedPlan(t, planId);
    resultOf(
      await call(h, t.architect.credential, "plan", [
        "signoff",
        planId,
        integrationId,
        "done",
      ]),
    );
    const restarted = h.core.restartAgentGeneration(
      ctx(h.core, h.owner),
      h.pm.agentId,
    );
    assert.deepEqual(restarted.summary.plans, [
      {
        planId,
        title: "a plan",
        tier: "normal",
        state: "approved",
        packages: 2,
        signedOff: [integrationId],
      },
    ]);
    assert.deepEqual(restarted.summary.integrations, [
      {
        integrationId,
        branch: h.core.integration(integrationId).branch,
        headSha: HEAD,
      },
    ]);
    const prompt = buildRolePrompt({
      roleName: "pm",
      kind: "PM",
      agentId: h.pm.agentId,
      waitTimeoutSeconds: 90,
      rolePrompt: null,
      restartSummary: restarted.summary,
    });
    assert.ok(prompt.includes(`- ${planId} [approved, normal]`));
    assert.ok(
      prompt.includes(
        `- ${integrationId} on branch ${h.core.integration(integrationId).branch} at ${HEAD}`,
      ),
    );

    t.stub.inHead = true;
    resultOf(await call(h, h.owner, "integrate", ["confirm", integrationId]));
    const later = h.core.restartAgentGeneration(
      ctx(h.core, h.owner),
      h.pm.agentId,
    );
    assert.deepEqual(later.summary.plans, []);
    assert.deepEqual(later.summary.integrations, []);
  });
});

test("a sign-off made while no PM is active is relayed to the next PM once, and never for a cancelled plan", async () => {
  await withTeam(async (t) => {
    const { h } = t;
    const planId = await openAndSubmit(t, "normal", "wp1", "wp2");
    const { integrationId } = await integratedPlan(t, planId);
    h.core.endAgent(ctx(h.core, h.owner), h.pm.agentId);
    resultOf(
      await call(h, t.architect.credential, "plan", [
        "signoff",
        planId,
        integrationId,
        "done",
      ]),
    );
    const missing = {
      planId,
      kind: "signed_off",
      integrationId,
    } as const;
    assert.ok(
      h.core
        .unannouncedPlanNotices(h.owner)
        .some(
          (n) => n.kind === "signed_off" && n.integrationId === integrationId,
        ),
    );
    assert.equal(
      h.core.announcePlanNotice(ctx(h.core, h.owner), missing).announced,
      false,
    );
    const pm2 = h.addMember("pm2", "PM");
    assert.equal(
      h.core.announcePlanNotice(ctx(h.core, h.owner), missing).announced,
      true,
    );
    const notices = bodies(h, pm2.agentId).filter((b) =>
      b.startsWith(`Plan ${planId} signed off. Integration ${integrationId} `),
    );
    assert.equal(notices.length, 1);
    assert.ok(notices[0]!.includes(h.core.integration(integrationId).branch));
    assert.match(
      h.core.integration(integrationId).branch,
      /^integration\/plan-1-/,
    );
    assert.deepEqual(
      h.core
        .unannouncedPlanNotices(h.owner)
        .filter((n) => n.kind === "signed_off"),
      [],
    );
  });
});

test("a sign-off notice for a plan cancelled while no PM was active is never relayed", async () => {
  await withTeam(async (t) => {
    const { h } = t;
    const planId = await openAndSubmit(t, "normal", "wp1", "wp2");
    const { integrationId } = await integratedPlan(t, planId);
    h.core.endAgent(ctx(h.core, h.owner), h.pm.agentId);
    resultOf(
      await call(h, t.architect.credential, "plan", [
        "signoff",
        planId,
        integrationId,
        "done",
      ]),
    );
    h.core.cancelPlan(ctx(h.core, h.owner), { planId });
    h.addMember("pm2", "PM");
    assert.deepEqual(
      h.core
        .unannouncedPlanNotices(h.owner)
        .filter((n) => n.kind === "signed_off"),
      [],
    );
    assert.equal(
      h.core.announcePlanNotice(ctx(h.core, h.owner), {
        planId,
        kind: "signed_off",
        integrationId,
      }).announced,
      false,
    );
  });
});
