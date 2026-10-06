import assert from "node:assert/strict";
import { test } from "node:test";
import type { CommandResponse } from "../src/daemon.js";
import { call, close, harness } from "./harness.js";
import { operatorWorld } from "./operator-harness.js";

function answered(response: CommandResponse): Record<string, unknown> {
  assert.equal(response.ok, true, JSON.stringify(response));
  return (response as unknown as { result: Record<string, unknown> }).result;
}

const SHA = "a".repeat(40);
const HEAD = "b".repeat(40);

/** Seeds the ledger directly: the counts and the merged list read rows, not the flows that wrote them. */
function seed(db: import("../src/controller/sqlite.js").Database): void {
  db.exec("PRAGMA foreign_keys = OFF");
  const project = (
    db.prepare("SELECT project_id FROM projects").get() as {
      project_id: string;
    }
  ).project_id;
  const now = "2026-01-01T00:00:00.000Z";
  const report = db.prepare(
    `INSERT INTO agent_reports(project_id, report_id, sequence, agent_id, generation, actor_id, commit_sha, branch, summary, state, reason, evidence_json, created_at)
     VALUES (?, ?, ?, 'a', 1, 'x', ?, 'b', 's', ?, ?, '{}', ?)`,
  );
  for (let i = 1; i <= 25; i++)
    report.run(project, `r${i}`, i, SHA, "accepted", null, now);
  report.run(project, "rj", 26, SHA, "rejected", "no_branch", now);
  const review = db.prepare(
    `INSERT INTO reviews(project_id, review_id, sequence, round, subject_report_id, commit_sha, base_sha, author_agent_id, author_actor_id,
       requested_by_actor_id, reviewer_role, reviewer_agent_id, reviewer_actor_id, state, verdict_text, created_at)
     VALUES (?, ?, ?, 1, ?, ?, ?, 'a', 'x', 'x', 'reviewer', 'v', 'y', 'passed', 'ok', ?)`,
  );
  for (let i = 1; i <= 23; i++)
    review.run(project, `rv${i}`, i, `r${i}`, SHA, SHA, now);
  const integration = db.prepare(
    `INSERT INTO integrations(project_id, integration_id, sequence, base_sha, branch, requested_by, state, head_sha, conflict_report_id,
       conflict_files_json, conflict_files_omitted, failure_reason, created_at, completed_at)
     VALUES (?, ?, ?, ?, ?, 'pm', ?, ?, ?, ?, ?, ?, ?, ?)`,
  );
  const states = [
    "merged", // i1, review passed after findings
    "merged", // i2, no review
    "confirmed",
    "discarded",
    "conflicted",
    "failed",
    "running",
  ];
  for (let i = 1; i <= 21; i++) {
    const state = i <= 7 ? states[i - 1]! : "confirmed";
    const headed = ["merged", "confirmed", "discarded"].includes(state);
    integration.run(
      project,
      `i${i}`,
      i,
      SHA,
      `br${i}`,
      state,
      headed ? HEAD : null,
      state === "conflicted" ? "r1" : null,
      state === "conflicted" ? '["f"]' : null,
      state === "conflicted" ? 1 : null,
      state === "failed" ? "boom" : null,
      `2026-01-01T00:00:${String(i).padStart(2, "0")}.000Z`,
      state === "running" ? null : now,
    );
  }
  const integrationReview = db.prepare(
    `INSERT INTO reviews(project_id, review_id, sequence, round, subject_integration_id, commit_sha, base_sha, reviewer_role,
       reviewer_agent_id, reviewer_actor_id, requested_by_actor_id, state, verdict_text, created_at)
     VALUES (?, ?, ?, ?, 'i1', ?, ?, 'reviewer', 'v', 'y', 'x', ?, 'v', ?)`,
  );
  integrationReview.run(project, "ir1", 24, 1, SHA, SHA, "findings", now);
  integrationReview.run(project, "ir2", 25, 2, SHA, SHA, "passed", now);
}

test("operator status carries pipeline totals beyond the 20-row lists, and the merged integrations", async () => {
  const w = await operatorWorld({});
  try {
    w.raw(seed);
    const status = answered(await call(w.h, w.h.owner, "status", [])) as {
      pipelineCounts: Record<string, Record<string, number>>;
      awaitingConfirm: Record<string, unknown>[];
      reports: unknown[];
      reviews: unknown[];
      integrations: unknown[];
    };
    const sum = (m: Record<string, number>) =>
      Object.values(m).reduce((a, b) => a + b, 0);
    assert.equal(status.pipelineCounts.reports!.accepted, 25);
    assert.equal(sum(status.pipelineCounts.reviews!), 25);
    assert.equal(status.pipelineCounts.reviews!.passed, 24);
    assert.equal(sum(status.pipelineCounts.integrations!), 21);
    assert.equal(status.pipelineCounts.integrations!.merged, 2);
    assert.equal(status.reports.length, 20);
    assert.equal(status.reviews.length, 20);
    assert.equal(status.integrations.length, 20);
    assert.deepEqual(status.awaitingConfirm, [
      {
        integrationId: "i2",
        branch: "br2",
        createdAt: "2026-01-01T00:00:02.000Z",
        reviewState: null,
      },
      {
        integrationId: "i1",
        branch: "br1",
        createdAt: "2026-01-01T00:00:01.000Z",
        reviewState: "passed",
      },
    ]);
  } finally {
    await w.stop();
  }
});

test("pendingProposals lists only proposed proposals, for the operator, when [operator] is on", async () => {
  const w = await operatorWorld({});
  try {
    const open = w.propose("echo one");
    const approved = w.approve(w.propose("echo two"));
    const status = answered(await call(w.h, w.h.owner, "status", [])) as {
      pendingProposals: Record<string, unknown>[];
    };
    const ids = status.pendingProposals.map((p) => p.proposalId);
    assert.deepEqual(status.pendingProposals[0], {
      proposalId: open.proposalId,
      kind: open.kind,
      command: open.command,
      proposer: open.proposerAgentId,
      reason: open.reason,
      createdAt: open.createdAt,
    });
    assert.deepEqual(ids, [open.proposalId]);
    assert.equal(ids.includes(approved.proposalId), false);
    const pm = answered(await call(w.h, w.h.pm.credential, "status", []));
    for (const key of ["pipelineCounts", "awaitingConfirm", "pendingProposals"])
      assert.equal(key in pm, false, key);
  } finally {
    await w.stop();
  }
});

test("without [operator] the status has the counts but no pendingProposals", async () => {
  const h = await harness();
  try {
    const status = answered(await call(h, h.owner, "status", []));
    assert.equal("pipelineCounts" in status, true);
    assert.equal("pendingProposals" in status, false);
  } finally {
    await close(h);
  }
});
