import assert from "node:assert/strict";
import { test } from "node:test";
import type { CapstanConfig } from "../src/config/capstan-config.js";
import {
  call,
  close,
  ctx,
  harness,
  type Harness,
  type Member,
} from "./harness.js";

const HEAD = "d".repeat(40);

function config(planReview: "high_risk" | "always"): CapstanConfig {
  return {
    architect: {
      enabled: true,
      role: "developer2",
      planReview,
      reviewerRole: null,
      maxPackages: 8,
      countTowardWorkerLimit: false,
      highRiskTriggers: [],
    },
    roles: [
      { name: "pm", kind: "PM" },
      { name: "developer", kind: "Developer" },
      { name: "developer2", kind: "Developer" },
      { name: "reviewer", kind: "Verifier" },
    ],
  } as unknown as CapstanConfig;
}

function planBody(): string {
  return JSON.stringify({
    summary: "split the work",
    packages: [
      {
        id: "wp1",
        title: "first",
        owns: ["src/a/"],
        acceptance: ["a works"],
        estimate_hours: 2,
      },
      {
        id: "wp2",
        title: "second",
        owns: ["src/b/"],
        depends_on: ["wp1"],
        acceptance: ["b works"],
        estimate_hours: 2,
      },
    ],
    risks: ["none known"],
  });
}

interface Stub {
  spawns: string[];
  releases: string[];
  failSpawn: Error | undefined;
}

function addReviewer(h: Harness, name: string): Member {
  h.core.createSeat(ctx(h.core, h.owner), {
    seatId: `${name}-seat`,
    name,
    role: "Verifier",
  });
  const actor = h.core.createActor(ctx(h.core, h.owner), {
    displayName: name,
    role: "Verifier",
    seatId: `${name}-seat`,
  });
  h.core.registerAgent(ctx(h.core, h.owner), {
    agentId: name,
    roleName: "reviewer",
    seatId: `${name}-seat`,
    actorId: actor.actorId,
  });
  return {
    agentId: name,
    credential: actor.credential,
    actorId: actor.actorId,
  };
}

async function withPlans(
  planReview: "high_risk" | "always",
  run: (
    h: Harness,
    stub: Stub,
    architect: Member,
    reviewers: Member[],
  ) => Promise<void>,
): Promise<void> {
  const stub: Stub = { spawns: [], releases: [], failSpawn: undefined };
  const reviewers: Member[] = [];
  const holder: { h?: Harness } = {};
  const h = await harness({
    commands: {
      config: config(planReview),
      commitExists: async () => true,
      integrationGit: { headCommit: async () => HEAD } as never,
      launcher: {
        spawn: async (role: string) => {
          stub.spawns.push(role);
          if (stub.failSpawn) throw stub.failSpawn;
          const reviewer = addReviewer(
            holder.h!,
            `reviewer-${reviewers.length + 1}`,
          );
          reviewers.push(reviewer);
          return { state: "started", agentId: reviewer.agentId };
        },
        release: async (agentId: string) => {
          stub.releases.push(agentId);
          return { state: "released", agentId };
        },
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
        ["developer2", "Developer"],
        ["reviewer", "Verifier"],
      ].map(([name, kind], index) => ({
        name: name!,
        kind: kind as "PM" | "Developer" | "Verifier",
        host: "claude",
        configHash: String.fromCharCode(97 + index).repeat(64),
      })),
    );
    await run(h, stub, h.addMember("developer2", "Developer"), reviewers);
  } finally {
    await close(h);
  }
}

const bodies = (h: Harness, agentId: string): string[] =>
  h.core.messagesFor(agentId).map((m) => m.body);

async function open(h: Harness, tier: "normal" | "high-risk"): Promise<string> {
  const answer = await call(h, h.pm.credential, "plan", ["open", tier, "T"]);
  assert.ok(answer.ok, JSON.stringify(answer));
  return (answer as { result: { planId: string } }).result.planId;
}

test("a normal plan is approved at once, tells the PM and spawns no reviewer", async () => {
  await withPlans("high_risk", async (h, stub, architect) => {
    const planId = await open(h, "normal");
    const answer = await call(h, architect.credential, "plan", [
      "submit",
      planId,
      planBody(),
    ]);
    assert.equal(
      (answer as { result: { state: string } }).result.state,
      "approved",
    );
    assert.deepEqual(stub.spawns, []);
    const notices = bodies(h, h.pm.agentId).filter((b) =>
      b.startsWith(`Plan ${planId} approved`),
    );
    assert.equal(notices.length, 1);
    assert.match(notices[0]!, /wp2: "second"; depends on: wp1/);
    assert.match(notices[0]!, /Integration order: /);
  });
});

test("a high-risk submit spawns one reviewer, sets in_review, and a pass approves the plan and tells the PM", async () => {
  await withPlans("high_risk", async (h, stub, architect, reviewers) => {
    const planId = await open(h, "high-risk");
    const answer = await call(h, architect.credential, "plan", [
      "submit",
      planId,
      planBody(),
    ]);
    assert.ok(answer.ok, JSON.stringify(answer));
    const result = (answer as { result: Record<string, unknown> }).result;
    assert.equal(result.state, "in_review");
    assert.equal(result.reviewerAgentId, reviewers[0]!.agentId);
    assert.deepEqual(stub.spawns, ["reviewer"]);
    assert.equal(h.core.listPlans(h.owner)[0]?.state, "in_review");
    assert.deepEqual(
      bodies(h, h.pm.agentId).filter((b) => b.startsWith("Plan ")),
      [],
    );
    const verdict = await call(h, reviewers[0]!.credential, "review", [
      "pass",
      "sound",
    ]);
    assert.ok(verdict.ok, JSON.stringify(verdict));
    assert.equal(h.core.listPlans(h.owner)[0]?.state, "approved");
    assert.equal(
      bodies(h, h.pm.agentId).filter((b) =>
        b.startsWith(`Plan ${planId} approved`),
      ).length,
      1,
    );
    assert.ok(
      bodies(h, architect.agentId).some(
        (b) => b.startsWith("Review ") && b.includes("PASS"),
      ),
    );
  });
});

test("plan_review = always reviews a normal plan too", async () => {
  await withPlans("always", async (h, stub, architect) => {
    const planId = await open(h, "normal");
    const answer = await call(h, architect.credential, "plan", [
      "submit",
      planId,
      planBody(),
    ]);
    assert.equal(
      (answer as { result: { state: string } }).result.state,
      "in_review",
    );
    assert.equal(stub.spawns.length, 1);
  });
});

test("a reviewer that cannot spawn leaves the plan a draft and tells the architect", async () => {
  await withPlans("high_risk", async (h, stub, architect) => {
    const planId = await open(h, "high-risk");
    stub.failSpawn = new Error("herdr is down");
    const failed = await call(h, architect.credential, "plan", [
      "submit",
      planId,
      planBody(),
    ]);
    assert.equal(failed.ok, false);
    assert.equal(h.core.listPlans(h.owner)[0]?.state, "draft");
    assert.ok(
      bodies(h, architect.agentId).some((b) =>
        b.startsWith(`Plan ${planId} review could not start`),
      ),
    );
    stub.failSpawn = undefined;
    const again = await call(h, architect.credential, "plan", [
      "submit",
      planId,
      planBody(),
    ]);
    assert.ok(again.ok, JSON.stringify(again));
    assert.equal(
      (again as { result: { revision: number } }).result.revision,
      2,
    );
  });
});

test("findings send the plan back to draft, a resubmit starts round 2, and round 6 is refused after the PM is told", async () => {
  await withPlans("high_risk", async (h, stub, architect, reviewers) => {
    const planId = await open(h, "high-risk");
    for (let round = 1; round <= 5; round += 1) {
      const submitted = await call(h, architect.credential, "plan", [
        "submit",
        planId,
        planBody(),
      ]);
      assert.ok(submitted.ok, JSON.stringify(submitted));
      const verdict = await call(
        h,
        reviewers[round - 1]!.credential,
        "review",
        ["findings", `fix round ${round}`],
      );
      assert.ok(verdict.ok, JSON.stringify(verdict));
      assert.equal(h.core.listPlans(h.owner)[0]?.state, "draft");
      assert.equal(h.core.reviews(h.owner, 10)[0]!.round, round);
    }
    assert.equal(stub.spawns.length, 5);
    assert.equal(
      bodies(h, h.pm.agentId).filter((b) =>
        b.startsWith(`Plan ${planId} needs attention`),
      ).length,
      1,
    );
    const sixth = await call(h, architect.credential, "plan", [
      "submit",
      planId,
      planBody(),
    ]);
    assert.equal(sixth.ok, false);
    assert.match((sixth as { message: string }).message, /^review_limit: /);
    assert.equal(stub.spawns.length, 5);
    assert.equal(h.core.listPlans(h.owner)[0]?.state, "draft");
  });
});

test("the architect may request a review of its own plan by id", async () => {
  await withPlans("high_risk", async (h, stub, architect) => {
    const planId = await open(h, "high-risk");
    await call(h, architect.credential, "plan", ["submit", planId, planBody()]);
    const dup = await call(h, architect.credential, "request-review", [planId]);
    assert.equal(dup.ok, false);
    assert.match((dup as { message: string }).message, /already open/);
    assert.equal(stub.spawns.length, 1);
    assert.equal(
      (await call(h, h.developer.credential, "request-review", [planId])).ok,
      false,
    );
  });
});
