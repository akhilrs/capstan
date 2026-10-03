import assert from "node:assert/strict";
import { test } from "node:test";
import { openSqlite } from "../src/controller/sqlite.js";
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
    packages: ids.map((id, index) => ({
      ...(index === 0 ? {} : { depends_on: [ids[index - 1]] }),
      id,
      title: `package ${id}`,
      owns: [`src/${id}/`],
      interfaces: [`exports ${id}()`],
      acceptance: [`${id} works`],
      estimate_hours: 2,
    })),
  });
}

function bodiesFor(h: Harness, agentId: string): string[] {
  return h.core
    .messagesFor(agentId)
    .map((m) => m.body)
    .filter((body) => !/^Plan \S+ approved/.test(body));
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
  const reviewer = t.reviewers.at(-1)!;
  const verdict = await call(t.h, reviewer.credential, "review", [
    "pass",
    "looks right",
  ]);
  assert.ok(verdict.ok, JSON.stringify(verdict));
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

type Answer = Awaited<ReturnType<typeof call>>;

function assign(
  t: Team,
  planId: string,
  pkg: string,
  who: Member,
  ...extra: string[]
): Promise<Answer> {
  return call(t.h, t.h.pm.credential, "plan", [
    "assign",
    planId,
    pkg,
    who.agentId,
    ...extra,
  ]);
}

let current: Team | undefined;
function t_messageBody(answer: Answer): string {
  const { messageId } = (answer as { result: { messageId: string } }).result;
  return current!.h.core.message(messageId)!.body;
}

async function finishAndReview(
  t: Team,
  who: Member,
  commit: string,
): Promise<string> {
  const reportId = reportBy(t.h, who, commit);
  await passReview(t, t.h.pm, reportId);
  return reportId;
}

function progressOf(t: Team, planId: string, pkg: string): string {
  return t.h.core
    .planRecord(t.h.owner, planId)!
    .packages.find((p) => p.packageId === pkg)!.progress;
}

function events(t: Team): string[] {
  const db = openSqlite(`${t.h.stateDirectory}/controller.sqlite`, {
    readOnly: true,
  });
  try {
    return (
      db
        .prepare(
          "SELECT payload_json FROM controller_events WHERE entity_type = 'plan' ORDER BY sequence",
        )
        .all() as {
        payload_json: string;
      }[]
    ).map((r) => r.payload_json);
  } finally {
    db.close();
  }
}

test("a package whose dependency is not reviewed is refused, naming every unmet package, and nothing changes", async () => {
  await withTeam(async (t) => {
    current = t;
    const { h } = t;
    const planId = await approvedPlan(t, "wp1", "wp2");
    const refused = await assign(t, planId, "wp2", t.devB);
    assert.equal(refused.ok, false);
    assert.equal((refused as { code: string }).code, "rejected");
    const text = (refused as { message: string }).message;
    assert.match(
      text,
      /dependencies_unmet: package wp2 depends on wp1 \(unassigned\)/,
    );
    assert.deepEqual(bodiesFor(h, t.devB.agentId), []);
    assert.equal(progressOf(t, planId, "wp2"), "unassigned");

    assert.ok((await assign(t, planId, "wp1", t.devA)).ok);
    for (const stage of ["assigned", "reported", "findings"]) {
      if (stage === "reported") reportBy(h, t.devA, "1".repeat(40));
      if (stage === "findings") {
        const requested = await call(h, h.pm.credential, "request-review", [
          latestReport(t),
        ]);
        assert.ok(requested.ok, JSON.stringify(requested));
        const verdict = await call(
          h,
          t.reviewers.at(-1)!.credential,
          "review",
          ["findings", "fix it"],
        );
        assert.ok(verdict.ok, JSON.stringify(verdict));
      }
      const again = await assign(t, planId, "wp2", t.devB);
      assert.equal(again.ok, false, stage);
      assert.match(
        (again as { message: string }).message,
        new RegExp(`wp1 \\(${stage}\\)`),
      );
      assert.equal(progressOf(t, planId, "wp1"), stage);
    }
    assert.deepEqual(bodiesFor(h, t.devB.agentId), []);
    assert.equal(progressOf(t, planId, "wp2"), "unassigned");
  });
});

function latestReport(t: Team): string {
  const db = openSqlite(`${t.h.stateDirectory}/controller.sqlite`, {
    readOnly: true,
  });
  try {
    return (
      db
        .prepare(
          "SELECT report_id FROM agent_reports ORDER BY sequence DESC LIMIT 1",
        )
        .get() as { report_id: string }
    ).report_id;
  } finally {
    db.close();
  }
}

test("assign succeeds without --early once the dependency is reviewed", async () => {
  await withTeam(async (t) => {
    current = t;
    const planId = await approvedPlan(t, "wp1", "wp2");
    assert.ok((await assign(t, planId, "wp1", t.devA)).ok);
    await finishAndReview(t, t.devA, "1".repeat(40));
    assert.equal(progressOf(t, planId, "wp1"), "reviewed");
    const answer = await assign(t, planId, "wp2", t.devB);
    assert.ok(answer.ok, JSON.stringify(answer));
    const result = (answer as { result: { early: boolean; unmet: string[] } })
      .result;
    assert.equal(result.early, false);
    assert.deepEqual(result.unmet, []);
    assert.ok(!t_messageBody(answer).includes("not yet reviewed"));
  });
});

test("--early assigns past unmet dependencies, records the reason and tells the developer", async () => {
  await withTeam(async (t) => {
    current = t;
    const planId = await approvedPlan(t, "wp1", "wp2");
    const answer = await assign(
      t,
      planId,
      "wp2",
      t.devB,
      "--early",
      "api is stable",
    );
    assert.ok(answer.ok, JSON.stringify(answer));
    const result = (answer as { result: { early: boolean; unmet: string[] } })
      .result;
    assert.equal(result.early, true);
    assert.deepEqual(result.unmet, ["wp1 (unassigned)"]);
    const body = t_messageBody(answer);
    assert.match(body, /dependencies wp1 \(unassigned\) are not yet reviewed/);
    assert.match(body, /Build against the interfaces the plan states/);
    assert.match(body, /cannot be integrated before them/);
    assert.ok(
      events(t).some(
        (p) => p.includes("api is stable") && p.includes("wp1 (unassigned)"),
      ),
      "the ledger event carries the reason and the unmet list",
    );
  });
});

test("--early needs a non-empty reason", async () => {
  await withTeam(async (t) => {
    current = t;
    const planId = await approvedPlan(t, "wp1", "wp2");
    for (const extra of [
      ["--early"],
      ["--early", ""],
      ["--early", "   "],
      ["--early", "x".repeat(501)],
      ["--late", "x"],
    ]) {
      const answer = await assign(t, planId, "wp2", t.devB, ...extra);
      assert.equal(answer.ok, false, JSON.stringify(extra));
      assert.equal(
        (answer as { code: string }).code,
        "invalid_request",
        JSON.stringify(extra),
      );
    }
    assert.deepEqual(bodiesFor(t.h, t.devB.agentId), []);
  });
});

test("a cancelled dependency counts as unmet", async () => {
  await withTeam(async (t) => {
    current = t;
    const planId = await approvedPlan(t, "wp1", "wp2");
    t.h.core.cancelPlan(ctx(t.h.core, t.h.owner), {
      planId,
      packageId: "wp1",
    });
    const answer = await assign(t, planId, "wp2", t.devB);
    assert.equal(answer.ok, false);
    assert.match((answer as { message: string }).message, /wp1 \(cancelled\)/);
  });
});

test("integration order: a dependent needs its dependency merged, confirmed or earlier in the list", async () => {
  await withTeam(async (t) => {
    current = t;
    const { h } = t;
    const planId = await approvedPlan(t, "wp1", "wp2");
    assert.ok((await assign(t, planId, "wp1", t.devA)).ok);
    assert.ok(
      (await assign(t, planId, "wp2", t.devB, "--early", "parallel")).ok,
    );
    const first = await finishAndReview(t, t.devA, "1".repeat(40));
    const second = await finishAndReview(t, t.devB, "2".repeat(40));

    const alone = await call(h, t.architect.credential, "integrate", [second]);
    assert.equal(alone.ok, false);
    assert.match(
      (alone as { message: string }).message,
      /integration_order: report \S+ \(package wp2\) needs wp1 integrated first or earlier in this list/,
    );
    const reversed = await call(h, t.architect.credential, "integrate", [
      second,
      first,
    ]);
    assert.equal(reversed.ok, false);
    assert.match(
      (reversed as { message: string }).message,
      /integration_order/,
    );

    const good = await call(h, t.architect.credential, "integrate", [
      first,
      second,
    ]);
    assert.ok(good.ok, JSON.stringify(good));
  });
});

test("integration order: a dependency merged earlier satisfies a later integration, and reports outside plans are unaffected", async () => {
  await withTeam(async (t) => {
    current = t;
    const { h } = t;
    const planId = await approvedPlan(t, "wp1", "wp2");
    assert.ok((await assign(t, planId, "wp1", t.devA)).ok);
    assert.ok(
      (await assign(t, planId, "wp2", t.devB, "--early", "parallel")).ok,
    );
    const first = await finishAndReview(t, t.devA, "1".repeat(40));
    const second = await finishAndReview(t, t.devB, "2".repeat(40));
    assert.ok((await call(h, t.architect.credential, "integrate", [first])).ok);
    assert.ok(
      (await call(h, t.architect.credential, "integrate", [second])).ok,
      "dependency already merged",
    );

    const loose = member(h, "dev-loose", "Developer", "developer");
    const outside = await finishAndReview(t, loose, "3".repeat(40));
    const answer = await call(h, t.architect.credential, "integrate", [
      outside,
    ]);
    assert.ok(answer.ok, JSON.stringify(answer));
  });
});

async function twoReported(
  t: Team,
): Promise<{ planId: string; second: string }> {
  const planId = await approvedPlan(t, "wp1", "wp2");
  assert.ok((await assign(t, planId, "wp1", t.devA)).ok);
  assert.ok((await assign(t, planId, "wp2", t.devB, "--early", "parallel")).ok);
  await finishAndReview(t, t.devA, "1".repeat(40));
  const second = await finishAndReview(t, t.devB, "2".repeat(40));
  const blocked = await call(t.h, t.architect.credential, "integrate", [
    second,
  ]);
  assert.equal(blocked.ok, false, "refused while everything is live");
  return { planId, second };
}

test("integration order skips a cancelled dependency, a cancelled plan and a superseded plan", async () => {
  for (const how of ["package", "plan", "superseded"] as const) {
    await withTeam(async (t) => {
      current = t;
      const { planId, second } = await twoReported(t);
      if (how === "package")
        t.h.core.cancelPlan(ctx(t.h.core, t.h.owner), {
          planId,
          packageId: "wp1",
        });
      else if (how === "plan")
        t.h.core.cancelPlan(ctx(t.h.core, t.h.owner), { planId });
      else {
        const next = t.h.core.openPlan(ctx(t.h.core, t.h.pm.credential), {
          tier: "normal",
          title: "next",
          supersedesPlanId: planId,
        }).planId;
        const submitted = await call(t.h, t.architect.credential, "plan", [
          "submit",
          next,
          planBody("wp9"),
        ]);
        assert.ok(submitted.ok, JSON.stringify(submitted));
        assert.equal(
          t.h.core.planRecord(t.h.owner, planId)!.plan.state,
          "superseded",
        );
      }
      const answer = await call(t.h, t.architect.credential, "integrate", [
        second,
      ]);
      assert.ok(answer.ok, `${how}: ${JSON.stringify(answer)}`);
    });
  }
});

test("integration order refuses a dependent whose dependency has no report yet", async () => {
  for (const dependency of ["unassigned", "assigned"] as const) {
    await withTeam(async (t) => {
      current = t;
      const planId = await approvedPlan(t, "wp1", "wp2");
      if (dependency === "assigned")
        assert.ok((await assign(t, planId, "wp1", t.devA)).ok);
      assert.ok(
        (await assign(t, planId, "wp2", t.devB, "--early", "parallel")).ok,
      );
      const second = await finishAndReview(t, t.devB, "2".repeat(40));
      const answer = await call(t.h, t.architect.credential, "integrate", [
        second,
      ]);
      assert.equal(answer.ok, false, dependency);
      assert.match(
        (answer as { message: string }).message,
        /integration_order: report \S+ \(package wp2\) needs wp1, which has no report yet/,
        dependency,
      );
    });
  }
});
