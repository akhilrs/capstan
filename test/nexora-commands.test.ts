import assert from "node:assert/strict";
import { test } from "node:test";
import type { CapstanConfig } from "../src/config/capstan-config.js";
import type { ReportEvidence } from "../src/controller/core.js";
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

const config = {
  architect: {
    enabled: true,
    role: "architect",
    planReview: "never",
    reviewerRole: null,
    maxPackages: 8,
    countTowardWorkerLimit: false,
    highRiskTriggers: [],
  },
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
        ["developer", "Developer"],
        ["architect", "Developer"],
        ["reviewer", "Verifier"],
        ["supervisor", "Supervisor"],
      ].map(([name, kind], index) => ({
        name: name!,
        kind: kind as "PM" | "Developer" | "Verifier" | "Supervisor",
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

async function approvedPlan(t: Team, ...ids: string[]): Promise<string> {
  const { h } = t;
  const opened = await call(h, h.pm.credential, "plan", [
    "open",
    "normal",
    "a plan",
  ]);
  assert.ok(opened.ok, JSON.stringify(opened));
  const planId = (opened as { result: { planId: string } }).result.planId;
  const submitted = await call(h, t.architect.credential, "plan", [
    "submit",
    planId,
    planBody(...ids),
  ]);
  assert.ok(submitted.ok, JSON.stringify(submitted));
  return planId;
}

function codeOf(response: Awaited<ReturnType<typeof call>>): string {
  return response.ok ? "ok" : response.code;
}

function bodiesFor(h: Harness, agentId: string): string[] {
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

async function passReview(
  t: Team,
  requester: Member,
  subjectId: string,
): Promise<void> {
  const requested = await call(t.h, requester.credential, "request-review", [
    subjectId,
  ]);
  assert.ok(requested.ok, JSON.stringify(requested));
  const verdict = await call(t.h, t.reviewers.at(-1)!.credential, "review", [
    "pass",
    "looks right",
  ]);
  assert.ok(verdict.ok, JSON.stringify(verdict));
}

async function link(
  h: Harness,
  credential: string,
  ...args: string[]
): Promise<Awaited<ReturnType<typeof call>>> {
  return await call(h, credential, "link", args);
}

const reviewedNotices = (h: Harness): string[] =>
  bodiesFor(h, h.pm.agentId).filter((b) => / package \S+ reviewed/.test(b));

test("link and link bind are for the PM and the operator only", async () => {
  await withTeam(async (t) => {
    const { h } = t;
    const planId = await approvedPlan(t, "wp1");
    for (const who of [t.architect, t.devA, t.reviewers[0]])
      if (who !== undefined)
        for (const args of [
          ["plan", planId, "PM-1"],
          ["bind", "req-1", t.devA.agentId],
        ])
          assert.equal(
            codeOf(await link(h, who.credential, ...args)),
            "forbidden",
          );
    const linked = await link(h, h.pm.credential, "plan", planId, "PM-1");
    assert.ok(linked.ok, JSON.stringify(linked));
    assert.equal(
      (linked as { result: { externalId: string; syncedState: string } }).result
        .syncedState,
      "todo",
    );
    assert.ok((await link(h, h.owner, "requirement", "req-1", "PM-2")).ok);
    const bound = await link(
      h,
      h.pm.credential,
      "bind",
      "req-1",
      t.devA.agentId,
    );
    assert.ok(bound.ok, JSON.stringify(bound));
    assert.equal(
      (bound as { result: { boundAgentId: string } }).result.boundAgentId,
      t.devA.agentId,
    );
  });
});

test("link refuses bad input, a different Nexora id and an unknown plan", async () => {
  await withTeam(async (t) => {
    const { h } = t;
    const planId = await approvedPlan(t, "wp1");
    assert.equal(codeOf(await link(h, h.pm.credential)), "invalid_request");
    assert.equal(
      codeOf(await link(h, h.pm.credential, "task", "x", "PM-1")),
      "invalid_request",
    );
    assert.equal(
      codeOf(await link(h, h.pm.credential, "plan", planId, "nope")),
      "invalid_request",
    );
    assert.equal(
      codeOf(await link(h, h.pm.credential, "plan", planId, "PM-1", "bogus")),
      "invalid_request",
    );
    assert.equal(
      codeOf(await link(h, h.pm.credential, "plan", "no-such-plan", "PM-1")),
      "rejected",
    );
    assert.ok((await link(h, h.pm.credential, "plan", planId, "PM-1")).ok);
    const conflict = await link(h, h.pm.credential, "plan", planId, "PM-9");
    assert.equal(codeOf(conflict), "rejected");
    assert.match((conflict as { message: string }).message, /link_conflict/);
    assert.equal(
      codeOf(await link(h, h.pm.credential, "bind", "req-9", t.devA.agentId)),
      "rejected",
    );
  });
});

test("plan cancel is operator only and cancels the plan, notifying the PM and the assignee", async () => {
  await withTeam(async (t) => {
    const { h } = t;
    const planId = await approvedPlan(t, "wp1", "wp2");
    await call(h, h.pm.credential, "plan", [
      "assign",
      planId,
      "wp1",
      t.devA.agentId,
    ]);
    for (const who of [h.pm, t.architect, t.devA])
      assert.equal(
        codeOf(await call(h, who.credential, "plan", ["cancel", planId])),
        "forbidden",
      );
    const answer = await call(h, h.owner, "plan", ["cancel", planId]);
    assert.ok(answer.ok, JSON.stringify(answer));
    assert.deepEqual(
      (answer as { result: { cancelledPackages: string[] } }).result
        .cancelledPackages,
      ["wp1", "wp2"],
    );
    assert.ok(
      bodiesFor(h, h.pm.agentId).some((b) =>
        b.startsWith(`Plan ${planId} cancelled`),
      ),
    );
    assert.ok(
      bodiesFor(h, t.devA.agentId).some((b) =>
        b.startsWith(`Stop work on package wp1 of plan ${planId}`),
      ),
    );
    const again = await call(h, h.owner, "plan", ["cancel", planId]);
    assert.match((again as { message: string }).message, /already_cancelled/);
    assert.equal(
      codeOf(await call(h, h.owner, "plan", ["cancel"])),
      "invalid_request",
    );
  });
});

test("plan cancel of one package leaves the rest of the plan alone", async () => {
  await withTeam(async (t) => {
    const { h } = t;
    const planId = await approvedPlan(t, "wp1", "wp2");
    const answer = await call(h, h.owner, "plan", ["cancel", planId, "wp2"]);
    assert.ok(answer.ok, JSON.stringify(answer));
    const shown = h.core.planRecord(h.owner, planId)!;
    assert.equal(shown.plan.cancelledAt, null);
    assert.equal(
      shown.packages.find((p) => p.packageId === "wp1")!.cancelledAt,
      null,
    );
    assert.notEqual(
      shown.packages.find((p) => p.packageId === "wp2")!.cancelledAt,
      null,
    );
    const assign = await call(h, h.pm.credential, "plan", [
      "assign",
      planId,
      "wp2",
      t.devA.agentId,
    ]);
    assert.match((assign as { message: string }).message, /package_cancelled/);
  });
});

test("replace of a developer whose package was cancelled leaves the cancelled package and the seed without it", async () => {
  await withTeam(async (t) => {
    const { h } = t;
    const planId = await approvedPlan(t, "wp1", "wp2");
    for (const [pkg, who] of [
      ["wp1", t.devA],
      ["wp2", t.devB],
    ] as const)
      await call(h, h.pm.credential, "plan", [
        "assign",
        planId,
        pkg,
        who.agentId,
      ]);
    await call(h, h.owner, "plan", ["cancel", planId, "wp1"]);
    const successor = member(h, "dev-c", "Developer", "developer");
    h.core.endAgent(ctx(h.core, h.owner), t.devA.agentId);
    h.core.recordAgentReplaced(ctx(h.core, h.owner), {
      predecessorId: t.devA.agentId,
      successorId: successor.agentId,
    });
    const wp1 = h.core
      .planRecord(h.owner, planId)!
      .packages.find((p) => p.packageId === "wp1")!;
    assert.equal(wp1.assigneeAgentId, t.devA.agentId);
    assert.deepEqual(h.core.agentSeed(successor.agentId).packages, []);
  });
});

test("plan show prints the Nexora id, synced state, wanted state and drift", async () => {
  await withTeam(async (t) => {
    const { h } = t;
    const planId = await approvedPlan(t, "wp1");
    await link(h, h.pm.credential, "plan", planId, "PM-1");
    await link(h, h.pm.credential, "package", `${planId}/wp1`, "PM-2");
    await call(h, h.pm.credential, "plan", [
      "assign",
      planId,
      "wp1",
      t.devA.agentId,
    ]);
    const shown = (await call(h, h.pm.credential, "plan", [
      "show",
      planId,
    ])) as {
      result: {
        nexora: Record<string, unknown>;
        packages: Record<string, unknown>[];
      };
    };
    assert.deepEqual(shown.result.nexora, {
      externalId: "PM-1",
      syncedState: "todo",
      wanted: "in_progress",
      drift: true,
    });
    assert.equal(shown.result.packages[0]!.externalId, "PM-2");
    assert.equal(shown.result.packages[0]!.syncedState, "todo");
    assert.equal(shown.result.packages[0]!.wanted, "in_progress");
    assert.equal(shown.result.packages[0]!.drift, true);
    await link(
      h,
      h.pm.credential,
      "package",
      `${planId}/wp1`,
      "PM-2",
      "in_progress",
    );
    const list = (await call(h, h.pm.credential, "plan", ["show"])) as {
      result: { plans: Record<string, unknown>[] };
    };
    assert.equal(list.result.plans[0]!.externalId, "PM-1");
    const other = (await call(h, h.pm.credential, "plan", [
      "show",
      await approvedPlan(t, "wp9"),
    ])) as { result: Record<string, unknown> };
    assert.equal("nexora" in other.result, false);
  });
});

test("status carries a Nexora drift section for the PM only", async () => {
  await withTeam(async (t) => {
    const { h } = t;
    const planId = await approvedPlan(t, "wp1");
    await link(h, h.pm.credential, "plan", planId, "PM-1");
    await call(h, h.pm.credential, "plan", [
      "assign",
      planId,
      "wp1",
      t.devA.agentId,
    ]);
    const pm = (await call(h, h.pm.credential, "status", [])) as {
      result: { nexoraDrift?: Record<string, unknown>[] };
    };
    assert.deepEqual(pm.result.nexoraDrift, [
      {
        refKind: "plan",
        refId: planId,
        externalId: "PM-1",
        syncedState: "todo",
        wanted: "in_progress",
      },
    ]);
    for (const credential of [
      h.owner,
      t.devA.credential,
      t.architect.credential,
    ]) {
      const other = (await call(h, credential, "status", [])) as {
        result: Record<string, unknown>;
      };
      assert.equal(other.result.nexoraDrift, undefined);
    }
    await link(h, h.pm.credential, "plan", planId, "PM-1", "in_progress");
    const synced = (await call(h, h.pm.credential, "status", [])) as {
      result: { nexoraDrift: unknown[] };
    };
    assert.deepEqual(synced.result.nexoraDrift, []);
  });
});

test("the PM is told once when a linked package's report passes review, and not without a plan link", async () => {
  await withTeam(async (t) => {
    const { h } = t;
    const planId = await approvedPlan(t, "wp1", "wp2");
    for (const [pkg, who] of [
      ["wp1", t.devA],
      ["wp2", t.devB],
    ] as const)
      await call(h, h.pm.credential, "plan", [
        "assign",
        planId,
        pkg,
        who.agentId,
      ]);
    const unlinked = reportBy(h, t.devA, "b".repeat(40));
    await passReview(t, t.architect, unlinked);
    assert.deepEqual(reviewedNotices(h), [], "no plan link, no notice");

    await link(h, h.pm.credential, "plan", planId, "PM-1");
    const second = reportBy(h, t.devB, "c".repeat(40));
    await passReview(t, t.architect, second);
    const notices = reviewedNotices(h);
    assert.equal(notices.length, 1);
    assert.ok(
      notices[0]!.startsWith(
        `Plan ${planId} package wp2 reviewed\nReport: ${second}`,
      ),
      notices[0],
    );

    await passReview(t, t.architect, second);
    assert.equal(reviewedNotices(h).length, 1, "a repeat review adds nothing");
  });
});

test("a findings verdict and a cancelled package queue no reviewed notice", async () => {
  await withTeam(async (t) => {
    const { h } = t;
    const planId = await approvedPlan(t, "wp1", "wp2");
    await link(h, h.pm.credential, "plan", planId, "PM-1");
    for (const [pkg, who] of [
      ["wp1", t.devA],
      ["wp2", t.devB],
    ] as const)
      await call(h, h.pm.credential, "plan", [
        "assign",
        planId,
        pkg,
        who.agentId,
      ]);
    const first = reportBy(h, t.devA, "b".repeat(40));
    await call(h, t.architect.credential, "request-review", [first]);
    await call(h, t.reviewers.at(-1)!.credential, "review", [
      "findings",
      "needs work",
    ]);
    assert.deepEqual(reviewedNotices(h), []);
    const second = reportBy(h, t.devB, "c".repeat(40));
    await call(h, h.owner, "plan", ["cancel", planId, "wp2"]);
    await passReview(t, t.architect, second);
    assert.deepEqual(reviewedNotices(h), []);
  });
});
