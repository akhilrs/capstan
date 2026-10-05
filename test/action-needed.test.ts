import assert from "node:assert/strict";
import test from "node:test";
/* The test drives the controller's real notice builders, so a reworded notice fails it. */
/* eslint-disable no-restricted-imports */
import { controllerActionNeeded } from "../src/controller/action-needed.js";
import {
  agentStuckNotice,
  deliveryProblemNotice,
  findingNoticeBody,
  findingTask,
  integrationConflictNotice,
  lostNotice,
  packageReviewedNotice,
  planApprovedNotice,
  planCancelledNotice,
  planNeedsAttentionNotice,
  planSignedOffNotice,
  reportNotice,
  reviewNotice,
} from "../src/controller/helpers.js";
/* eslint-enable no-restricted-imports */
import { proposalNoticeToPm } from "../src/operator.js";

const finding = {
  finding_id: "finding-1",
  target_agent_id: "developer-1",
  raised_by_agent_id: "supervisor-1",
  severity: "warning",
  evidence_text: "it loops",
  requested_correction: "stop",
  resolution_condition: "no loop",
  interventions: 1,
  state_reason: null,
} as never;

const proposal = {
  proposalId: "op-1",
  kind: "command",
  command: "ls",
  commandSha: "a".repeat(64),
  reason: "look",
  forceRestart: false,
  proposerAgentId: "operator-1",
  autoRule: null,
} as never;

const planBody = JSON.stringify({
  packages: [{ id: "wp1", title: "One" }],
});

const needsAction: Array<[string, string]> = [
  [
    "plan signed off",
    planSignedOffNotice("plan-1", "integration-1", "integration/x", "a", "ok"),
  ],
  ["plan approved", planApprovedNotice("plan-1", planBody, null)],
  ["plan needs attention", planNeedsAttentionNotice("plan-1")],
  ["plan cancelled", planCancelledNotice("plan-1", undefined)],
  ["package cancelled", planCancelledNotice("plan-1", "wp1")],
  [
    "package reviewed",
    packageReviewedNotice("plan-1", "wp1", "report-1", "b".repeat(40)),
  ],
  [
    "integration conflict",
    integrationConflictNotice("integration-1", "report-1", ["a.ts"], 2),
  ],
  ["operator proposal", proposalNoticeToPm(proposal)],
  [
    "delivery problem",
    deliveryProblemNotice("m-1", "developer-1", "expired", "slow", "do it"),
  ],
  ["agent stalled", agentStuckNotice("stalled", "developer-1", false)],
  ["agent blocked", agentStuckNotice("blocked", "developer-1", true)],
  [
    "agent lost",
    lostNotice(
      {
        agent_id: "developer-1",
        role_name: "developer",
        kind: "Developer",
      } as never,
      "pane_gone",
      null,
      [],
      false,
    ),
  ],
  ["finding to the target", findingTask(finding, 1, "it loops")],
  ["finding raised", findingNoticeBody(finding, "raised", null)],
  ["finding escalated", findingNoticeBody(finding, "escalated", "still")],
];

const informational: Array<[string, string]> = [
  [
    "verified report",
    reportNotice(
      {
        report_id: "r-1",
        agent_id: "developer-1",
        generation: 1,
        commit_sha: "c".repeat(40),
        branch: "feat/x",
        summary: "done",
      } as never,
      "developer",
    ),
  ],
  [
    "review result",
    reviewNotice(
      {
        review_id: "rv-1",
        subject_report_id: "r-1",
        subject_plan_id: null,
        subject_integration_id: null,
        round: 1,
        state: "passed",
        reviewer_agent_id: "reviewer-1",
        reviewer_role: "reviewer",
        commit_sha: "c".repeat(40),
        verdict_text: "fine",
      } as never,
      ["developer-1"],
    ),
  ],
  ["teammate text", "Please look at the Finding in my branch"],
  [
    "operator proposal that ran automatically",
    "Operator proposal op-1 was approved automatically but not run: the run is paused (x).",
  ],
  ["pause notice", "The operator paused the run: why."],
];

for (const [name, body] of needsAction)
  test(`the controller notice "${name}" needs action`, () => {
    assert.equal(controllerActionNeeded(body), true, body);
  });

for (const [name, body] of informational)
  test(`"${name}" is informational`, () => {
    assert.equal(controllerActionNeeded(body), false, body);
  });
