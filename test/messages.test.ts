import assert from "node:assert/strict";
import { mkdtempSync, readdirSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { test } from "node:test";
import Database from "better-sqlite3";
import {
  AuthenticationError,
  AuthorizationError,
} from "../src/controller/auth.js";
import {
  ControllerCore,
  IdempotencyConflictError,
  MessageTransitionError,
  MutationConflictError,
} from "../src/controller/core.js";
import type { MessagingTimers } from "../src/controller/messaging.js";
import type {
  InitialProject,
  MutationContext,
} from "../src/controller/types.js";

const timers: MessagingTimers = {
  maxDeferralSeconds: 120,
  pmAckTimeoutSeconds: 600,
  pmNotifyAfterSeconds: 300,
  notifyIntervalSeconds: 600,
  stallAfterSeconds: 900,
  workerAckTimeoutSeconds: 600,
};

const inputKinds = [
  "project_config",
  "task_brief",
  "acceptance_criteria",
  "policy",
  "plan",
] as const;

function projectInfo(): InitialProject {
  const identity = crypto.randomUUID().replaceAll("-", "");
  return {
    projectId: `p${identity}`,
    name: "Messaging test project",
    ownerCredential: `owner-${identity}`,
    initialInputs: inputKinds.map((kind) => ({
      kind,
      content:
        kind === "acceptance_criteria" ? ["criterion"] : { kind, revision: 1 },
    })),
  };
}

function ctx(core: ControllerCore, credential: string): MutationContext {
  const id = crypto.randomUUID();
  return {
    credential,
    requestId: `req-${id}`,
    idempotencyKey: `idem-${id}`,
    expectedVersion: core.stateVersion,
    inputRevision: core.inputRevision,
  };
}

interface Member {
  readonly name: string;
  readonly agentId: string;
  readonly seatId: string;
  readonly actorId: string;
  readonly credential: string;
}

interface World {
  readonly core: ControllerCore;
  readonly stateDirectory: string;
  readonly info: InitialProject;
  readonly owner: string;
  readonly pm: Member;
  readonly developer: Member;
  readonly reviewer: Member;
  readonly clock: { nowMs: number; script: number[] };
  advance(seconds: number): void;
  ctx(credential?: string): MutationContext;
}

async function world(): Promise<World> {
  const stateDirectory = mkdtempSync(path.join(tmpdir(), "capstan-messages-"));
  const info = projectInfo();
  const clock = {
    nowMs: Date.parse("2026-01-01T00:00:00.000Z"),
    script: [] as number[],
  };
  const core = await ControllerCore.open({
    stateDirectory,
    project: info,
    clock: () =>
      new Date(clock.script.length > 0 ? clock.script.shift()! : clock.nowMs),
  });
  const owner = info.ownerCredential;
  core.syncRoleDefinitions(ctx(core, owner), [
    { name: "pm", kind: "PM", host: "claude", configHash: "a".repeat(64) },
    {
      name: "developer",
      kind: "Developer",
      host: "claude",
      configHash: "b".repeat(64),
    },
    {
      name: "reviewer",
      kind: "Verifier",
      host: "claude",
      configHash: "c".repeat(64),
    },
  ]);
  const member = (
    name: string,
    kind: "PM" | "Developer" | "Verifier",
  ): Member => {
    const seatId = `${name}-seat`;
    core.createSeat(ctx(core, owner), { seatId, name, role: kind });
    const actor = core.createActor(ctx(core, owner), {
      displayName: name,
      role: kind,
      seatId,
    });
    const agentId = `${name}-agent`;
    core.registerAgent(ctx(core, owner), {
      agentId,
      roleName: name,
      seatId,
      actorId: actor.actorId,
    });
    return {
      name,
      agentId,
      seatId,
      actorId: actor.actorId,
      credential: actor.credential,
    };
  };
  return {
    core,
    stateDirectory,
    info,
    owner,
    pm: member("pm", "PM"),
    developer: member("developer", "Developer"),
    reviewer: member("reviewer", "Verifier"),
    clock,
    advance: (seconds) => {
      clock.nowMs += seconds * 1000;
    },
    ctx: (credential = owner) => ctx(core, credential),
  };
}

function close(w: World): void {
  w.core.close();
  rmSync(w.stateDirectory, { recursive: true, force: true });
}

function send(w: World, to: Member, body = "hello", from = w.owner): string {
  return w.core.enqueueMessage(w.ctx(from), {
    recipientAgentId: to.agentId,
    body,
  }).messageId;
}

function stateOf(w: World, messageId: string): string {
  return w.core.message(messageId)!.state;
}

function assertRejected(w: World, code: string, run: () => unknown): void {
  const before = w.core.messageRejections().length;
  assert.throws(
    run,
    (error: unknown) =>
      error instanceof MessageTransitionError && error.code === code,
  );
  assert.equal(w.core.messageRejections().length, before + 1);
}

function events(
  w: World,
  entityType: string,
): Array<{
  from_state: string | null;
  to_state: string | null;
  entity_id: string;
  sequence: number;
  state_version: number;
}> {
  const db = new Database(path.join(w.stateDirectory, "controller.sqlite"));
  try {
    return db
      .prepare(
        "SELECT sequence, entity_id, from_state, to_state, state_version FROM controller_events WHERE entity_type = ? ORDER BY sequence",
      )
      .all(entityType) as never;
  } finally {
    db.close();
  }
}

test("a message moves through every legal transition and each one is audited", async () => {
  const w = await world();
  try {
    const { core, developer } = w;
    const acked = send(w, developer, "one");
    core.recordSent(w.ctx(), acked);
    assert.equal(stateOf(w, acked), "sent");
    core.ackMessage(w.ctx(developer.credential), acked);
    assert.equal(stateOf(w, acked), "acked");

    const deferred = send(w, developer, "two");
    core.recordDeferral(w.ctx(), deferred, "agent_busy");
    core.recordSent(w.ctx(), deferred);
    w.advance(601);
    assert.deepEqual(core.advanceMessaging(w.ctx(), timers).applied, [
      deferred,
    ]);
    assert.equal(stateOf(w, deferred), "unacked");
    core.ackMessage(w.ctx(developer.credential), deferred);
    assert.equal(stateOf(w, deferred), "acked_late");

    const expiring = send(w, developer, "three");
    core.recordDeferral(w.ctx(), expiring, "agent_blocked");
    w.advance(121);
    core.advanceMessaging(w.ctx(), timers);
    assert.equal(stateOf(w, expiring), "expired");
    core.resolveMessage(w.ctx(), expiring, "skip");
    assert.equal(stateOf(w, expiring), "cancelled");

    const failing = send(w, developer, "four");
    core.recordFailure(w.ctx(), failing, "pane closed");
    assert.equal(stateOf(w, failing), "failed");
    core.resolveMessage(w.ctx(), failing, "cancel");
    assert.equal(stateOf(w, failing), "cancelled");

    const queuedCancelled = send(w, developer, "five");
    core.resolveMessage(w.ctx(), queuedCancelled, "cancel");
    assert.equal(stateOf(w, queuedCancelled), "cancelled");

    const trail = events(w, "message").filter(
      (event) => event.entity_id === deferred,
    );
    assert.deepEqual(
      trail.map((event) => [event.from_state, event.to_state]),
      [
        [null, "queued"],
        ["queued", "deferred"],
        ["deferred", "sent"],
        ["sent", "unacked"],
        ["unacked", "acked_late"],
      ],
    );
    assert.deepEqual(w.core.messageRejections(), []);
  } finally {
    close(w);
  }
});

test("illegal transitions are rejected, recorded, and leave the message unchanged", async () => {
  const w = await world();
  try {
    const { core, developer, reviewer } = w;
    const queued = send(w, developer);
    assertRejected(w, "illegal_transition", () =>
      core.ackMessage(w.ctx(developer.credential), queued),
    );
    assertRejected(w, "not_recipient", () =>
      core.ackMessage(w.ctx(reviewer.credential), queued),
    );
    assertRejected(w, "illegal_resolution", () =>
      core.resolveMessage(w.ctx(), queued, "retry"),
    );
    core.recordSent(w.ctx(), queued);
    assertRejected(w, "illegal_transition", () =>
      core.recordDeferral(w.ctx(), queued, "agent_busy"),
    );
    assertRejected(w, "illegal_transition", () =>
      core.recordSent(w.ctx(), queued),
    );
    assertRejected(w, "illegal_transition", () =>
      core.recordFailure(w.ctx(), queued, "x"),
    );
    core.ackMessage(w.ctx(developer.credential), queued);
    assertRejected(w, "illegal_transition", () =>
      core.ackMessage(w.ctx(developer.credential), queued),
    );
    assertRejected(w, "illegal_resolution", () =>
      core.resolveMessage(w.ctx(), queued, "cancel"),
    );
    assertRejected(w, "unknown_message", () =>
      core.ackMessage(w.ctx(developer.credential), "missing"),
    );
    assertRejected(w, "unknown_recipient", () =>
      core.enqueueMessage(w.ctx(), { recipientAgentId: "nobody", body: "x" }),
    );
    assert.equal(stateOf(w, queued), "acked");
    assert.equal(core.message(queued)!.sendAttempts, 1);
    const recorded = core
      .messageRejections()
      .find(
        (rejection) =>
          rejection.action === "message.ack" &&
          rejection.code === "illegal_transition" &&
          rejection.fromState === "queued",
      )!;
    assert.equal(recorded.action, "message.ack");
    assert.equal(recorded.fromState, "queued");
    assert.equal(recorded.attemptedState, "acked");
    const rejectionEvents = events(w, "message_rejection");
    assert.equal(rejectionEvents.length, core.messageRejections().length);
    assert.ok(rejectionEvents.every((event) => event.to_state === null));
  } finally {
    close(w);
  }
});

test("a replayed rejected call raises the same error and records nothing twice", async () => {
  const w = await world();
  try {
    const queued = send(w, w.developer);
    const context = w.ctx(w.developer.credential);
    assert.throws(
      () => w.core.ackMessage(context, queued),
      MessageTransitionError,
    );
    const before = w.core.messageRejections().length;
    assert.throws(
      () => w.core.ackMessage(context, queued),
      (error: unknown) =>
        error instanceof MessageTransitionError &&
        error.code === "illegal_transition",
    );
    assert.equal(w.core.messageRejections().length, before);
  } finally {
    close(w);
  }
});

test("an unresolved message blocks later ones and a resolution releases them in order", async () => {
  const w = await world();
  try {
    const { core, developer } = w;
    const first = send(w, developer, "first");
    const second = send(w, developer, "second");
    const third = send(w, developer, "third");
    core.recordDeferral(w.ctx(), first, "agent_busy");
    assertRejected(w, "not_head", () => core.recordSent(w.ctx(), second));
    assertRejected(w, "not_head", () =>
      core.recordDeferral(w.ctx(), second, "agent_busy"),
    );
    core.resolveMessage(w.ctx(), first, "retry");
    assert.equal(stateOf(w, first), "queued");
    assertRejected(w, "not_head", () => core.recordSent(w.ctx(), second));
    core.recordSent(w.ctx(), first);
    assertRejected(w, "not_head", () => core.recordSent(w.ctx(), second));
    core.ackMessage(w.ctx(developer.credential), first);
    core.recordSent(w.ctx(), second);
    core.resolveMessage(w.ctx(), third, "skip");
    assert.deepEqual(
      core.messagesFor(developer.agentId).map((message) => message.state),
      ["acked", "sent", "cancelled"],
    );
    core.ackMessage(w.ctx(developer.credential), second);
    assert.equal(
      w.core
        .messagesFor(developer.agentId)
        .every((m) => ["acked", "cancelled"].includes(m.state)),
      true,
    );
  } finally {
    close(w);
  }
});

test("a queued message behind a blocked head can be cancelled by the operator", async () => {
  const w = await world();
  try {
    const first = send(w, w.developer);
    const second = send(w, w.developer);
    w.core.recordFailure(w.ctx(), first, "gone");
    w.core.resolveMessage(w.ctx(), second, "cancel", "not needed");
    assert.equal(stateOf(w, second), "cancelled");
    assert.equal(stateOf(w, first), "failed");
  } finally {
    close(w);
  }
});

test("a PM message can be resolved only by the operator; a worker message by the PM or the operator", async () => {
  const w = await world();
  try {
    const { core, pm, developer } = w;
    const toPm = send(w, pm, "report", w.developer.credential);
    assert.equal(
      core.pullMessage(w.ctx(pm.credential)).message?.messageId,
      toPm,
    );
    assert.equal(stateOf(w, toPm), "sent");
    w.advance(601);
    core.advanceMessaging(w.ctx(), timers);
    assert.equal(stateOf(w, toPm), "unacked");
    assertRejected(w, "operator_only", () =>
      core.resolveMessage(w.ctx(pm.credential), toPm, "retry"),
    );
    assert.throws(
      () => core.resolveMessage(w.ctx(developer.credential), toPm, "retry"),
      AuthorizationError,
    );
    core.resolveMessage(w.ctx(), toPm, "retry", "resend once");
    assert.equal(stateOf(w, toPm), "queued");

    const toWorker = send(w, developer, "work");
    core.recordFailure(w.ctx(), toWorker, "pane closed");
    core.resolveMessage(w.ctx(pm.credential), toWorker, "retry");
    assert.equal(stateOf(w, toWorker), "queued");
  } finally {
    close(w);
  }
});

test("no code path resends: every timer expiring finds no additional send attempt", async () => {
  const w = await world();
  try {
    const { core, developer, pm } = w;
    const sent = send(w, developer, "once");
    core.recordSent(w.ctx(), sent);
    const deferredBusy = send(w, pm, "pm note", w.developer.credential);
    const pull = core.pullMessage(w.ctx(pm.credential));
    assert.equal(pull.message?.messageId, deferredBusy);
    const stuckInput = send(w, w.reviewer, "type ahead");
    core.recordDeferral(w.ctx(), stuckInput, "input_not_empty");
    let clearActions = 0;
    for (let step = 0; step < 500; step += 1) {
      w.advance(100);
      const result = core.advanceMessaging(w.ctx(), timers);
      clearActions += result.actions.filter(
        (action) => action.kind === "clear_then_send",
      ).length;
    }
    assert.ok(
      clearActions > 0,
      "clear_then_send stays due until it is recorded",
    );
    assert.equal(core.message(sent)!.sendAttempts, 1);
    assert.equal(core.message(deferredBusy)!.sendAttempts, 1);
    assert.equal(core.message(stuckInput)!.sendAttempts, 0);
    assert.equal(stateOf(w, stuckInput), "deferred");
    assert.equal(stateOf(w, sent), "unacked");
    assert.equal(
      events(w, "message").filter((event) => event.to_state === "sent").length,
      2,
    );
  } finally {
    close(w);
  }
});

test("the PM ack timer pauses while the PM works outside a registered wait and runs inside one", async () => {
  const w = await world();
  try {
    const { core, pm } = w;
    core.recordAgentObservation(w.ctx(), pm.agentId, "working");
    const message = send(w, pm, "check", w.developer.credential);
    core.pullMessage(w.ctx(pm.credential));
    w.advance(50_000);
    assert.deepEqual(core.advanceMessaging(w.ctx(), timers).applied, []);
    assert.equal(stateOf(w, message), "sent");
    core.beginWait(w.ctx(pm.credential));
    w.advance(599);
    assert.deepEqual(core.advanceMessaging(w.ctx(), timers).applied, []);
    w.advance(1);
    assert.deepEqual(core.advanceMessaging(w.ctx(), timers).applied, [message]);
    assert.equal(stateOf(w, message), "unacked");
  } finally {
    close(w);
  }
});

test("the PM notification timer pauses outside a wait, runs inside one, repeats, and only the head notifies", async () => {
  const w = await world();
  try {
    const { core, pm } = w;
    core.recordAgentObservation(w.ctx(), pm.agentId, "working");
    const head = send(w, pm, "head", w.developer.credential);
    const behind = send(w, pm, "behind", w.developer.credential);
    w.advance(10_000);
    assert.deepEqual(core.advanceMessaging(w.ctx(), timers).actions, []);
    const { waitId } = core.beginWait(w.ctx(pm.credential));
    w.advance(299);
    assert.deepEqual(core.advanceMessaging(w.ctx(), timers).actions, []);
    w.advance(1);
    assert.deepEqual(core.advanceMessaging(w.ctx(), timers).actions, [
      { kind: "notify_operator", messageId: head, repeat: false },
    ]);
    core.recordNotification(w.ctx(), head);
    assert.deepEqual(core.advanceMessaging(w.ctx(), timers).actions, []);
    w.advance(599);
    assert.deepEqual(core.advanceMessaging(w.ctx(), timers).actions, []);
    w.advance(1);
    assert.deepEqual(core.advanceMessaging(w.ctx(), timers).actions, [
      { kind: "notify_operator", messageId: head, repeat: true },
    ]);
    core.endWait(w.ctx(pm.credential), waitId);
    assertRejected(w, "not_notifiable", () =>
      core.recordNotification(w.ctx(), behind),
    );
    core.pullMessage(w.ctx(pm.credential));
    core.ackMessage(w.ctx(pm.credential), head);
    core.pullMessage(w.ctx(pm.credential));
    core.resolveMessage(w.ctx(), behind, "cancel");
    core.recordAgentObservation(w.ctx(), pm.agentId, "idle");
  } finally {
    close(w);
  }
});

test("a retry restarts the notification clock", async () => {
  const w = await world();
  try {
    const { core, pm } = w;
    const message = send(w, pm, "again", w.developer.credential);
    w.advance(400);
    assert.equal(core.advanceMessaging(w.ctx(), timers).actions.length, 1);
    core.recordNotification(w.ctx(), message);
    core.pullMessage(w.ctx(pm.credential));
    w.advance(601);
    core.advanceMessaging(w.ctx(), timers);
    assert.equal(stateOf(w, message), "unacked");
    core.resolveMessage(w.ctx(), message, "retry");
    w.advance(100);
    assert.deepEqual(core.advanceMessaging(w.ctx(), timers).actions, []);
    w.advance(200);
    assert.deepEqual(core.advanceMessaging(w.ctx(), timers).actions, [
      { kind: "notify_operator", messageId: message, repeat: true },
    ]);
  } finally {
    close(w);
  }
});

test("stall timers skip a registered wait, including one that stays open, and resume when it returns", async () => {
  const w = await world();
  try {
    const { core, developer } = w;
    core.recordAgentObservation(w.ctx(), developer.agentId, "working");
    w.advance(899);
    assert.deepEqual(
      core.advanceMessaging(w.ctx(), timers).stalledAgentIds,
      [],
    );
    w.advance(1);
    assert.deepEqual(core.advanceMessaging(w.ctx(), timers).stalledAgentIds, [
      developer.agentId,
    ]);

    const { waitId } = core.beginWait(w.ctx(developer.credential));
    for (let step = 0; step < 50; step += 1) {
      w.advance(10_000);
      assert.deepEqual(
        core.advanceMessaging(w.ctx(), timers).stalledAgentIds,
        [],
      );
    }
    core.endWait(w.ctx(developer.credential), waitId);
    w.advance(899);
    assert.deepEqual(
      core.advanceMessaging(w.ctx(), timers).stalledAgentIds,
      [],
    );
    w.advance(1);
    assert.deepEqual(core.advanceMessaging(w.ctx(), timers).stalledAgentIds, [
      developer.agentId,
    ]);
  } finally {
    close(w);
  }
});

test("the controller may close a wait whose connection dropped; other agents may not", async () => {
  const w = await world();
  try {
    const { waitId } = w.core.beginWait(w.ctx(w.pm.credential));
    assertRejected(w, "not_wait_owner", () =>
      w.core.endWait(w.ctx(w.developer.credential), waitId),
    );
    assert.deepEqual(w.core.endWaitAsController(w.ctx(), waitId), {
      ended: true,
    });
    assert.deepEqual(w.core.endWaitAsController(w.ctx(), waitId), {
      ended: false,
    });
  } finally {
    close(w);
  }
});

test("a late ack moves an unacked message to acked_late and only the recipient may ack", async () => {
  const w = await world();
  try {
    const message = send(w, w.developer);
    w.core.recordSent(w.ctx(), message);
    w.advance(601);
    w.core.advanceMessaging(w.ctx(), timers);
    assert.equal(stateOf(w, message), "unacked");
    assertRejected(w, "not_recipient", () =>
      w.core.ackMessage(w.ctx(w.reviewer.credential), message),
    );
    w.core.ackMessage(w.ctx(w.developer.credential), message);
    assert.equal(stateOf(w, message), "acked_late");
  } finally {
    close(w);
  }
});

test("replacing a generation cancels every open message, closes waits and voids the old credential", async () => {
  const w = await world();
  try {
    const { core, developer, owner } = w;
    const queued = send(w, developer, "queued");
    const deferred = send(w, developer, "deferred");
    const sent = send(w, developer, "sent");
    const unacked = send(w, developer, "unacked");
    const done = send(w, developer, "done");
    core.recordSent(w.ctx(), queued);
    core.ackMessage(w.ctx(developer.credential), queued);
    core.recordDeferral(w.ctx(), deferred, "agent_busy");
    core.recordSent(w.ctx(), deferred);
    core.ackMessage(w.ctx(developer.credential), deferred);
    core.recordSent(w.ctx(), sent);
    w.advance(601);
    core.advanceMessaging(w.ctx(), timers);
    assert.equal(stateOf(w, sent), "unacked");
    const later = send(w, developer, "later");
    core.beginWait(w.ctx(developer.credential));
    const openWaits = (): number => {
      const db = new Database(path.join(w.stateDirectory, "controller.sqlite"));
      try {
        return (
          db
            .prepare(
              "SELECT COUNT(*) AS n FROM agent_waits WHERE agent_id = ? AND ended_at IS NULL",
            )
            .get(developer.agentId) as { n: number }
        ).n;
      } finally {
        db.close();
      }
    };
    assert.equal(openWaits(), 1);
    const context = w.ctx();
    const before = events(w, "message").length;
    const replaced = core.replaceAgentGeneration(context, developer.agentId);
    assert.equal(replaced.generation, 2);
    assert.equal(openWaits(), 0);
    assert.deepEqual(
      [...replaced.cancelledMessageIds].sort(),
      [later, sent, unacked, done].sort(),
    );
    for (const id of replaced.cancelledMessageIds) {
      assert.equal(stateOf(w, id), "cancelled");
      assert.equal(core.message(id)!.cancelReason, "generation_replaced");
    }
    assert.equal(stateOf(w, queued), "acked");
    assert.equal(stateOf(w, deferred), "acked");
    const batch = events(w, "message").slice(before);
    assert.equal(batch.length, replaced.cancelledMessageIds.length);
    const agentEvent = events(w, "agent").at(-1)!;
    assert.ok(
      batch.every((event) => event.state_version === agentEvent.state_version),
    );
    assert.ok(batch.every((event) => event.to_state === "cancelled"));
    const sequences = [
      ...batch.map((event) => event.sequence),
      agentEvent.sequence,
    ];
    assert.deepEqual(
      sequences,
      sequences.map((_, index) => sequences[0]! + index),
    );

    assert.throws(
      () => core.ackMessage(w.ctx(developer.credential), sent),
      AuthenticationError,
    );
    const fresh = replaced.credential;
    assertRejected(w, "illegal_transition", () =>
      core.ackMessage(w.ctx(fresh), sent),
    );
    assert.equal(
      core.agentRecord(developer.agentId)!.actorId,
      replaced.actorId,
    );
    const again = core.replaceAgentGeneration(context, developer.agentId);
    assert.deepEqual(again, replaced);
    const secondOperator = core.createActor(w.ctx(owner), {
      displayName: "second",
      role: "operator",
    });
    assert.throws(
      () =>
        core.replaceAgentGeneration(
          { ...context, credential: secondOperator.credential },
          developer.agentId,
        ),
      IdempotencyConflictError,
    );
    const stale = send(w, developer, "for generation two");
    core.recordSent(w.ctx(), stale);
    core.ackMessage(w.ctx(fresh), stale);
    assert.equal(stateOf(w, stale), "acked");
    assert.equal(core.message(stale)!.recipientGeneration, 2);
  } finally {
    close(w);
  }
});

for (const target of [
  "queued",
  "deferred",
  "sent",
  "unacked",
  "expired",
  "failed",
] as const)
  test(`replacing a generation cancels a ${target} message`, async () => {
    const w = await world();
    try {
      const { core, developer } = w;
      const message = send(w, developer);
      const behind = send(w, developer, "behind");
      if (target === "deferred")
        core.recordDeferral(w.ctx(), message, "agent_busy");
      if (target === "sent" || target === "unacked")
        core.recordSent(w.ctx(), message);
      if (target === "failed") core.recordFailure(w.ctx(), message, "gone");
      if (target === "expired") {
        core.recordDeferral(w.ctx(), message, "agent_blocked");
        w.advance(121);
        core.advanceMessaging(w.ctx(), timers);
      }
      if (target === "unacked") {
        w.advance(601);
        core.advanceMessaging(w.ctx(), timers);
      }
      assert.equal(stateOf(w, message), target);
      const replaced = core.replaceAgentGeneration(w.ctx(), developer.agentId);
      assert.deepEqual(
        [...replaced.cancelledMessageIds].sort(),
        [message, behind].sort(),
      );
      assert.equal(stateOf(w, message), "cancelled");
      assert.equal(core.message(message)!.cancelReason, "generation_replaced");
      assert.equal(stateOf(w, behind), "cancelled");
    } finally {
      close(w);
    }
  });

test("an ended agent cancels its messages, loses its actor and frees the seat", async () => {
  const w = await world();
  try {
    const { core, reviewer, owner } = w;
    const pending = send(w, reviewer);
    const { waitId } = core.beginWait(w.ctx(reviewer.credential));
    const result = core.endAgent(w.ctx(), reviewer.agentId);
    assert.deepEqual(result.cancelledMessageIds, [pending]);
    assert.equal(core.message(pending)!.cancelReason, "agent_ended");
    assert.equal(core.agentRecord(reviewer.agentId)!.state, "ended");
    assert.throws(
      () => core.beginWait(w.ctx(reviewer.credential)),
      AuthenticationError,
    );
    assert.deepEqual(core.endWaitAsController(w.ctx(), waitId), {
      ended: false,
    });
    assertRejected(w, "unknown_recipient", () =>
      core.enqueueMessage(w.ctx(), {
        recipientAgentId: reviewer.agentId,
        body: "late",
      }),
    );
    const actor = core.createActor(w.ctx(owner), {
      displayName: "reviewer 2",
      role: "Verifier",
      seatId: reviewer.seatId,
    });
    core.registerAgent(w.ctx(owner), {
      agentId: "reviewer-2",
      roleName: "reviewer",
      seatId: reviewer.seatId,
      actorId: actor.actorId,
    });
    assert.equal(core.agentRecord("reviewer-2")!.generation, 1);
  } finally {
    close(w);
  }
});

test("registerAgent refuses a mismatched, inactive or already bound actor, seat or role", async () => {
  const w = await world();
  try {
    const { core, owner, developer } = w;
    core.createSeat(w.ctx(owner), {
      seatId: "s2",
      name: "second",
      role: "Developer",
    });
    const good = core.createActor(w.ctx(owner), {
      displayName: "d2",
      role: "Developer",
      seatId: "s2",
    });
    const register = (
      over: Partial<{
        agentId: string;
        roleName: string;
        seatId: string;
        actorId: string;
      }>,
    ) =>
      core.registerAgent(w.ctx(owner), {
        agentId: "d2-agent",
        roleName: "developer",
        seatId: "s2",
        actorId: good.actorId,
        ...over,
      });
    assert.throws(
      () => register({ roleName: "reviewer" }),
      /seat must be active and match/,
    );
    assert.throws(
      () => register({ roleName: "unknown" }),
      /active configured role/,
    );
    assert.throws(
      () => register({ seatId: developer.seatId }),
      /attached to the agent seat|already bound/,
    );
    assert.throws(
      () => register({ actorId: developer.actorId }),
      /attached to the agent seat/,
    );
    assert.throws(() => register({ agentId: "bad id!" }), TypeError);
    core.revokeActor(w.ctx(owner), good.actorId);
    assert.throws(() => register({}), /active, of the role kind/);
    const second = core.createActor(w.ctx(owner), {
      displayName: "d3",
      role: "Developer",
      seatId: "s2",
    });
    core.registerAgent(w.ctx(owner), {
      agentId: "d3-agent",
      roleName: "developer",
      seatId: "s2",
      actorId: second.actorId,
    });
    assert.throws(
      () =>
        core.registerAgent(w.ctx(owner), {
          agentId: "d4-agent",
          roleName: "developer",
          seatId: "s2",
          actorId: second.actorId,
        }),
      MutationConflictError,
    );
  } finally {
    close(w);
  }
});

test("replacement and ending refuse while the agent seat has an active assignment", async () => {
  const w = await world();
  try {
    const { core, developer, owner } = w;
    core.createWorkItem(w.ctx(owner), {
      workItemId: "work-1",
      title: "A task",
      description: "Do it",
      requiredRole: "Developer",
    });
    core.markReady(w.ctx(owner), "work-1");
    core.assignWorkItem(w.ctx(owner), "work-1", developer.seatId);
    assert.throws(
      () => core.replaceAgentGeneration(w.ctx(), developer.agentId),
      MutationConflictError,
    );
    assert.throws(
      () => core.endAgent(w.ctx(), developer.agentId),
      MutationConflictError,
    );
    assert.equal(core.agentRecord(developer.agentId)!.generation, 1);
  } finally {
    close(w);
  }
});

test("a worker deferred on a non-empty input line logs the text, clears after the deferral and then sends once", async () => {
  const w = await world();
  try {
    const { core, developer } = w;
    const message = send(w, developer, "instruction");
    core.recordDeferral(w.ctx(), message, "input_not_empty");
    core.recordDeferral(w.ctx(), message, "input_not_empty");
    assert.equal(events(w, "message_note").length, 0);
    assert.deepEqual(core.advanceMessaging(w.ctx(), timers).actions, []);
    w.advance(120);
    const due = core.advanceMessaging(w.ctx(), timers);
    assert.deepEqual(due.actions, [
      { kind: "clear_then_send", messageId: message, notifyOperator: true },
    ]);
    core.recordInputClear(w.ctx(), message, "half typed by the operator");
    core.recordSent(w.ctx(), message);
    assert.equal(core.message(message)!.sendAttempts, 1);
    const db = new Database(path.join(w.stateDirectory, "controller.sqlite"));
    try {
      const rows = db
        .prepare("SELECT text, text_hash FROM message_input_clears")
        .all() as Array<{ text: string; text_hash: string }>;
      assert.deepEqual(
        rows.map((row) => row.text),
        ["half typed by the operator"],
      );
      const payloads = (
        db
          .prepare("SELECT payload_json FROM controller_events")
          .all() as Array<{ payload_json: string }>
      )
        .map((row) => row.payload_json)
        .join("\n");
      assert.ok(!payloads.includes("half typed"));
      assert.ok(!payloads.includes("instruction"));
    } finally {
      db.close();
    }
    assertRejected(w, "not_input_deferred", () =>
      core.recordInputClear(w.ctx(), message, "again"),
    );
  } finally {
    close(w);
  }
});

test("a changed deferral reason is a note, not a transition", async () => {
  const w = await world();
  try {
    const message = send(w, w.developer);
    w.core.recordDeferral(w.ctx(), message, "agent_busy");
    w.core.recordDeferral(w.ctx(), message, "agent_blocked");
    assert.equal(w.core.message(message)!.deferredReason, "agent_blocked");
    assert.equal(events(w, "message_note").length, 1);
    assert.equal(stateOf(w, message), "deferred");
  } finally {
    close(w);
  }
});

test("the PM pulls its head message once, workers cannot pull, and the inbox is read-only", async () => {
  const w = await world();
  try {
    const { core, pm, developer } = w;
    assert.deepEqual(core.pullMessage(w.ctx(pm.credential)), { message: null });
    const first = send(w, pm, "first", developer.credential);
    const second = send(w, pm, "second", developer.credential);
    assert.deepEqual(
      core.agentInbox(pm.credential).map((m) => m.messageId),
      [first],
    );
    assert.deepEqual(
      core.agentInbox(w.owner, pm.agentId).map((m) => m.messageId),
      [first],
    );
    assert.equal(stateOf(w, first), "queued");
    assert.equal(
      core.pullMessage(w.ctx(pm.credential)).message?.messageId,
      first,
    );
    assert.deepEqual(core.pullMessage(w.ctx(pm.credential)), { message: null });
    assert.deepEqual(
      core.agentInbox(pm.credential).map((m) => m.messageId),
      [first],
    );
    core.ackMessage(w.ctx(pm.credential), first);
    assert.equal(
      core.pullMessage(w.ctx(pm.credential)).message?.messageId,
      second,
    );
    assertRejected(w, "pull_not_allowed", () =>
      core.pullMessage(w.ctx(developer.credential)),
    );
    assert.throws(
      () => core.agentInbox(developer.credential, pm.agentId),
      AuthorizationError,
    );
  } finally {
    close(w);
  }
});

test("enqueue validates the body and needs the send capability; audit payloads never hold the body", async () => {
  const w = await world();
  try {
    const { core, developer } = w;
    for (const body of ["", "a\u0000b", "x".repeat(16 * 1024 + 1)])
      assert.throws(
        () =>
          core.enqueueMessage(w.ctx(), {
            recipientAgentId: developer.agentId,
            body,
          }),
        TypeError,
      );
    core.enqueueMessage(w.ctx(), {
      recipientAgentId: developer.agentId,
      body: "x".repeat(16 * 1024),
    });
    const secret = "SECRET-BODY-TEXT";
    send(w, developer, secret);
    const db = new Database(path.join(w.stateDirectory, "controller.sqlite"));
    try {
      const payloads = (
        db
          .prepare("SELECT payload_json FROM controller_events")
          .all() as Array<{ payload_json: string }>
      )
        .map((row) => row.payload_json)
        .join("\n");
      assert.ok(!payloads.includes(secret));
      assert.ok(payloads.includes("bodyHash"));
    } finally {
      db.close();
    }
    assert.throws(() => core.beginWait(w.ctx(w.owner)), AuthorizationError);
  } finally {
    close(w);
  }
});

test("advanceMessaging writes nothing when nothing is due and rolls back when the clock moved backwards", async () => {
  const w = await world();
  try {
    const { core, developer } = w;
    const message = send(w, developer);
    core.recordSent(w.ctx(), message);
    const version = core.stateVersion;
    assert.deepEqual(core.advanceMessaging(w.ctx(), timers), {
      applied: [],
      actions: [],
      stalledAgentIds: [],
    });
    assert.equal(core.stateVersion, version);
    w.advance(601);
    const context = w.ctx();
    w.clock.script = [w.clock.nowMs, w.clock.nowMs - 601_000];
    const result = core.advanceMessaging(context, timers);
    assert.deepEqual(result.applied, []);
    assert.equal(core.stateVersion, version);
    assert.equal(stateOf(w, message), "sent");
    assert.deepEqual(core.advanceMessaging(w.ctx(), timers).applied, [message]);
    assert.throws(
      () => core.advanceMessaging(w.ctx(developer.credential), timers),
      AuthorizationError,
    );
  } finally {
    close(w);
  }
});

test("agent observations are written only when the state changes", async () => {
  const w = await world();
  try {
    const { core, pm } = w;
    assert.deepEqual(
      core.recordAgentObservation(w.ctx(), pm.agentId, "working"),
      { recorded: true },
    );
    const version = core.stateVersion;
    assert.deepEqual(
      core.recordAgentObservation(w.ctx(), pm.agentId, "working"),
      { recorded: false },
    );
    assert.equal(core.stateVersion, version);
    assert.deepEqual(core.recordAgentObservation(w.ctx(), pm.agentId, "idle"), {
      recorded: true,
    });
    assert.throws(
      () =>
        core.recordAgentObservation(w.ctx(), pm.agentId, "sleeping" as never),
      TypeError,
    );
  } finally {
    close(w);
  }
});

test("migration 0015 leaves existing rows unchanged and gives existing actors the message capabilities", async () => {
  const w = await world();
  const { core, stateDirectory, info } = w;
  try {
    core.close();
    const databasePath = path.join(stateDirectory, "controller.sqlite");
    const db = new Database(databasePath);
    const newTables = [
      "message_input_clears",
      "message_rejections",
      "message_resolutions",
      "rounds",
      "messages",
      "agent_waits",
      "agent_state_history",
      "agents",
    ];
    const snapshot = (
      database: Database.Database,
    ): Record<string, string[]> => {
      const tables = (
        database
          .prepare(
            "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' AND name NOT IN ('schema_migrations') ORDER BY name",
          )
          .all() as Array<{ name: string }>
      )
        .map((row) => row.name)
        .filter((name) => !newTables.includes(name));
      return Object.fromEntries(
        tables.map((name) => [
          name,
          (database.prepare(`SELECT * FROM ${name}`).all() as unknown[])
            .map((row) => JSON.stringify(row))
            .filter((row) => !row.includes('"message:'))
            .sort(),
        ]),
      );
    };
    let before: Record<string, string[]>;
    let grantedBefore: number;
    try {
      const grants = () =>
        (
          db
            .prepare(
              "SELECT COUNT(*) AS n FROM capability_grants WHERE capability LIKE 'message:%'",
            )
            .get() as { n: number }
        ).n;
      grantedBefore = grants();
      assert.ok(grantedBefore > 0);
      db.pragma("foreign_keys = OFF");
      for (const table of newTables) db.exec(`DROP TABLE ${table}`);
      db.pragma("foreign_keys = ON");
      db.exec(
        "DELETE FROM capability_grants WHERE capability LIKE 'message:%'",
      );
      db.exec(
        "DELETE FROM role_capabilities WHERE capability LIKE 'message:%'",
      );
      db.exec("DELETE FROM schema_migrations WHERE version = 15");
      before = snapshot(db);
    } finally {
      db.close();
    }
    const reopened = await ControllerCore.open({
      stateDirectory,
      project: info,
    });
    try {
      reopened.close();
      const check = new Database(databasePath);
      try {
        assert.deepEqual(snapshot(check), before);
        assert.deepEqual(check.pragma("foreign_key_check"), []);
        assert.equal(
          (
            check
              .prepare(
                "SELECT COUNT(*) AS n FROM capability_grants WHERE capability LIKE 'message:%'",
              )
              .get() as { n: number }
          ).n,
          grantedBefore,
        );
        const byRole = check
          .prepare(
            `SELECT a.role AS role, group_concat(DISTINCT cg.capability) AS caps
             FROM actors a JOIN capability_grants cg ON cg.actor_id = a.actor_id AND cg.capability LIKE 'message:%'
             WHERE a.is_internal = 0 GROUP BY a.role ORDER BY a.role`,
          )
          .all() as Array<{ role: string; caps: string }>;
        const caps = Object.fromEntries(
          byRole.map((row) => [row.role, row.caps.split(",").sort()]),
        );
        assert.deepEqual(caps.PM, [
          "message:receive",
          "message:resolve",
          "message:send",
        ]);
        assert.deepEqual(caps.Developer, ["message:receive", "message:send"]);
        assert.deepEqual(caps.operator, ["message:resolve", "message:send"]);
        assert.equal(
          (
            check
              .prepare(
                "SELECT COUNT(*) AS n FROM capability_grants WHERE capability LIKE 'message:%' AND actor_id IN (SELECT actor_id FROM actors WHERE is_internal = 1)",
              )
              .get() as { n: number }
          ).n,
          0,
        );
        assert.ok(
          readdirSync(stateDirectory).some((entry) =>
            entry.startsWith("controller.sqlite.pre-v15-"),
          ),
        );
      } finally {
        check.close();
      }
      const again = await ControllerCore.open({
        stateDirectory,
        project: info,
      });
      try {
        assert.equal(again.roleDefinitions().length, 3);
      } finally {
        again.close();
      }
    } finally {
      // already closed above
    }
  } finally {
    rmSync(stateDirectory, { recursive: true, force: true });
  }
});
