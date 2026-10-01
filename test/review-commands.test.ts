import assert from "node:assert/strict";
import { test } from "node:test";
import type { CapstanConfig } from "../src/config/capstan-config.js";
import type { ReportEvidence } from "../src/controller/core.js";
import type { MergeResult } from "../src/git.js";
import {
  call,
  close,
  ctx,
  harness,
  type Harness,
  type Member,
} from "./harness.js";

const BASE = "a".repeat(40);
const COMMIT = "b".repeat(40);
const BRANCH = "capstan/developer-1-g1";

const config = {
  roles: [
    { name: "pm", kind: "PM" },
    { name: "developer", kind: "Developer" },
    { name: "reviewer", kind: "Verifier" },
  ],
} as unknown as CapstanConfig;

interface Stub {
  spawns: Array<{ role: string; baseSha: string | undefined }>;
  releases: string[];
  failSpawn: Error | undefined;
  commitExists: boolean;
  merge: MergeResult;
  inHead: boolean;
  deleted: string[];
}

function setup(h: Harness): void {
  h.core.syncRoleDefinitions(
    ctx(h.core, h.owner),
    [
      ["pm", "PM"],
      ["developer", "Developer"],
      ["developer2", "Developer"],
      ["pm2", "PM"],
      ["reviewer", "Verifier"],
    ].map(([name, kind], index) => ({
      name: name!,
      kind: kind as "PM" | "Developer" | "Verifier",
      host: "claude",
      configHash: String.fromCharCode(97 + index).repeat(64),
    })),
  );
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

function report(h: Harness): string {
  h.core.recordAgentPane(ctx(h.core, h.owner), {
    agentId: h.developer.agentId,
    workspaceId: null,
    paneId: null,
    worktreePath: null,
    branch: BRANCH,
    baseSha: BASE,
  });
  const evidence: ReportEvidence = {
    generation: 1,
    branch: BRANCH,
    baseSha: BASE,
    commitExists: true,
    branchTip: COMMIT,
    isAncestorOfTip: true,
    isAncestorOfBase: false,
    checkedAt: "2026-10-01T00:00:00.000Z",
  };
  return h.core.recordAgentReport(ctx(h.core, h.developer.credential), {
    commitSha: COMMIT,
    summary: "done",
    evidence,
  }).record.reportId;
}

async function withHarness(
  run: (h: Harness, stub: Stub, spawned: Member[]) => Promise<void>,
): Promise<void> {
  const stub: Stub = {
    spawns: [],
    releases: [],
    failSpawn: undefined,
    commitExists: true,
    merge: { kind: "merged", headSha: "d".repeat(40) },
    inHead: false,
    deleted: [],
  };
  let n = 0;
  const holder: { h?: Harness } = {};
  const spawned: Member[] = [];
  const spawnReviewer = (): Member => {
    const member = addReviewer(holder.h!, `reviewer-${(n += 1)}`);
    spawned.push(member);
    return member;
  };
  const h = await harness({
    commands: {
      config,
      commitExists: async () => stub.commitExists,
      integrationGit: {
        headCommit: async () => BASE,
        commitExists: async () => true,
        merge: async () => stub.merge,
        branchTip: async () => null,
        isInHead: async () => stub.inHead,
        deleteBranch: async (branch: string) => {
          stub.deleted.push(branch);
          return true;
        },
      },
      launcher: {
        launchPm: async () => ({ state: "started", agentId: "pm-1" }),
        restartPm: async () => ({ state: "started", agentId: "pm-1" }),
        spawn: async (role: string, options?: { baseSha?: string }) => {
          stub.spawns.push({ role, baseSha: options?.baseSha });
          if (stub.failSpawn) throw stub.failSpawn;
          return { state: "started", agentId: spawnReviewer().agentId };
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
    setup(h);
    await run(h, stub, spawned);
  } finally {
    await close(h);
  }
}

test("request-review spawns a reviewer at the reported commit, records the review and answers with the reviewer and the round", async () => {
  await withHarness(async (h, stub) => {
    const reportId = report(h);
    const answer = await call(h, h.pm.credential, "request-review", [reportId]);
    assert.ok(answer.ok, JSON.stringify(answer));
    const result = (answer as { result: Record<string, unknown> }).result;
    assert.equal(result.round, 1);
    assert.equal(result.reviewerRole, "reviewer");
    assert.equal(result.commit, COMMIT);
    assert.deepEqual(stub.spawns, [{ role: "reviewer", baseSha: COMMIT }]);
    assert.equal(
      h.core.reviews(h.owner, 5)[0]!.reviewerAgentId,
      result.reviewerAgentId,
    );
    const again = await call(h, h.pm.credential, "request-review", [reportId]);
    assert.equal(again.ok, false);
    assert.match((again as { message: string }).message, /already open/);
    assert.equal(
      stub.spawns.length,
      1,
      "the second request was refused before any spawn",
    );
    assert.deepEqual(stub.releases, []);
  });
});

test("request-review is for the PM only, validates its arguments and refuses a missing commit without spawning", async () => {
  await withHarness(async (h, stub) => {
    const reportId = report(h);
    for (const credential of [h.developer.credential, h.owner])
      assert.equal(
        (
          (await call(h, credential, "request-review", [reportId])) as {
            code: string;
          }
        ).code,
        "forbidden",
      );
    for (const args of [
      [],
      ["a", "b", "c"],
      ["bad id"],
      [reportId, "Bad Role"],
    ])
      assert.equal(
        (
          (await call(h, h.pm.credential, "request-review", args)) as {
            code: string;
          }
        ).code,
        "invalid_request",
        JSON.stringify(args),
      );
    stub.commitExists = false;
    const missing = await call(h, h.pm.credential, "request-review", [
      reportId,
    ]);
    assert.match((missing as { message: string }).message, /^commit_missing:/);
    assert.deepEqual(stub.spawns, []);
    stub.commitExists = true;
    const unknown = await call(h, h.pm.credential, "request-review", [
      reportId,
      "developer",
    ]);
    assert.match(
      (unknown as { message: string }).message,
      /^unknown_reviewer_role:/,
    );
    const noReport = await call(h, h.pm.credential, "request-review", [
      "no-such-report",
    ]);
    assert.match(
      (noReport as { message: string }).message,
      /^review_refused: the report or integration does not exist/,
    );
    assert.deepEqual(
      stub.spawns,
      [],
      "nothing was spawned for any refused request",
    );
  });
});

test("a failed spawn costs no review round and a failure after the spawn releases the reviewer", async () => {
  await withHarness(async (h, stub) => {
    const reportId = report(h);
    stub.failSpawn = new Error("herdr is down");
    const failed = await call(h, h.pm.credential, "request-review", [reportId]);
    assert.equal(failed.ok, false);
    assert.equal(h.core.reviews(h.owner, 5).length, 0, "no round was recorded");
    stub.failSpawn = undefined;
    h.core.endAgent(ctx(h.core, h.owner), h.pm.agentId);
    const pm2 = h.addMember("pm2", "PM");
    const ok = await call(h, pm2.credential, "request-review", [reportId]);
    assert.ok(ok.ok, JSON.stringify(ok));
    assert.equal(h.core.reviews(h.owner, 5)[0]!.round, 1);
  });
});

test("review is for a reviewer only, takes a verdict and a text, and releases the reviewer after the reply", async () => {
  await withHarness(async (h, stub, spawned) => {
    const reportId = report(h);
    const requested = await call(h, h.pm.credential, "request-review", [
      reportId,
    ]);
    const reviewerId = (requested as { result: { reviewerAgentId: string } })
      .result.reviewerAgentId;
    const reviewer = spawned[0]!;
    assert.equal(reviewer.agentId, reviewerId);
    for (const credential of [h.developer.credential, h.pm.credential, h.owner])
      assert.equal(
        (
          (await call(h, credential, "review", ["pass", "x"])) as {
            code: string;
          }
        ).code,
        "forbidden",
      );
    for (const args of [
      [],
      ["pass"],
      ["approve", "x"],
      ["pass", "   "],
      ["pass", "a", "b"],
    ])
      assert.equal(
        (
          (await call(h, reviewer.credential, "review", args)) as {
            code: string;
          }
        ).code,
        "invalid_request",
        JSON.stringify(args),
      );
    assert.deepEqual(stub.releases, []);
    const answer = await call(h, reviewer.credential, "review", [
      "findings",
      "bug at src/a.ts:3\r\nsecond line",
    ]);
    assert.ok(answer.ok, JSON.stringify(answer));
    assert.equal(
      (answer as { result: { state: string; announced: boolean } }).result
        .state,
      "findings",
    );
    assert.equal(
      (answer as { result: { announced: boolean } }).result.announced,
      true,
    );
    assert.equal(
      h.core.reviews(h.owner, 5)[0]!.verdictText,
      "bug at src/a.ts:3\nsecond line",
    );
    for (
      let waited = 0;
      waited < 1000 && stub.releases.length === 0;
      waited += 10
    )
      await new Promise((resolve) => setTimeout(resolve, 10));
    assert.deepEqual(
      stub.releases,
      [reviewerId],
      "the reviewer is released after the reply",
    );
    const second = await call(h, reviewer.credential, "review", [
      "pass",
      "again",
    ]);
    assert.equal(second.ok, false);
    assert.match(
      (second as { message: string }).message,
      /^review_refused: you have no review in progress/,
    );
    const status = await call(h, h.owner, "status");
    const reviews = (
      status as { result: { reviews: Array<{ state: string; round: number }> } }
    ).result.reviews;
    assert.deepEqual(
      reviews.map((r) => [r.round, r.state]),
      [[1, "findings"]],
    );
  });
});

test("a re-review after a fix is a new round with a different reviewer session", async () => {
  await withHarness(async (h, stub, spawned) => {
    const reportId = report(h);
    await call(h, h.pm.credential, "request-review", [reportId]);
    await call(h, spawned[0]!.credential, "review", ["findings", "fix this"]);
    await call(h, h.pm.credential, "request-review", [reportId]);
    await call(h, spawned[1]!.credential, "review", ["pass", "looks right"]);
    const reviews = h.core.reviews(h.owner, 5).slice().reverse();
    assert.deepEqual(
      reviews.map((r) => [r.round, r.state]),
      [
        [1, "findings"],
        [2, "passed"],
      ],
    );
    assert.notEqual(reviews[0]!.reviewerAgentId, reviews[1]!.reviewerAgentId);
    assert.notEqual(reviews[0]!.reviewerActorId, reviews[1]!.reviewerActorId);
    for (const r of reviews) {
      assert.notEqual(r.reviewerActorId, r.authorActorId);
      assert.equal(r.authorAgentId, h.developer.agentId);
    }
    assert.equal(stub.spawns.length, 2);
  });
});

test("integrate is for the PM and the operator, merges reviewed reports and the merged commit is reviewed like a report", async () => {
  await withHarness(async (h, stub, spawned) => {
    const reportId = report(h);
    const refused = await call(h, h.developer.credential, "integrate", [
      reportId,
    ]);
    assert.equal(refused.ok, false);
    assert.match(
      (refused as { message: string }).message,
      /only the PM or the operator/,
    );
    const empty = await call(h, h.pm.credential, "integrate", []);
    assert.equal(empty.ok, false);
    const unreviewed = await call(h, h.pm.credential, "integrate", [reportId]);
    assert.equal(unreviewed.ok, false);
    assert.match(
      (unreviewed as { message: string }).message,
      /^integration_refused: .*no passed review/,
    );

    await call(h, h.pm.credential, "request-review", [reportId]);
    await call(h, spawned[0]!.credential, "review", ["pass", "fine"]);
    const merged = await call(h, h.pm.credential, "integrate", [reportId]);
    assert.ok(merged.ok, JSON.stringify(merged));
    const result = (merged as { result: Record<string, unknown> }).result;
    assert.equal(result.state, "merged");
    assert.equal(result.head, "d".repeat(40));
    assert.deepEqual(result.reports, [reportId]);
    const integrationId = result.integrationId as string;

    const requested = await call(h, h.pm.credential, "request-review", [
      integrationId,
    ]);
    assert.ok(requested.ok, JSON.stringify(requested));
    assert.deepEqual(stub.spawns.at(-1), {
      role: "reviewer",
      baseSha: "d".repeat(40),
    });
    await call(h, spawned[1]!.credential, "review", [
      "pass",
      "the merge is right",
    ]);

    const early = await call(h, h.pm.credential, "integrate", [
      "confirm",
      integrationId,
    ]);
    assert.equal(early.ok, false);
    assert.match((early as { message: string }).message, /^not_in_head: /);
    stub.inHead = true;
    const confirmed = await call(h, h.owner, "integrate", [
      "confirm",
      integrationId,
    ]);
    assert.ok(confirmed.ok, JSON.stringify(confirmed));
    assert.equal(
      (confirmed as { result: { branchRemoved: boolean } }).result
        .branchRemoved,
      true,
    );
    assert.equal(stub.deleted.length, 1);
  });
});

test("an integration conflict is answered with the report and files and nothing is kept", async () => {
  await withHarness(async (h, stub, spawned) => {
    const reportId = report(h);
    await call(h, h.pm.credential, "request-review", [reportId]);
    await call(h, spawned[0]!.credential, "review", ["pass", "fine"]);
    stub.merge = { kind: "conflicted", reportId, files: ["a.txt"], omitted: 0 };
    const answer = await call(h, h.pm.credential, "integrate", [reportId]);
    assert.ok(answer.ok, JSON.stringify(answer));
    const result = (answer as { result: Record<string, unknown> }).result;
    assert.equal(result.state, "conflicted");
    assert.equal(result.branch, null);
    assert.deepEqual(result.conflict, { reportId, files: ["a.txt"] });
    assert.deepEqual(stub.deleted, []);
  });
});
