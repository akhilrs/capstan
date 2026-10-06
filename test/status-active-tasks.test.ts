import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { test } from "node:test";
import { openSqlite } from "../src/controller/sqlite.js";
import type { CommandResponse } from "../src/daemon.js";
import type { ReportEvidence } from "../src/controller/core.js";
import {
  MAX_STATUS_TASK_PACKAGES,
  MAX_STATUS_TASK_PLANS,
  MAX_STATUS_TASK_REQUIREMENTS,
} from "../src/commands/shared.js";
import { integrate, type IntegrationGit } from "../src/integration.js";
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
const CAPS = {
  plans: MAX_STATUS_TASK_PLANS,
  packages: MAX_STATUS_TASK_PACKAGES,
  requirements: MAX_STATUS_TASK_REQUIREMENTS,
};

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
  return h.core.recordAgentReport(ctx(h.core, author.credential), {
    commitSha: commit,
    summary: `work of ${author.agentId}`,
    evidence,
  }).record.reportId;
}

let reviewers = 0;

function passReview(h: Harness, subjectId: string): void {
  const reviewer = member(h, `reviewer-${++reviewers}`, "Verifier", "reviewer");
  h.core.beginReview(ctx(h.core, h.pm.credential), {
    subjectId,
    reviewerRole: "reviewer",
    reviewerAgentId: reviewer.agentId,
  });
  h.core.completeReview(ctx(h.core, reviewer.credential), {
    verdict: "pass",
    text: "ok",
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

function body(...packages: [string, string][]): string {
  return JSON.stringify({
    summary: "s",
    packages: packages.map(([id, title]) => ({ id, title })),
  });
}

interface Team {
  readonly h: Harness;
  readonly architect: Member;
  readonly dev1: Member;
  readonly dev2: Member;
  readonly dev3: Member;
}

async function team(): Promise<Team> {
  const h = await harness();
  withRoles(h);
  return {
    h,
    architect: member(h, "architect", "Developer", "developer"),
    dev1: h.developer,
    dev2: member(h, "dev-two", "Developer", "developer"),
    dev3: member(h, "dev-three", "Developer", "developer"),
  };
}

function open(h: Harness, title = "a plan", supersedes?: string): string {
  return h.core.openPlan(ctx(h.core, h.pm.credential), {
    tier: "normal",
    title,
    ...(supersedes === undefined ? {} : { supersedesPlanId: supersedes }),
  }).planId;
}

function submit(t: Team, planId: string, json: string, review = false): void {
  t.h.core.submitPlan(ctx(t.h.core, t.architect.credential), {
    planId,
    bodyJson: json,
    baseSha: BASE,
    review,
  });
}

function assign(t: Team, planId: string, packageId: string, who: Member): void {
  t.h.core.assignPackage(ctx(t.h.core, t.h.pm.credential), {
    planId,
    packageId,
    agentId: who.agentId,
  });
}

function pane(
  h: Harness,
  agentId: string,
  taskRef?: string,
  taskTitle?: string,
): void {
  h.core.recordAgentPane(ctx(h.core, h.owner), {
    agentId,
    workspaceId: null,
    paneId: null,
    worktreePath: null,
    branch: null,
    baseSha: null,
    ...(taskRef === undefined ? {} : { taskRef }),
    ...(taskTitle === undefined ? {} : { taskTitle }),
  });
}

function link(
  h: Harness,
  refKind: "plan" | "package" | "requirement",
  refId: string,
  externalId: string,
): void {
  h.core.linkExternal(ctx(h.core, h.owner), { refKind, refId, externalId });
}

test("activeTasks lists an approved plan's packages with titles, assignees, progress and Nexora ids, and total and done skip a cancelled package", async () => {
  const t = await team();
  try {
    const { h } = t;
    const planId = open(h, "the plan");
    submit(
      t,
      planId,
      body(["wp1", "First"], ["wp2", "Second"], ["wp3", "Third"]),
    );
    link(h, "plan", planId, "NX-1");
    link(h, "package", `${planId}/wp2`, "NX-2");
    assign(t, planId, "wp2", t.dev1);
    passReview(h, reportBy(h, t.dev1, "1".repeat(40)));
    assign(t, planId, "wp3", t.dev2);
    h.core.cancelPlan(ctx(h.core, h.owner), { planId, packageId: "wp1" });

    const tasks = h.core.activeTasks(h.owner, CAPS);
    assert.equal(tasks.truncated, false);
    assert.deepEqual(tasks.plans, [
      {
        planId,
        title: "the plan",
        state: "approved",
        architectAgentId: "architect",
        nexoraId: "NX-1",
        total: 2,
        done: 1,
        packages: [
          {
            packageId: "wp1",
            title: "First",
            nexoraId: null,
            assigneeAgentId: null,
            progress: "cancelled",
          },
          {
            packageId: "wp2",
            title: "Second",
            nexoraId: "NX-2",
            assigneeAgentId: "developer-agent",
            progress: "reviewed",
          },
          {
            packageId: "wp3",
            title: "Third",
            nexoraId: null,
            assigneeAgentId: "dev-two",
            progress: "assigned",
          },
        ],
      },
    ]);
    assert.deepEqual(tasks.requirements, []);
  } finally {
    await close(t.h);
  }
});

test("a signed-off, a cancelled and a superseded plan are absent; a draft and a plan in review are present with their current titles", async () => {
  const t = await team();
  try {
    const { h } = t;
    const signed = open(h, "signed");
    submit(t, signed, body(["wp1", "One"]));
    assign(t, signed, "wp1", t.dev1);
    const report = reportBy(h, t.dev1, "1".repeat(40));
    passReview(h, report);
    const record = await integrate(
      {
        core: h.core,
        git: git(),
        context: (credential: string) => ctx(h.core, credential),
        credential: h.owner,
        log: () => undefined,
      },
      { reportIds: [report], requestedBy: "operator" },
    );
    passReview(h, record.integrationId);
    h.core.recordSignoff(ctx(h.core, t.architect.credential), {
      planId: signed,
      integrationId: record.integrationId,
      summary: "looks right",
    });
    const cancelled = open(h, "cancelled");
    h.core.cancelPlan(ctx(h.core, h.owner), { planId: cancelled });
    const old = open(h, "old");
    submit(t, old, body(["wp1", "Old one"]));
    const next = open(h, "next", old);
    submit(t, next, body(["wp1", "Next one"]));
    assert.equal(h.core.planRecord(h.owner, old)!.plan.state, "superseded");
    const draft = open(h, "draft");
    const reviewing = open(h, "reviewing");
    submit(t, reviewing, body(["wp1", "Under review"]), true);
    assert.equal(
      h.core.planRecord(h.owner, reviewing)!.plan.state,
      "in_review",
    );

    const plans = h.core.activeTasks(h.owner, CAPS).plans;
    assert.deepEqual(
      plans.map((p) => [p.planId, p.state]),
      [
        [next, "approved"],
        [draft, "draft"],
        [reviewing, "in_review"],
      ],
    );
    assert.deepEqual(
      plans.find((p) => p.planId === reviewing)!.packages.map((p) => p.title),
      ["Under review"],
    );
    assert.deepEqual(plans.find((p) => p.planId === draft)!.packages, []);
  } finally {
    await close(t.h);
  }
});

test("a requirement shows for an active agent spawned with it or bound to it, with the link's Nexora id and the spawn title, and goes when the agents are released", async () => {
  const t = await team();
  try {
    const { h } = t;
    pane(h, t.dev1.agentId, "req-1", "Fix it");
    pane(h, t.dev2.agentId, "plan-1/wp1", "a package");
    link(h, "requirement", "req-1", "NX-9");
    link(h, "requirement", "req-2", "NX-10");
    h.core.bindRequirement(ctx(h.core, h.owner), {
      refId: "req-2",
      agentId: t.dev3.agentId,
    });
    pane(h, t.dev3.agentId, undefined, "bound title");

    assert.deepEqual(h.core.activeTasks(h.owner, CAPS).requirements, [
      {
        refId: "req-1",
        nexoraId: "NX-9",
        title: "Fix it",
        agentIds: [t.dev1.agentId],
      },
      {
        refId: "req-2",
        nexoraId: "NX-10",
        title: "bound title",
        agentIds: [t.dev3.agentId],
      },
    ]);
    h.core.endAgent(ctx(h.core, h.owner), t.dev1.agentId);
    h.core.endAgent(ctx(h.core, h.owner), t.dev3.agentId);
    assert.deepEqual(h.core.activeTasks(h.owner, CAPS).requirements, []);
  } finally {
    await close(t.h);
  }
});

test("the caps cut 12 plans to 10 (newest kept), 35 packages to 30 and 25 requirements to 20 and set truncated; otherwise it is false", async () => {
  const t = await team();
  try {
    const { h } = t;
    assert.equal(h.core.activeTasks(h.owner, CAPS).truncated, false);
    const ids: string[] = [];
    for (let i = 1; i <= 12; i++) ids.push(open(h, `plan ${i}`));
    const big = ids[11]!;
    // The plan limit is 20 packages, so a larger body is written to the ledger directly.
    const bigBody = body(
      ...Array.from({ length: 35 }, (_, i): [string, string] => [
        `wp${i + 1}`,
        `Package ${i + 1}`,
      ]),
    );
    const db = openSqlite(`${h.stateDirectory}/controller.sqlite`);
    try {
      db.prepare(
        `INSERT INTO plan_revisions(project_id, plan_id, revision, base_sha, body_json, body_sha, author_agent_id, author_actor_id, created_at)
         VALUES (?, ?, 1, ?, ?, ?, ?, ?, ?)`,
      ).run(
        h.info.projectId,
        big,
        BASE,
        bigBody,
        "e".repeat(64),
        t.architect.agentId,
        t.architect.actorId,
        "2026-10-01T00:00:00.000Z",
      );
      db.prepare(
        "UPDATE plans SET current_revision = 1 WHERE project_id = ? AND plan_id = ?",
      ).run(h.info.projectId, big);
    } finally {
      db.close();
    }
    const plans = h.core.activeTasks(h.owner, CAPS).plans;
    assert.deepEqual(
      plans.map((p) => p.planId),
      ids.slice(2),
    );
    assert.equal(plans.at(-1)!.packages.length, 30);
    assert.equal(plans.at(-1)!.total, 35);

    for (let i = 1; i <= 25; i++) {
      const name = `agent-${String(i).padStart(2, "0")}`;
      member(h, name, "Developer", "developer");
      pane(h, name, `req-${String(i).padStart(2, "0")}`);
    }
    const tasks = h.core.activeTasks(h.owner, CAPS);
    assert.equal(tasks.requirements.length, 20);
    assert.equal(tasks.truncated, true);
  } finally {
    await close(t.h);
  }
});

test("only the operator gets activeTasks and panes in status", async () => {
  const t = await team();
  try {
    const { h } = t;
    pane(h, t.dev1.agentId, "req-1", "Fix it");
    const ok = (response: CommandResponse): Record<string, unknown> => {
      assert.equal(response.ok, true);
      return (response as unknown as { result: Record<string, unknown> })
        .result;
    };
    const operator = ok(await call(h, h.owner, "status", []));
    assert.ok(operator.activeTasks);
    assert.deepEqual(
      (operator.panes as { taskRef: string | null }[]).map((p) => p.taskRef),
      ["req-1"],
    );
    const pm = ok(await call(h, h.pm.credential, "status", []));
    assert.equal(pm.activeTasks, undefined);
    assert.equal(pm.panes, undefined);
    assert.throws(() => h.core.activeTasks(h.pm.credential, CAPS));
  } finally {
    await close(t.h);
  }
});

const FIXTURE = path.resolve(
  import.meta.dirname,
  "..",
  "..",
  "test",
  "fixtures",
  "status-active-tasks.json",
);

/** Random ids and clock readings of a status result replaced by stable stand-ins, in order of first appearance. */
function normalized(value: unknown): unknown {
  const uuids = new Map<string, string>();
  const walk = (node: unknown): unknown => {
    if (typeof node === "string") {
      if (/^\d{4}-\d\d-\d\dT[\d:.]+Z$/.test(node))
        return "2026-01-01T00:00:00.000Z";
      if (/^p[0-9a-f]{32}$/.test(node)) return "project-id";
      if (/^[0-9a-f]{8}(-[0-9a-f]{4}){3}-[0-9a-f]{12}$/.test(node)) {
        if (!uuids.has(node)) uuids.set(node, `uuid-${uuids.size + 1}`);
        return uuids.get(node);
      }
      return node;
    }
    if (Array.isArray(node)) return node.map(walk);
    if (typeof node === "object" && node !== null)
      return Object.fromEntries(
        Object.entries(node).map(([key, entry]) => [key, walk(entry)]),
      );
    return node;
  };
  return walk(value);
}

test("test/fixtures/status-active-tasks.json equals the operator status result of the seeded ledger", async () => {
  const t = await team();
  try {
    const { h } = t;
    const planId = open(h, "the plan");
    submit(t, planId, body(["wp1", "First"], ["wp2", "Second"]));
    link(h, "package", `${planId}/wp1`, "NX-1");
    assign(t, planId, "wp1", t.dev1);
    const report = reportBy(h, t.dev1, "1".repeat(40));
    passReview(h, report);
    assign(t, planId, "wp2", t.dev2);
    pane(h, t.dev3.agentId, "req-1", "Fix the thing");
    link(h, "requirement", "req-1", "NX-7");
    const reviewer = member(h, "reviewer-live", "Verifier", "reviewer");
    const second = reportBy(h, t.dev3, "2".repeat(40));
    h.core.beginReview(ctx(h.core, h.pm.credential), {
      subjectId: second,
      reviewerRole: "reviewer",
      reviewerAgentId: reviewer.agentId,
    });
    member(h, "plain", "Developer", "developer");

    const response = await call(h, h.owner, "status", []);
    assert.equal(response.ok, true);
    const actual = normalized(
      (response as unknown as { result: unknown }).result,
    );
    if (process.env.UPDATE_FIXTURES === "1")
      fs.writeFileSync(FIXTURE, `${JSON.stringify(actual, null, 2)}\n`);
    assert.deepEqual(actual, JSON.parse(fs.readFileSync(FIXTURE, "utf8")));
  } finally {
    await close(t.h);
  }
});
