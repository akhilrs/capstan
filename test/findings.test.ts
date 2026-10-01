import assert from "node:assert/strict";
import { test } from "node:test";
import Database from "better-sqlite3";
import {
  MAX_FINDING_CONDITION_BYTES,
  MAX_FINDING_CORRECTION_BYTES,
  MAX_FINDING_EVIDENCE_BYTES,
  type AgentFindingRecord,
} from "../src/controller/core.js";
import { startReportRelay } from "../src/reports.js";
import { close, ctx, harness, type Harness, type Member } from "./harness.js";

const EVIDENCE =
  'npm test failed 4 times in a row with "expected 3 got 4" at test/a.test.ts:12';
const FIX = "Read the assertion at test/a.test.ts:12 and fix the sum";
const DONE = "npm test passes";

function withRoles(h: Harness): void {
  h.core.syncRoleDefinitions(
    ctx(h.core, h.owner),
    [
      ["pm", "PM"],
      ["developer", "Developer"],
      ["developer2", "Developer"],
      ["pm2", "PM"],
      ["supervisor", "Supervisor"],
      ["reviewer", "Verifier"],
    ].map(([name, kind], index) => ({
      name: name!,
      kind: kind as "PM" | "Developer" | "Verifier" | "Supervisor",
      host: "claude",
      configHash: String.fromCharCode(97 + index).repeat(64),
    })),
  );
}

function member(
  h: Harness,
  name: string,
  kind: "Developer" | "Supervisor" | "Verifier",
  roleName: string,
): Member {
  const seatId = `${name}-seat`;
  h.core.createSeat(ctx(h.core, h.owner), { seatId, name, role: kind });
  const actor = h.core.createActor(ctx(h.core, h.owner), {
    displayName: name,
    role: kind,
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

function supervisor(h: Harness, name = "supervisor-1"): Member {
  return member(h, name, "Supervisor", "supervisor");
}

function raise(
  h: Harness,
  by: Member,
  target = h.developer.agentId,
  overrides: Partial<{
    severity: string;
    evidence: string;
    correction: string;
    doneWhen: string;
  }> = {},
): AgentFindingRecord {
  return h.core.raiseFinding(ctx(h.core, by.credential), {
    targetAgentId: target,
    severity: "high",
    evidence: EVIDENCE,
    correction: FIX,
    doneWhen: DONE,
    ...overrides,
  });
}

function check(
  h: Harness,
  by: Member,
  findingId: string,
  result: string,
  evidence = "the same failure again",
): AgentFindingRecord {
  return h.core.checkFinding(ctx(h.core, by.credential), {
    findingId,
    result,
    evidence,
  });
}

/** Delivers the message to the developer and has the developer acknowledge it. */
function ack(h: Harness, messageId: string, by = h.developer): void {
  h.core.recordSent(ctx(h.core, h.owner), messageId);
  h.core.ackMessage(ctx(h.core, by.credential), messageId);
}

function messageState(h: Harness, messageId: string): string {
  return h.core
    .messagesFor(h.developer.agentId)
    .find((m) => m.messageId === messageId)!.state;
}

function pmBodies(h: Harness): string[] {
  return h.core.messagesFor(h.pm.agentId).map((m) => m.body);
}

test("a Supervisor raises a finding: the finding, the delivery to the target and the PM notice are written together, with the supervisor's words quoted", async () => {
  const h = await harness();
  try {
    withRoles(h);
    const s = supervisor(h);
    const finding = raise(h, s);
    assert.equal(finding.state, "open");
    assert.equal(finding.interventions, 1);
    assert.equal(finding.targetAgentId, h.developer.agentId);
    assert.equal(finding.raisedByAgentId, s.agentId);
    assert.equal(finding.deliveries.length, 1);
    const [task] = h.core.messagesFor(h.developer.agentId);
    assert.equal(task!.messageId, finding.deliveries[0]!.messageId);
    assert.equal(h.core.senderOf(task!.senderActorId).role, "controller");
    assert.ok(
      task!.body.startsWith(
        `Finding ${finding.findingId} (high) from supervisor ${s.agentId}, intervention 1 of 2\n`,
      ),
    );
    assert.ok(task!.body.includes(`Evidence: ${JSON.stringify(EVIDENCE)}`));
    assert.ok(
      task!.body.includes(`Requested correction: ${JSON.stringify(FIX)}`),
    );
    assert.ok(task!.body.includes(`Done when: ${JSON.stringify(DONE)}`));
    assert.ok(task!.body.includes("not instructions from the controller"));
    const notices = pmBodies(h);
    assert.equal(notices.length, 1);
    assert.ok(
      notices[0]!.startsWith(
        `Finding ${finding.findingId} (high) on ${h.developer.agentId} raised by supervisor`,
      ),
    );
    assert.ok(notices[0]!.includes(JSON.stringify(EVIDENCE)));
    assert.equal(h.core.findings(h.owner)[0]!.findingId, finding.findingId);
  } finally {
    await close(h);
  }
});

test("only an active Supervisor can raise a finding, and only about an active Developer or Verifier agent that is not itself", async () => {
  const h = await harness();
  try {
    withRoles(h);
    const s = supervisor(h);
    const reviewer = member(h, "reviewer-1", "Verifier", "reviewer");
    for (const credential of [h.developer.credential, h.pm.credential])
      assert.throws(
        () =>
          h.core.raiseFinding(ctx(h.core, credential), {
            targetAgentId: reviewer.agentId,
            severity: "low",
            evidence: EVIDENCE,
            correction: FIX,
            doneWhen: DONE,
          }),
        /capability|Authorization|not allowed|does not have/i,
      );
    assert.throws(() => raise(h, s, h.pm.agentId), /Developer or Verifier/);
    const other = supervisor(h, "supervisor-2");
    assert.throws(() => raise(h, s, other.agentId), /Developer or Verifier/);
    assert.throws(() => raise(h, s, "nobody"), /not active/);
    assert.equal(raise(h, s, reviewer.agentId).targetAgentId, reviewer.agentId);
    h.core.endAgent(ctx(h.core, h.owner), h.developer.agentId);
    assert.throws(() => raise(h, s), /not active/);
  } finally {
    await close(h);
  }
});

test("the fields are normalized and refused, never cut: severity, empty or invisible text, the byte limits and one open finding per target", async () => {
  const h = await harness();
  try {
    withRoles(h);
    const s = supervisor(h);
    assert.throws(
      () => raise(h, s, undefined, { severity: "High" }),
      /severity must be/,
    );
    assert.throws(
      () => raise(h, s, undefined, { severity: "urgent" }),
      /severity must be/,
    );
    for (const blank of ["", "   ", "⠀", "ㅤ ᅟ", "﻿", "��", "​"])
      assert.throws(
        () => raise(h, s, undefined, { evidence: blank }),
        /evidence must contain visible text/,
        JSON.stringify(blank),
      );
    assert.throws(
      () =>
        raise(h, s, undefined, {
          evidence: "e".repeat(MAX_FINDING_EVIDENCE_BYTES + 1),
        }),
      /evidence must be at most 1500 bytes/,
    );
    assert.throws(
      () =>
        raise(h, s, undefined, {
          correction: "c".repeat(MAX_FINDING_CORRECTION_BYTES + 1),
        }),
      /correction must be at most 600 bytes/,
    );
    assert.throws(
      () =>
        raise(h, s, undefined, {
          doneWhen: "d".repeat(MAX_FINDING_CONDITION_BYTES + 1),
        }),
      /done-when condition must be at most 300 bytes/,
    );
    assert.throws(
      () => raise(h, s, undefined, { evidence: "é".repeat(751) }),
      /at most 1500 bytes/,
      "limits count UTF-8 bytes",
    );
    const finding = raise(h, s, undefined, {
      evidence: "line one\r\nline two line three\u0007 end​",
      correction: "e".repeat(MAX_FINDING_CORRECTION_BYTES),
    });
    assert.equal(finding.evidence, "line one\nline two\nline three  end");
    assert.equal(
      finding.requestedCorrection.length,
      MAX_FINDING_CORRECTION_BYTES,
    );
    assert.throws(() => raise(h, s), /already open/);
  } finally {
    await close(h);
  }
});

test("a check is refused until the latest delivery is acknowledged and only by the raising Supervisor; resolved closes the finding and tells the PM", async () => {
  const h = await harness();
  try {
    withRoles(h);
    const s = supervisor(h);
    const other = supervisor(h, "supervisor-2");
    const finding = raise(h, s);
    const messageId = finding.deliveries[0]!.messageId;
    assert.throws(
      () => check(h, s, finding.findingId, "resolved"),
      /has not acknowledged/,
    );
    h.core.recordSent(ctx(h.core, h.owner), messageId);
    assert.throws(
      () => check(h, s, finding.findingId, "resolved"),
      /has not acknowledged/,
      "sent is not acked",
    );
    h.core.ackMessage(ctx(h.core, h.developer.credential), messageId);
    assert.throws(
      () => check(h, other, finding.findingId, "resolved"),
      /only the Supervisor that raised/,
    );
    assert.throws(
      () => check(h, s, finding.findingId, "fixed"),
      /resolved or unresolved/,
    );
    assert.throws(
      () => check(h, s, finding.findingId, "resolved", "  "),
      /visible text/,
    );
    const done = check(
      h,
      s,
      finding.findingId,
      "resolved",
      "npm test now passes twice",
    );
    assert.equal(done.state, "resolved");
    assert.equal(done.stateReason, null);
    assert.deepEqual(
      done.checks.map((c) => [c.afterIntervention, c.result]),
      [[1, "resolved"]],
    );
    assert.throws(
      () => check(h, s, finding.findingId, "resolved"),
      /already resolved/,
    );
    const notices = pmBodies(h);
    assert.equal(notices.length, 2);
    assert.ok(notices[1]!.includes("is resolved after intervention 1"));
    assert.ok(
      notices[1]!.includes(JSON.stringify("npm test now passes twice")),
    );
    assert.equal(raise(h, s).state, "open", "the lock cleared");
  } finally {
    await close(h);
  }
});

test("an unresolved check sends the second correction; an unresolved check after it escalates; a third intervention is never made", async () => {
  const h = await harness();
  try {
    withRoles(h);
    const s = supervisor(h);
    const first = raise(h, s);
    ack(h, first.deliveries[0]!.messageId);
    const second = check(
      h,
      s,
      first.findingId,
      "unresolved",
      "still failing with expected 3 got 4",
    );
    assert.equal(second.state, "open");
    assert.equal(second.interventions, 2);
    assert.equal(second.deliveries.length, 2);
    const task = h.core
      .messagesFor(h.developer.agentId)
      .find((m) => m.messageId === second.deliveries[1]!.messageId)!;
    assert.ok(task.body.includes("intervention 2 of 2"));
    assert.ok(
      task.body.includes(JSON.stringify("still failing with expected 3 got 4")),
    );
    assert.ok(
      task.body.includes(JSON.stringify(FIX)),
      "the original correction is kept",
    );
    assert.ok(
      task.body.includes(JSON.stringify(DONE)),
      "the original done-when is kept",
    );
    assert.throws(
      () => check(h, s, first.findingId, "unresolved"),
      /has not acknowledged/,
    );
    ack(h, second.deliveries[1]!.messageId);
    const escalated = check(
      h,
      s,
      first.findingId,
      "unresolved",
      "no change after two corrections",
    );
    assert.equal(escalated.state, "escalated");
    assert.equal(escalated.stateReason, "second_unresolved");
    assert.equal(escalated.deliveries.length, 2, "no third delivery");
    assert.deepEqual(
      escalated.checks.map((c) => [c.afterIntervention, c.result]),
      [
        [1, "unresolved"],
        [2, "unresolved"],
      ],
    );
    const last = pmBodies(h).at(-1)!;
    assert.ok(last.includes("ESCALATED to the operator"));
    assert.ok(last.includes("did not recover after two corrections"));
    assert.ok(last.includes("Interventions used: 2 of 2"));
    assert.throws(
      () => check(h, s, first.findingId, "unresolved"),
      /already escalated/,
    );
    assert.equal(
      h.core
        .messagesFor(h.developer.agentId)
        .filter((m) => m.body.startsWith("Finding")).length,
      2,
    );
    assert.equal(h.core.findings(h.owner)[0]!.state, "escalated");
  } finally {
    await close(h);
  }
});

test("a finding with no check before the deadline is escalated by the controller; deferrals and expiry do not restart the clock, a retry and the ack do", async () => {
  let nowMs = Date.parse("2026-10-01T12:00:00.000Z");
  const h = await harness({ clock: () => new Date(nowMs) });
  try {
    withRoles(h);
    const s = supervisor(h);
    const finding = raise(h, s);
    const messageId = finding.deliveries[0]!.messageId;
    const newContext = () => ctx(h.core, h.owner);
    assert.deepEqual(h.core.sweepFindings(newContext, 1800), []);
    nowMs += 1799_000;
    assert.deepEqual(h.core.sweepFindings(newContext, 1800), []);
    h.core.recordDeferral(newContext(), messageId, "agent_busy");
    nowMs += 1_000;
    assert.deepEqual(
      h.core.sweepFindings(newContext, 1800),
      [finding.findingId],
      "a deferral did not restart the clock",
    );
    const done = h.core.findings(h.owner)[0]!;
    assert.equal(done.state, "escalated");
    assert.equal(done.stateReason, "timed_out");
    assert.equal(done.checks[0]!.result, "timed_out");
    assert.match(
      done.checks[0]!.evidence,
      /no check by the supervisor within 1800 seconds/,
    );
    assert.equal(
      messageState(h, messageId),
      "cancelled",
      "the undelivered correction is cancelled with the finding",
    );
    assert.ok(
      pmBodies(h).at(-1)!.includes("recorded no check before the deadline"),
    );
    assert.deepEqual(h.core.sweepFindings(newContext, 1800), []);

    const again = raise(h, s);
    const second = again.deliveries[0]!.messageId;
    nowMs += 1500_000;
    h.core.recordSent(newContext(), second);
    nowMs += 1500_000;
    assert.deepEqual(
      h.core.sweepFindings(newContext, 1800),
      [],
      "the send restarted the clock",
    );
    h.core.ackMessage(ctx(h.core, h.developer.credential), second);
    nowMs += 1799_000;
    assert.deepEqual(
      h.core.sweepFindings(newContext, 1800),
      [],
      "the ack restarted the clock",
    );
    nowMs += 1_000;
    assert.deepEqual(h.core.sweepFindings(newContext, 1800), [again.findingId]);
    assert.throws(() => h.core.sweepFindings(newContext, 0), /positive/);
  } finally {
    await close(h);
  }
});

test("a finding is cancelled, with its notice and its undelivered correction, when its target or its raising Supervisor ends", async () => {
  const h = await harness();
  try {
    withRoles(h);
    const s = supervisor(h);
    const second = h.addMember("developer2", "Developer");
    const aboutDeveloper = raise(h, s);
    const aboutSecond = raise(h, s, second.agentId);
    h.core.endAgent(ctx(h.core, h.owner), h.developer.agentId);
    const [a, b] = [
      h.core
        .findings(h.owner)
        .find((f) => f.findingId === aboutDeveloper.findingId)!,
      h.core
        .findings(h.owner)
        .find((f) => f.findingId === aboutSecond.findingId)!,
    ];
    assert.equal(a.state, "cancelled");
    assert.equal(a.stateReason, "target_ended");
    assert.equal(b.state, "open");
    assert.ok(
      pmBodies(h).at(-1)!.includes("is cancelled: the target agent ended"),
    );
    h.core.endAgent(ctx(h.core, h.owner), s.agentId);
    const closed = h.core
      .findings(h.owner)
      .find((f) => f.findingId === aboutSecond.findingId)!;
    assert.equal(closed.state, "cancelled");
    assert.equal(closed.stateReason, "raiser_ended");
    assert.ok(
      pmBodies(h).at(-1)!.includes("the supervisor that raised it ended"),
    );
    const states = h.core.messagesFor(second.agentId).map((m) => m.state);
    assert.deepEqual(states, ["cancelled"]);
  } finally {
    await close(h);
  }
});

test("a replaced generation keeps the finding: the raiser checks with its new credential and the target's message follows the agent", async () => {
  const h = await harness();
  try {
    withRoles(h);
    const s = supervisor(h);
    const finding = raise(h, s);
    const replaced = h.core.replaceAgentGeneration(
      ctx(h.core, h.owner),
      s.agentId,
    );
    const fresh = {
      agentId: s.agentId,
      credential: replaced.credential,
      actorId: replaced.actorId,
    };
    ack(h, finding.deliveries[0]!.messageId);
    const checked = check(h, fresh, finding.findingId, "unresolved");
    assert.equal(
      checked.interventions,
      2,
      "the new generation of the raiser checked",
    );
    assert.throws(
      () => check(h, s, finding.findingId, "unresolved"),
      /authenticat|credential|revoked|not accepted|inactive/i,
    );
  } finally {
    await close(h);
  }
});

test("notices wait for exactly one active PM and are announced in order per finding, by the relay", async () => {
  const h = await harness();
  try {
    withRoles(h);
    h.core.endAgent(ctx(h.core, h.owner), h.pm.agentId);
    const s = supervisor(h);
    const finding = raise(h, s);
    ack(h, finding.deliveries[0]!.messageId);
    check(h, s, finding.findingId, "resolved", "all good now");
    assert.equal(h.core.unannouncedFindingNotices(h.owner).length, 2);
    assert.equal(h.core.messagesFor(h.pm.agentId).length, 0);
    const first = h.addMember("pm2", "PM");
    const second = member(h, "pm-extra", "Developer", "developer2");
    assert.ok(first && second);
    const relay = startReportRelay({
      core: h.core,
      credential: h.owner,
      intervalMs: 20,
      log: () => undefined,
    });
    try {
      await new Promise((resolve) => setTimeout(resolve, 150));
    } finally {
      relay.stop();
    }
    assert.equal(h.core.unannouncedFindingNotices(h.owner).length, 0);
    const bodies = h.core.messagesFor(first.agentId).map((m) => m.body);
    assert.equal(bodies.length, 2);
    assert.ok(bodies[0]!.includes("raised by supervisor"), "raised first");
    assert.ok(bodies[1]!.includes("is resolved"), "resolved second");
  } finally {
    await close(h);
  }
});

test("the relay escalates a finding the supervisor never checks", async () => {
  let nowMs = Date.parse("2026-10-01T12:00:00.000Z");
  const h = await harness({ clock: () => new Date(nowMs) });
  try {
    withRoles(h);
    const s = supervisor(h);
    const finding = raise(h, s);
    nowMs += 61_000;
    const relay = startReportRelay({
      core: h.core,
      credential: h.owner,
      intervalMs: 20,
      findingCheckSeconds: 60,
      log: () => undefined,
    });
    try {
      await new Promise((resolve) => setTimeout(resolve, 150));
    } finally {
      relay.stop();
    }
    const done = h.core
      .findings(h.owner)
      .find((f) => f.findingId === finding.findingId)!;
    assert.equal(done.state, "escalated");
    assert.equal(done.stateReason, "timed_out");
  } finally {
    await close(h);
  }
});

test("a delivery the target has not acknowledged does not allow a check, and one acknowledged late does", async () => {
  let nowMs = Date.parse("2026-10-01T12:00:00.000Z");
  const h = await harness({ clock: () => new Date(nowMs) });
  try {
    withRoles(h);
    const s = supervisor(h);
    const finding = raise(h, s);
    const messageId = finding.deliveries[0]!.messageId;
    h.core.recordSent(ctx(h.core, h.owner), messageId);
    nowMs += 700_000;
    h.core.advanceMessaging(ctx(h.core, h.owner), {
      maxDeferralSeconds: 120,
      pmAckTimeoutSeconds: 600,
      pmNotifyAfterSeconds: 300,
      notifyIntervalSeconds: 600,
      stallAfterSeconds: 900,
      workerAckTimeoutSeconds: 600,
    });
    assert.equal(messageState(h, messageId), "unacked");
    assert.throws(
      () => check(h, s, finding.findingId, "resolved"),
      /has not acknowledged/,
    );
    h.core.ackMessage(ctx(h.core, h.developer.credential), messageId);
    assert.equal(messageState(h, messageId), "acked_late");
    assert.equal(check(h, s, finding.findingId, "resolved").state, "resolved");
  } finally {
    await close(h);
  }
});

test("the ledger refuses to rewrite, delete or move back a finding, its deliveries, checks and notices", async () => {
  const h = await harness();
  try {
    withRoles(h);
    const s = supervisor(h);
    const finding = raise(h, s);
    ack(h, finding.deliveries[0]!.messageId);
    check(h, s, finding.findingId, "unresolved");
    h.core.close();
    const db = new Database(`${h.stateDirectory}/controller.sqlite`);
    try {
      assert.throws(
        () => db.exec("UPDATE agent_findings SET evidence_text = 'x'"),
        /immutable/,
      );
      assert.throws(
        () => db.exec("UPDATE agent_findings SET target_agent_id = 'other'"),
        /immutable/,
      );
      assert.throws(
        () => db.exec("UPDATE agent_findings SET interventions = 1"),
        /only grow/,
      );
      assert.throws(() => db.exec("DELETE FROM agent_findings"), /immutable/);
      assert.throws(
        () => db.exec("UPDATE agent_finding_deliveries SET attempt = 1"),
        /immutable/,
      );
      assert.throws(
        () => db.exec("DELETE FROM agent_finding_deliveries"),
        /immutable/,
      );
      assert.throws(
        () => db.exec("UPDATE agent_finding_checks SET result = 'resolved'"),
        /immutable/,
      );
      assert.throws(
        () => db.exec("DELETE FROM agent_finding_checks"),
        /immutable/,
      );
      assert.throws(
        () => db.exec("UPDATE agent_finding_notices SET event = 'resolved'"),
        /immutable/,
      );
      assert.throws(
        () =>
          db.exec(
            "UPDATE agent_finding_notices SET message_id = 'x' WHERE message_id IS NOT NULL",
          ),
        /announced once/,
      );
      assert.throws(
        () => db.exec("DELETE FROM agent_finding_notices"),
        /immutable/,
      );
      db.exec(
        "UPDATE agent_findings SET state = 'resolved', closed_at = created_at",
      );
      assert.throws(
        () =>
          db.exec("UPDATE agent_findings SET state = 'open', closed_at = NULL"),
        /only moves forward|closed finding is final/,
      );
      assert.throws(
        () => db.exec("UPDATE agent_findings SET closed_at = 'x'"),
        /final/,
      );
      assert.throws(
        () =>
          db.exec(
            `INSERT INTO agent_findings(project_id, finding_id, sequence, target_agent_id, raised_by_agent_id, raised_by_actor_id, severity, evidence_text, requested_correction, resolution_condition, state, interventions, created_at)
             SELECT project_id, 'self', 99, target_agent_id, target_agent_id, raised_by_actor_id, 'low', 'e', 'c', 'd', 'open', 1, created_at FROM agent_findings`,
          ),
        /CHECK constraint/,
      );
      assert.throws(
        () =>
          db.exec(
            `INSERT INTO agent_findings(project_id, finding_id, sequence, target_agent_id, raised_by_agent_id, raised_by_actor_id, severity, evidence_text, requested_correction, resolution_condition, state, interventions, created_at)
             SELECT project_id, 'blank', 98, target_agent_id, raised_by_agent_id, raised_by_actor_id, 'low', '  ', 'c', 'd', 'open', 1, created_at FROM agent_findings`,
          ),
        /CHECK constraint/,
      );
    } finally {
      db.close();
    }
  } finally {
    await close(h);
  }
});
