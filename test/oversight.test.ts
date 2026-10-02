import assert from "node:assert/strict";
import { test } from "node:test";
import {
  MAX_PM_WAKES,
  type MessagingTimers,
} from "../src/controller/messaging.js";
import { close, ctx, harness, type Harness } from "./harness.js";

const TIMERS: MessagingTimers = {
  maxDeferralSeconds: 120,
  maxBusyDeferralSeconds: 3600,
  pmAckTimeoutSeconds: 600,
  pmNotifyAfterSeconds: 300,
  notifyIntervalSeconds: 600,
  stallAfterSeconds: 900,
  workerAckTimeoutSeconds: 600,
  pmWakeAfterSeconds: 20,
  pmWakeIntervalSeconds: 120,
};

interface World {
  readonly h: Harness;
  advance(seconds: number): void;
}

async function world(): Promise<World> {
  let now = Date.parse("2026-10-02T10:00:00.000Z");
  const h = await harness({ clock: () => new Date(now) });
  return {
    h,
    advance: (seconds) => {
      now += seconds * 1000;
    },
  };
}

const send = (w: World, to: string, body = "work"): string =>
  w.h.core.enqueueMessage(ctx(w.h.core, w.h.pm.credential), {
    recipientAgentId: to,
    body,
  }).messageId;

const messagesOf = (w: World, agentId: string) => w.h.core.messagesFor(agentId);

const notices = (w: World, prefix: string) =>
  messagesOf(w, w.h.pm.agentId).filter((m) => m.body.startsWith(prefix));

function deferLong(
  w: World,
  id: string,
  reason: "agent_busy" | "agent_blocked",
): void {
  w.h.core.recordDeferral(ctx(w.h.core, w.h.owner), id, reason);
}

test("a message that waits for a busy worker expires only after the busy limit, and the PM is told once with the command that unblocks the queue", async () => {
  const w = await world();
  try {
    const dev = w.h.developer.agentId;
    const id = send(w, dev, "write the parser for the feed");
    const behind = send(w, dev, "second");
    deferLong(w, id, "agent_busy");
    w.advance(TIMERS.maxDeferralSeconds + 1);
    const early = w.h.core.advanceMessaging(ctx(w.h.core, w.h.owner), TIMERS);
    assert.deepEqual(early.applied, [], "120 seconds of busy is not a failure");
    assert.equal(w.h.core.message(id)!.state, "deferred");
    assert.equal(notices(w, "Delivery problem").length, 0);
    w.advance(TIMERS.maxBusyDeferralSeconds);
    const late = w.h.core.advanceMessaging(ctx(w.h.core, w.h.owner), TIMERS);
    assert.deepEqual(late.applied, [id]);
    assert.equal(w.h.core.message(id)!.state, "expired");
    const [notice] = notices(w, "Delivery problem");
    assert.ok(notice);
    assert.equal(notice.state, "queued");
    assert.match(notice.body, new RegExp(`message ${id} to ${dev} is expired`));
    assert.match(notice.body, /write the parser for the feed/);
    assert.match(notice.body, new RegExp(`cstan resolve ${id} retry`));
    assert.match(notice.body, /skip.*cancel/);
    assert.equal(
      w.h.core.message(behind)!.state,
      "queued",
      "the queue behind it waits",
    );
    w.advance(600);
    w.h.core.advanceMessaging(ctx(w.h.core, w.h.owner), TIMERS);
    assert.equal(notices(w, "Delivery problem").length, 1, "told once");
  } finally {
    await close(w.h);
  }
});

test("an unacknowledged and a failed message to a worker each tell the PM once; a message to the PM does not", async () => {
  const w = await world();
  try {
    const owner = () => ctx(w.h.core, w.h.owner);
    const toPm = w.h.core.enqueueMessage(owner(), {
      recipientAgentId: w.h.pm.agentId,
      body: "for the pm",
    }).messageId;
    w.h.core.recordSent(owner(), toPm);
    const dev = w.h.developer.agentId;
    const sent = send(w, dev, "first");
    w.h.core.recordSent(owner(), sent);
    w.advance(TIMERS.workerAckTimeoutSeconds + 1);
    w.h.core.advanceMessaging(owner(), TIMERS);
    assert.equal(w.h.core.message(sent)!.state, "unacked");
    w.advance(TIMERS.pmAckTimeoutSeconds * 2);
    w.h.core.advanceMessaging(owner(), TIMERS);
    assert.equal(w.h.core.message(toPm)!.state, "unacked");
    const told = notices(w, "Delivery problem");
    assert.equal(
      told.length,
      1,
      "only the worker's message tells the PM; a PM message that goes unacked is the operator notification's business",
    );
    assert.match(told[0]!.body, /is unacked/);
    const other = w.h.addMember("developer2", "Developer");
    const failing = send(w, other.agentId, "will fail");
    w.h.core.recordFailure(owner(), failing, "Herdr failed: agent_not_found");
    const failed = notices(w, "Delivery problem").find((n) =>
      n.body.includes(failing),
    )!;
    assert.match(failed.body, /is failed \(Herdr failed: agent_not_found\)/);
    assert.equal(notices(w, "Delivery problem").length, 2);
  } finally {
    await close(w.h);
  }
});

test("a worker stalled or blocked for the stall time tells the PM once per episode, and the Supervisor is exempt", async () => {
  const w = await world();
  try {
    const dev = w.h.developer.agentId;
    const supervisor = w.h.addMember("supervisor", "Supervisor");
    w.h.core.recordAgentObservation(ctx(w.h.core, w.h.owner), dev, "working");
    w.h.core.recordAgentObservation(
      ctx(w.h.core, w.h.owner),
      supervisor.agentId,
      "working",
    );
    w.advance(TIMERS.stallAfterSeconds + 5);
    const first = w.h.core.advanceMessaging(ctx(w.h.core, w.h.owner), TIMERS);
    assert.deepEqual(
      first.attention.map((e) => [e.agentId, e.kind]),
      [[dev, "stalled"]],
      "the Supervisor never shows up",
    );
    assert.equal(
      w.h.core.queueAttentionNotices(ctx(w.h.core, w.h.owner), first.attention)
        .queued,
      1,
    );
    assert.equal(
      w.h.core.queueAttentionNotices(ctx(w.h.core, w.h.owner), first.attention)
        .queued,
      0,
    );
    const [stalled] = notices(w, "Agent stalled");
    assert.match(stalled!.body, new RegExp(`${dev} has shown no activity`));
    w.h.core.recordAgentObservation(ctx(w.h.core, w.h.owner), dev, "blocked");
    w.advance(TIMERS.stallAfterSeconds + 5);
    const second = w.h.core.advanceMessaging(ctx(w.h.core, w.h.owner), TIMERS);
    assert.deepEqual(
      second.attention.map((e) => [e.agentId, e.kind]),
      [[dev, "blocked"]],
    );
    assert.equal(
      w.h.core.queueAttentionNotices(ctx(w.h.core, w.h.owner), second.attention)
        .queued,
      1,
    );
    assert.equal(notices(w, "Agent blocked").length, 1);
  } finally {
    await close(w.h);
  }
});

test("an idle PM with an unread message gets a wake action after the wait, at most five times and never sooner than the interval; a busy PM, a read message or a disabled wake gets none", async () => {
  const w = await world();
  try {
    const pm = w.h.pm.agentId;
    w.h.core.recordAgentObservation(ctx(w.h.core, w.h.owner), pm, "idle");
    const id = w.h.core.enqueueMessage(ctx(w.h.core, w.h.owner), {
      recipientAgentId: pm,
      body: "report",
    }).messageId;
    const actions = (timers: MessagingTimers = TIMERS) =>
      w.h.core
        .advanceMessaging(ctx(w.h.core, w.h.owner), timers)
        .actions.map((a) => a.kind)
        .filter((kind) => kind === "wake_pm");
    assert.deepEqual(actions(), [], "not before the wait");
    w.advance(21);
    assert.deepEqual(
      actions({ ...TIMERS, pmWakeAfterSeconds: 0 }),
      [],
      "0 turns it off",
    );
    assert.deepEqual(actions(), ["wake_pm"]);
    for (let wake = 1; wake <= MAX_PM_WAKES; wake += 1) {
      assert.equal(
        w.h.core.recordPmWake(ctx(w.h.core, w.h.owner), id).wakes,
        wake,
      );
      assert.deepEqual(actions(), [], "not within the interval");
      w.advance(TIMERS.pmWakeIntervalSeconds + 1);
      if (wake < MAX_PM_WAKES) assert.deepEqual(actions(), ["wake_pm"]);
    }
    assert.deepEqual(actions(), [], `no more than ${MAX_PM_WAKES} wakes`);
  } finally {
    await close(w.h);
  }
});

test("a PM that is working, blocked or unobserved is not woken, and a message the PM has read is not either", async () => {
  const w = await world();
  try {
    const pm = w.h.pm.agentId;
    const id = w.h.core.enqueueMessage(ctx(w.h.core, w.h.owner), {
      recipientAgentId: pm,
      body: "report",
    }).messageId;
    w.advance(60);
    const wake = () =>
      w.h.core
        .advanceMessaging(ctx(w.h.core, w.h.owner), TIMERS)
        .actions.some((a) => a.kind === "wake_pm");
    assert.equal(wake(), false, "no observation yet");
    for (const state of ["working", "blocked", "unknown"] as const) {
      w.h.core.recordAgentObservation(ctx(w.h.core, w.h.owner), pm, state);
      assert.equal(wake(), false, state);
    }
    w.h.core.recordAgentObservation(ctx(w.h.core, w.h.owner), pm, "done");
    assert.equal(wake(), true, "done counts as idle");
    w.h.core.recordSent(ctx(w.h.core, w.h.owner), id);
    assert.equal(wake(), false, "the PM already read it");
    assert.throws(
      () => w.h.core.recordPmWake(ctx(w.h.core, w.h.owner), id),
      /no longer waiting/,
    );
  } finally {
    await close(w.h);
  }
});

test("the routine check is queued for the one Supervisor only while a worker is active, once per interval, and an open one is replaced, never blocking", async () => {
  const w = await world();
  try {
    const owner = () => ctx(w.h.core, w.h.owner);
    assert.deepEqual(
      w.h.core.queueSupervisionCheck(owner(), 300),
      { queued: false, cancelled: 0 },
      "no Supervisor",
    );
    const supervisor = w.h.addMember("supervisor", "Supervisor");
    const events = w.h.core.stateVersion;
    assert.equal(w.h.core.queueSupervisionCheck(owner(), 300).queued, true);
    const checks = () =>
      messagesOf(w, supervisor.agentId).filter((m) =>
        m.body.startsWith("Routine check"),
      );
    assert.equal(checks().length, 1);
    assert.match(checks()[0]!.body, /cstan observe each active worker/);
    assert.ok(w.h.core.stateVersion > events);
    const version = w.h.core.stateVersion;
    w.advance(100);
    assert.deepEqual(w.h.core.queueSupervisionCheck(owner(), 300), {
      queued: false,
      cancelled: 0,
    });
    assert.equal(
      w.h.core.stateVersion,
      version,
      "nothing is written while no check is due",
    );
    w.advance(250);
    const second = w.h.core.queueSupervisionCheck(owner(), 300);
    assert.deepEqual(second, { queued: true, cancelled: 1 });
    const all = checks();
    assert.equal(all.length, 2);
    assert.equal(all[0]!.state, "cancelled");
    assert.equal(all[0]!.stateReason, "superseded_check");
    assert.equal(all[1]!.state, "queued");
    w.h.core.recordSent(owner(), all[1]!.messageId);
    w.advance(TIMERS.workerAckTimeoutSeconds + 1);
    w.h.core.advanceMessaging(owner(), TIMERS);
    assert.equal(w.h.core.message(all[1]!.messageId)!.state, "unacked");
    assert.equal(
      notices(w, "Delivery problem").length,
      0,
      "a routine check never tells the PM",
    );
    w.advance(300);
    assert.equal(w.h.core.queueSupervisionCheck(owner(), 300).cancelled, 1);
    assert.throws(() => w.h.core.queueSupervisionCheck(owner(), 0), TypeError);
  } finally {
    await close(w.h);
  }
});

test("a message retried after a delivery problem tells the PM again when it goes wrong again, and a blocked worker's episode does not change while it stays blocked", async () => {
  const w = await world();
  try {
    const owner = () => ctx(w.h.core, w.h.owner);
    const dev = w.h.developer.agentId;
    const id = send(w, dev, "retry me");
    w.h.core.recordSent(owner(), id);
    w.advance(TIMERS.workerAckTimeoutSeconds + 1);
    w.h.core.advanceMessaging(owner(), TIMERS);
    assert.equal(notices(w, "Delivery problem").length, 1);
    w.h.core.resolveMessage(ctx(w.h.core, w.h.pm.credential), id, "retry");
    w.h.core.recordSent(owner(), id);
    w.advance(TIMERS.workerAckTimeoutSeconds + 1);
    w.h.core.advanceMessaging(owner(), TIMERS);
    assert.equal(
      notices(w, "Delivery problem").length,
      2,
      "the second failure of the same message is told again",
    );
    w.h.core.recordAgentObservation(owner(), dev, "blocked");
    w.advance(TIMERS.stallAfterSeconds + 5);
    const first = w.h.core.advanceMessaging(owner(), TIMERS).attention;
    w.advance(300);
    const later = w.h.core.advanceMessaging(owner(), TIMERS).attention;
    assert.equal(first[0]!.episodeMs, later[0]!.episodeMs);
  } finally {
    await close(w.h);
  }
});

test("a stuck worker message with no notice (the PM was away, or it predates the notices) is told once the PM is there, and never twice", async () => {
  const w = await world();
  try {
    const owner = () => ctx(w.h.core, w.h.owner);
    const dev = w.h.developer.agentId;
    w.h.core.endAgent(owner(), w.h.pm.agentId);
    const id = w.h.core.enqueueMessage(owner(), {
      recipientAgentId: dev,
      body: "went wrong while no PM was active",
    }).messageId;
    w.h.core.recordSent(owner(), id);
    w.advance(TIMERS.workerAckTimeoutSeconds + 1);
    w.h.core.advanceMessaging(owner(), TIMERS);
    assert.equal(w.h.core.message(id)!.state, "unacked");
    assert.deepEqual(
      w.h.core.queueMissingDeliveryNotices(owner()),
      { queued: 0 },
      "no PM yet",
    );
    const pm = w.h.addMember("pm2", "PM");
    const version = w.h.core.stateVersion;
    assert.deepEqual(w.h.core.queueMissingDeliveryNotices(owner()), {
      queued: 1,
    });
    const told = messagesOf(w, pm.agentId).filter((m) =>
      m.body.startsWith("Delivery problem"),
    );
    assert.equal(told.length, 1);
    assert.match(told[0]!.body, new RegExp(id));
    const after = w.h.core.stateVersion;
    assert.ok(after > version);
    assert.deepEqual(w.h.core.queueMissingDeliveryNotices(owner()), {
      queued: 0,
    });
    assert.equal(
      w.h.core.stateVersion,
      after,
      "nothing is written when nothing is missing",
    );
  } finally {
    await close(w.h);
  }
});
