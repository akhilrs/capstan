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
  /** What the stubbed launcher answers when a branch is to be renamed after a task. */
  rename?: (
    agentId: string,
    task: string,
  ) => { branch: string | null; renamed: boolean; note?: string };
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
        renameBranchForTask: async (agentId: string, task: string) =>
          stub.rename?.(agentId, task) ?? { branch: null, renamed: false },
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

test("plan assign sends exactly one message and binds the package in the same step, and a refusal does neither", async () => {
  await withTeam(async (t) => {
    const { h } = t;
    const planId = await approvedPlan(t, "wp1", "wp2");
    const answer = await call(h, h.pm.credential, "plan", [
      "assign",
      planId,
      "wp1",
      t.devA.agentId,
    ]);
    assert.ok(answer.ok, JSON.stringify(answer));
    const { messageId } = (answer as { result: { messageId: string } }).result;
    const sent = h.core.message(messageId)!;
    assert.equal(sent.recipientAgentId, t.devA.agentId);
    assert.equal(sent.state, "queued");
    assert.ok(sent.body.startsWith(`Work package ${planId}/wp1\n`), sent.body);
    assert.ok(sent.body.includes(t.architect.agentId));
    assert.ok(sent.body.includes(JSON.stringify("wp1 works")));
    assert.ok(sent.body.includes(JSON.stringify("src/wp1")));
    assert.deepEqual(bodiesFor(h, t.devA.agentId), [sent.body]);
    const wp1 = h.core
      .planRecord(h.owner, planId)!
      .packages.find((p) => p.packageId === "wp1")!;
    assert.equal(wp1.assigneeAgentId, t.devA.agentId);
    assert.equal(wp1.assignmentMessageId, messageId);

    for (const args of [
      [planId, "wp1", t.devB.agentId],
      [planId, "nope", t.devB.agentId],
      [planId, "wp2", t.architect.agentId],
      [planId, "wp2", t.devA.agentId],
      ["plan-9", "wp1", t.devB.agentId],
    ])
      assert.equal(
        codeOf(await call(h, h.pm.credential, "plan", ["assign", ...args])),
        "rejected",
        JSON.stringify(args),
      );
    assert.deepEqual(bodiesFor(h, t.devB.agentId), []);
    assert.equal(bodiesFor(h, t.devA.agentId).length, 1);
    assert.equal(
      h.core
        .planRecord(h.owner, planId)!
        .packages.find((p) => p.packageId === "wp2")!.assigneeAgentId,
      null,
    );
  });
});

test("plan assign is for the PM and the operator and checks its arguments", async () => {
  await withTeam(async (t) => {
    const { h } = t;
    const planId = await approvedPlan(t, "wp1");
    for (const credential of [
      t.architect.credential,
      t.devA.credential,
      h.seatOnly,
    ])
      assert.equal(
        codeOf(
          await call(h, credential, "plan", [
            "assign",
            planId,
            "wp1",
            t.devA.agentId,
          ]),
        ),
        "forbidden",
      );
    for (const args of [
      [],
      [planId],
      [planId, "wp1"],
      [planId, "wp1", t.devA.agentId, "extra"],
      [planId, "wp1", "bad id"],
    ])
      assert.equal(
        codeOf(await call(h, h.pm.credential, "plan", ["assign", ...args])),
        "invalid_request",
        JSON.stringify(args),
      );
    assert.equal(
      codeOf(
        await call(h, h.owner, "plan", [
          "assign",
          planId,
          "wp1",
          t.devA.agentId,
        ]),
      ),
      "ok",
    );
    assert.deepEqual(h.core.messagesFor(t.devB.agentId), []);
  });
});

test("a report from an assignee goes to the architect with the package line, and a report from anyone else goes to the PM", async () => {
  await withTeam(async (t) => {
    const { h } = t;
    const planId = await approvedPlan(t, "wp1");
    await call(h, h.pm.credential, "plan", [
      "assign",
      planId,
      "wp1",
      t.devA.agentId,
    ]);
    const mine = reportBy(h, t.devA, "b".repeat(40));
    const architectNotices = bodiesFor(h, t.architect.agentId);
    assert.equal(architectNotices.length, 1);
    assert.ok(architectNotices[0]!.startsWith(`Verified report ${mine}\n`));
    assert.ok(architectNotices[0]!.endsWith(`Work package: ${planId}/wp1`));
    assert.deepEqual(bodiesFor(h, h.pm.agentId), []);

    const other = reportBy(h, t.devB, "c".repeat(40));
    assert.deepEqual(bodiesFor(h, t.architect.agentId), architectNotices);
    const pmNotices = bodiesFor(h, h.pm.agentId);
    assert.equal(pmNotices.length, 1);
    assert.ok(pmNotices[0]!.startsWith(`Verified report ${other}\n`));
    assert.ok(!pmNotices[0]!.includes("Work package:"));
  });
});

test("with the architect inactive the report notice falls back to the PM", async () => {
  await withTeam(async (t) => {
    const { h } = t;
    const planId = await approvedPlan(t, "wp1");
    await call(h, h.pm.credential, "plan", [
      "assign",
      planId,
      "wp1",
      t.devA.agentId,
    ]);
    h.core.endAgent(ctx(h.core, h.owner), t.architect.agentId);
    const reportId = reportBy(h, t.devA, "b".repeat(40));
    const pmNotices = bodiesFor(h, h.pm.agentId);
    assert.equal(pmNotices.length, 1);
    assert.ok(pmNotices[0]!.startsWith(`Verified report ${reportId}\n`));
    assert.deepEqual(bodiesFor(h, t.architect.agentId), []);
  });
});

test("the architect requests reviews and integrates, the verdict reaches the requester, and only the PM or the operator confirms", async () => {
  await withTeam(async (t) => {
    const { h } = t;
    const planId = await approvedPlan(t, "wp1");
    await call(h, h.pm.credential, "plan", [
      "assign",
      planId,
      "wp1",
      t.devA.agentId,
    ]);
    const reportId = reportBy(h, t.devA, "b".repeat(40));

    for (const credential of [t.devA.credential, t.devB.credential])
      for (const [command, args] of [
        ["request-review", [reportId]],
        ["integrate", [reportId]],
        ["integrate", ["discard", "x"]],
        ["integrate", ["confirm", "x"]],
      ] as const)
        assert.equal(
          codeOf(await call(h, credential, command, [...args])),
          "forbidden",
          `${command} ${args.join(" ")}`,
        );

    const before = bodiesFor(h, t.architect.agentId).length;
    await passReview(t, t.architect, reportId);
    const verdicts = bodiesFor(h, t.architect.agentId).slice(before);
    assert.equal(verdicts.length, 1);
    assert.ok(verdicts[0]!.startsWith("Review "), verdicts[0]);
    assert.ok(verdicts[0]!.includes("PASS"));
    assert.deepEqual(
      bodiesFor(h, h.pm.agentId).filter((b) => b.startsWith("Review ")),
      [],
      "the PM did not request this review",
    );

    const integrated = await call(h, t.architect.credential, "integrate", [
      reportId,
    ]);
    assert.ok(integrated.ok, JSON.stringify(integrated));
    const { integrationId, state } = (
      integrated as { result: { integrationId: string; state: string } }
    ).result;
    assert.equal(state, "merged");

    await passReview(t, t.architect, integrationId);
    t.stub.inHead = true;
    const refused = await call(h, t.architect.credential, "integrate", [
      "confirm",
      integrationId,
    ]);
    assert.equal(codeOf(refused), "forbidden");
    assert.equal(h.core.integration(integrationId)!.state, "merged");
    const confirmed = await call(h, h.pm.credential, "integrate", [
      "confirm",
      integrationId,
    ]);
    assert.ok(confirmed.ok, JSON.stringify(confirmed));
    assert.equal(h.core.integration(integrationId)!.state, "confirmed");
  });
});

test("the architect may discard an integration, and a review the PM requested is announced to the PM", async () => {
  await withTeam(async (t) => {
    const { h } = t;
    const reportId = reportBy(h, t.devA, "b".repeat(40));
    const before = bodiesFor(h, h.pm.agentId).length;
    await passReview(t, h.pm, reportId);
    const notices = bodiesFor(h, h.pm.agentId).slice(before);
    assert.equal(notices.length, 1);
    assert.ok(notices[0]!.startsWith("Review "));
    assert.deepEqual(bodiesFor(h, t.architect.agentId), []);
    const integrated = await call(h, h.pm.credential, "integrate", [reportId]);
    const { integrationId } = (
      integrated as { result: { integrationId: string } }
    ).result;
    const discarded = await call(h, t.architect.credential, "integrate", [
      "discard",
      integrationId,
    ]);
    assert.ok(discarded.ok, JSON.stringify(discarded));
  });
});

test("a review the architect requested falls back to the PM once the architect is inactive", async () => {
  await withTeam(async (t) => {
    const { h } = t;
    const planId = await approvedPlan(t, "wp1");
    await call(h, h.pm.credential, "plan", [
      "assign",
      planId,
      "wp1",
      t.devA.agentId,
    ]);
    const reportId = reportBy(h, t.devA, "b".repeat(40));
    const requested = await call(h, t.architect.credential, "request-review", [
      reportId,
    ]);
    assert.ok(requested.ok, JSON.stringify(requested));
    h.core.endAgent(ctx(h.core, h.owner), t.architect.agentId);
    const before = bodiesFor(h, h.pm.agentId).length;
    const verdict = await call(h, t.reviewers.at(-1)!.credential, "review", [
      "findings",
      "a bug",
    ]);
    assert.ok(verdict.ok, JSON.stringify(verdict));
    const notices = bodiesFor(h, h.pm.agentId).slice(before);
    assert.equal(notices.length, 1);
    assert.ok(notices[0]!.startsWith("Review "));
  });
});

test("send: a developer may write to the architect, the architect to an active developer, and nothing else opens up", async () => {
  await withTeam(async (t) => {
    const { h } = t;
    const send = (from: Member | string, to: string) =>
      call(h, typeof from === "string" ? from : from.credential, "send", [
        to,
        "question",
      ]);
    assert.equal(codeOf(await send(t.devA, t.architect.agentId)), "ok");
    assert.equal(codeOf(await send(t.architect, t.devA.agentId)), "ok");
    assert.equal(codeOf(await send(t.architect, "@pm")), "ok");
    assert.equal(codeOf(await send(t.devA, "@pm")), "ok");
    assert.equal(
      codeOf(await send(t.devA, t.devB.agentId)),
      "recipient_not_allowed",
    );
    assert.equal(
      codeOf(await send(t.architect, t.architect.agentId)),
      "self_send",
    );
    const supervisor = h.addMember("supervisor", "Supervisor");
    assert.equal(
      codeOf(await send(t.architect, supervisor.agentId)),
      "recipient_not_allowed",
    );
    assert.equal(
      codeOf(await send(supervisor, t.architect.agentId)),
      "recipient_not_allowed",
    );
    const reviewer = member(h, "reviewer-x", "Verifier", "reviewer");
    assert.equal(
      codeOf(await send(reviewer, t.architect.agentId)),
      "recipient_not_allowed",
    );
    assert.equal(
      codeOf(await send(t.architect, reviewer.agentId)),
      "recipient_not_allowed",
    );
    h.core.endAgent(ctx(h.core, h.owner), t.devB.agentId);
    assert.equal(
      codeOf(await send(t.architect, t.devB.agentId)),
      "unknown_recipient",
    );
    h.core.endAgent(ctx(h.core, h.owner), t.architect.agentId);
    assert.equal(
      codeOf(await send(t.devA, t.architect.agentId)),
      "unknown_recipient",
    );
  });
});

test("with [architect] disabled a developer still writes only to the PM and the architect role gets no extra powers", async () => {
  const stub: Stub = { inHead: false };
  const h = await harness({
    commands: {
      config: {
        ...config,
        architect: { ...config.architect, enabled: false },
      } as CapstanConfig,
      integrationGit: {
        headCommit: async () => BASE,
        commitExists: async () => true,
        merge: async () => ({ kind: "merged", headSha: HEAD }),
        branchTip: async () => null,
        isInHead: async () => stub.inHead,
        deleteBranch: async () => true,
      },
    },
  });
  try {
    h.core.syncRoleDefinitions(ctx(h.core, h.owner), [
      { name: "pm", kind: "PM", host: "claude", configHash: "a".repeat(64) },
      {
        name: "developer",
        kind: "Developer",
        host: "claude",
        configHash: "b".repeat(64),
      },
      {
        name: "architect",
        kind: "Developer",
        host: "claude",
        configHash: "c".repeat(64),
      },
    ]);
    const architect = member(h, "architect", "Developer", "architect");
    const send = (from: Member, to: string) =>
      call(h, from.credential, "send", [to, "question"]);
    assert.equal(
      codeOf(await send(h.developer, architect.agentId)),
      "recipient_not_allowed",
    );
    assert.equal(
      codeOf(await send(architect, h.developer.agentId)),
      "recipient_not_allowed",
    );
    assert.equal(
      codeOf(await call(h, architect.credential, "integrate", ["x"])),
      "forbidden",
    );
    assert.equal(
      codeOf(await call(h, architect.credential, "request-review", ["x"])),
      "forbidden",
    );
  } finally {
    await close(h);
  }
});

test("replace rebinds the packages of the predecessor in the ledger", async () => {
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
    const successor = member(h, "dev-c", "Developer", "developer");
    h.core.endAgent(ctx(h.core, h.owner), t.devA.agentId);
    h.core.recordAgentReplaced(ctx(h.core, h.owner), {
      predecessorId: t.devA.agentId,
      successorId: successor.agentId,
    });
    const packages = h.core.planRecord(h.owner, planId)!.packages;
    assert.equal(
      packages.find((p) => p.packageId === "wp1")!.assigneeAgentId,
      successor.agentId,
    );
    assert.equal(
      packages.find((p) => p.packageId === "wp2")!.assigneeAgentId,
      t.devB.agentId,
    );
    const seed = h.core.agentSeed(t.devA.agentId);
    assert.deepEqual(
      seed.packages.map((p) => `${p.planId}/${p.packageId}`),
      [],
      "the predecessor holds nothing after the rebind",
    );
    assert.deepEqual(
      h.core.agentSeed(successor.agentId).packages.map((p) => p.packageId),
      ["wp1"],
    );
  });
});

test("plan assign names the assignee's branch after the package and prints it, or says the branch was kept", async () => {
  await withTeam(async (t) => {
    const { h } = t;
    const planId = await approvedPlan(t, "wp1", "wp2");
    const asked: string[] = [];
    t.stub.rename = (agentId, task) => {
      asked.push(`${agentId}:${task}`);
      return agentId === t.devA.agentId
        ? { branch: "feat/x-wp1", renamed: true }
        : {
            branch: "chore/dev-b-developer",
            renamed: false,
            note: "branch kept: chore/dev-b-developer (it already has commits)",
          };
    };
    const first = await call(h, h.pm.credential, "plan", [
      "assign",
      planId,
      "wp1",
      t.devA.agentId,
    ]);
    assert.ok(first.ok, JSON.stringify(first));
    const a = (first as { result: Record<string, unknown> }).result;
    assert.equal(a.branch, "feat/x-wp1");
    assert.equal(a.branchNote, undefined);
    const second = await call(h, h.pm.credential, "plan", [
      "assign",
      planId,
      "wp2",
      t.devB.agentId,
    ]);
    assert.ok(second.ok, JSON.stringify(second));
    const b = (second as { result: Record<string, unknown> }).result;
    assert.equal(b.branch, "chore/dev-b-developer");
    assert.match(String(b.branchNote), /^branch kept: chore\/dev-b-developer/);
    assert.deepEqual(asked, [
      `${t.devA.agentId}:${planId}/wp1`,
      `${t.devB.agentId}:${planId}/wp2`,
    ]);
  });
});
