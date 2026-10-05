import assert from "node:assert/strict";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { test } from "node:test";
import { openSqlite } from "../src/controller/sqlite.js";
import {
  DeliveryDriver,
  FAILURE_LIMIT,
  PROCESS_SAMPLE_MS,
  STUCK_AFTER_TICKS,
  type DriverAdapter,
} from "../src/driver.js";
import {
  HerdrProcessProbe,
  parseProcStat,
  type ProcessActivityProbe,
  type ProcessSample,
} from "../src/herdr/process-activity.js";
import { loadCapstanConfig } from "../src/config/capstan-config.js";
import { MAX_INPUT_CLEAR_BYTES } from "../src/controller/core.js";
import type { HerdrState } from "../src/controller/messaging.js";
import type { MessagingTimers } from "../src/controller/messaging.js";
import {
  AgentPaneMismatch,
  ClearFailed,
  InputUnreadable,
  InvalidArgumentError,
  MAX_TEXT_BYTES,
  NotIdle,
  PhaseError,
  SendAfterRecordError,
  UnknownPaneError,
  type KeyLogger,
  type SendOutcome,
} from "../src/herdr/adapter.js";
import { HerdrError } from "../src/herdr/runner.js";
import type {
  ChannelResult,
  NotificationRequest,
  Notifier,
} from "../src/notifier.js";
import { close, ctx, harness, type Harness } from "./harness.js";

const TIMERS: MessagingTimers = {
  maxDeferralSeconds: 120,
  maxBusyDeferralSeconds: 120,
  pmAckTimeoutSeconds: 600,
  pmNotifyAfterSeconds: 300,
  notifyIntervalSeconds: 600,
  stallAfterSeconds: 900,
  workerAckTimeoutSeconds: 600,
  pmWakeAfterSeconds: 0,
  pmWakeIntervalSeconds: 120,
};

interface Typed {
  paneId: string;
  text: string;
}

class StubAdapter implements DriverAdapter {
  readonly panes = new Map<string, string>();
  readonly entries = new Map<string, { agent?: string }>();
  readonly states = new Map<string, HerdrState | Error>();
  readonly typed: Typed[] = [];
  sends = 0;
  clears = 0;
  sendImpl:
    | ((input: {
        paneId: string;
        text: string;
        beforeSend: () => void | Promise<void>;
      }) => Promise<SendOutcome>)
    | undefined;
  clearImpl:
    | ((input: {
        paneId: string;
        deferredForMs: number;
        maxDeferralMs: number;
        discard: (text: string) => void | Promise<void>;
        log: KeyLogger;
      }) => Promise<{ cleared: boolean; text: string }>)
    | undefined;
  observeGate: Promise<void> | undefined;

  register(agentId: string, paneId = `w1:${agentId}`): void {
    this.panes.set(agentId, paneId);
    this.entries.set(paneId, { agent: agentId });
  }

  paneForAgent(agentId: string): string | undefined {
    return this.panes.get(agentId);
  }

  paneEntry(paneId: string): { agent?: string } | undefined {
    return this.entries.get(paneId);
  }

  async agentObservation(agentId: string): Promise<HerdrState> {
    await this.observeGate;
    const state = this.states.get(agentId) ?? "idle";
    if (state instanceof Error) throw state;
    return state;
  }

  async guardedSend(input: {
    paneId: string;
    text: string;
    beforeSend: () => void | Promise<void>;
  }): Promise<SendOutcome> {
    this.sends += 1;
    if (this.sendImpl) return this.sendImpl(input);
    await input.beforeSend();
    this.typed.push({ paneId: input.paneId, text: input.text });
    return { sent: true };
  }

  readonly wakes: Typed[] = [];
  wakeOutcome: "sent" | "pm_not_idle" | "input_not_empty" = "sent";
  /** Makes the wake throw after beforeSend recorded it. */
  wakeThrowsAfterRecord = false;
  async wakePm(input: {
    paneId: string;
    text: string;
    beforeSend: () => void | Promise<void>;
  }) {
    if (this.wakeOutcome !== "sent")
      return { sent: false as const, reason: this.wakeOutcome };
    await input.beforeSend();
    if (this.wakeThrowsAfterRecord) throw new Error("prompt failed");
    this.wakes.push({ paneId: input.paneId, text: input.text });
    return { sent: true as const };
  }
  async clearAfterDeferral(input: {
    paneId: string;
    deferredForMs: number;
    maxDeferralMs: number;
    discard: (text: string) => void | Promise<void>;
    log: KeyLogger;
  }): Promise<{ cleared: boolean; text: string }> {
    this.clears += 1;
    if (this.clearImpl) return this.clearImpl(input);
    await input.discard("typed by someone");
    return { cleared: true, text: "typed by someone" };
  }
}

class StubNotifier implements Notifier {
  readonly sent: NotificationRequest[] = [];
  readonly written: Array<{
    request: NotificationRequest;
    results: readonly ChannelResult[];
    recorded: boolean;
  }> = [];
  results: readonly ChannelResult[] = [
    { channel: "herdr", ok: true },
    { channel: "fallback", ok: true },
  ];
  onSend: (() => void) | undefined;

  async send(request: NotificationRequest): Promise<readonly ChannelResult[]> {
    this.sent.push(request);
    this.onSend?.();
    return this.results;
  }

  write(
    request: NotificationRequest,
    results: readonly ChannelResult[],
    recorded: boolean,
  ): void {
    this.written.push({ request, results, recorded });
  }
}

interface World {
  h: Harness;
  adapter: StubAdapter;
  notifier: StubNotifier;
  driver: DeliveryDriver;
  events: Array<{ event: string; details: Record<string, unknown> }>;
  clock: { now: number };
  tick(): Promise<void>;
  advance(ms: number): void;
  eventNames(): string[];
}

async function world(
  timers: MessagingTimers = TIMERS,
  processProbe?: ProcessActivityProbe,
  pmStaleSeconds?: number,
): Promise<World> {
  const clock = { now: Date.parse("2026-01-01T00:00:00.000Z") };
  const h = await harness({ clock: () => new Date(clock.now) });
  const adapter = new StubAdapter();
  adapter.register(h.developer.agentId);
  const notifier = new StubNotifier();
  const events: World["events"] = [];
  const driver = new DeliveryDriver({
    core: h.core,
    adapter,
    timers,
    notifier,
    credential: h.owner,
    now: () => clock.now,
    log: (event, details) => events.push({ event, details }),
    ...(processProbe === undefined ? {} : { processProbe }),
    ...(pmStaleSeconds === undefined ? {} : { pmStaleSeconds }),
  });
  return {
    h,
    adapter,
    notifier,
    driver,
    events,
    clock,
    tick: () => driver.tick(),
    advance: (ms) => {
      clock.now += ms;
    },
    eventNames: () => events.map((e) => e.event),
  };
}

function queue(
  w: World,
  to: string,
  body = "please do the thing",
  from = w.h.pm.credential,
): string {
  return w.h.core.enqueueMessage(ctx(w.h.core, from), {
    recipientAgentId: to,
    body,
  }).messageId;
}

function state(w: World, id: string): string {
  return w.h.core.message(id)!.state;
}

function rejections(w: World): number {
  return w.h.core.messageRejections().length;
}

test("the driver delivers a worker's queue head with a frame, records before typing, and never touches a PM", async () => {
  const w = await world();
  try {
    const first = queue(w, w.h.developer.agentId, "first");
    const second = queue(w, w.h.developer.agentId, "second");
    const toPm = w.h.core.enqueueMessage(ctx(w.h.core, w.h.owner), {
      recipientAgentId: w.h.pm.agentId,
      body: "for the PM",
    }).messageId;
    w.adapter.register(w.h.pm.agentId);
    await w.tick();
    assert.equal(w.adapter.typed.length, 1, "only the head is typed");
    assert.equal(
      w.adapter.typed[0]!.text,
      `[capstan message ${first} from pm (${w.h.pm.agentId})]\nfirst\nAcknowledge with: cstan ack ${first}`,
    );
    assert.equal(state(w, first), "sent");
    assert.equal(state(w, second), "queued");
    assert.equal(state(w, toPm), "queued", "a PM message is never pushed");
    assert.equal(
      w.adapter.typed.some(
        (t) => t.paneId === w.adapter.panes.get(w.h.pm.agentId),
      ),
      false,
    );
    await w.tick();
    assert.equal(
      w.adapter.typed.length,
      1,
      "the second waits for the first's ack",
    );
    w.h.core.ackMessage(ctx(w.h.core, w.h.developer.credential), first);
    await w.tick();
    assert.equal(w.adapter.typed.length, 2);
    assert.match(w.adapter.typed[1]!.text, /^\[capstan message .* from pm/);
    assert.equal(state(w, second), "sent");
  } finally {
    await close(w.h);
  }
});

test("an operator message is framed as coming from the operator", async () => {
  const w = await world();
  try {
    const id = queue(w, w.h.developer.agentId, "from me", w.h.owner);
    await w.tick();
    assert.equal(
      w.adapter.typed[0]!.text.split("\n")[0],
      `[capstan message ${id} from operator]`,
    );
  } finally {
    await close(w.h);
  }
});

test("a deferral is recorded with its reason, once, and the message stays the head", async () => {
  const w = await world();
  try {
    const id = queue(w, w.h.developer.agentId);
    w.adapter.sendImpl = async () => ({ sent: false, reason: "agent_busy" });
    await w.tick();
    await w.tick();
    const message = w.h.core.message(id)!;
    assert.equal(message.state, "deferred");
    assert.equal(message.deferredReason, "agent_busy");
    assert.equal(w.adapter.typed.length, 0);
    assert.equal(rejections(w), 0);
    w.adapter.sendImpl = undefined;
    await w.tick();
    assert.equal(
      state(w, id),
      "sent",
      "a deferred head is delivered when the agent is ready",
    );
  } finally {
    await close(w.h);
  }
});

test("a message that moved before the record is never typed and leaves no rejection row", async () => {
  const w = await world();
  try {
    const id = queue(w, w.h.developer.agentId);
    w.adapter.sendImpl = async (input) => {
      w.h.core.resolveMessage(ctx(w.h.core, w.h.owner), id, "cancel");
      await input.beforeSend();
      w.adapter.typed.push({ paneId: input.paneId, text: input.text });
      return { sent: true };
    };
    await w.tick();
    assert.equal(
      w.adapter.typed.length,
      0,
      "nothing typed for a cancelled message",
    );
    assert.equal(state(w, id), "cancelled");
    assert.equal(rejections(w), 0);
    assert.ok(w.eventNames().includes("stale_action"));
    assert.equal(w.eventNames().includes("delivery_failure"), false);
  } finally {
    await close(w.h);
  }
});

test("a prompt Herdr refuses after the record leaves the message sent, and a retry is the operator's call", async () => {
  const w = await world();
  try {
    const id = queue(w, w.h.developer.agentId);
    w.adapter.sendImpl = async (input) => {
      await input.beforeSend();
      throw new SendAfterRecordError("lost");
    };
    await w.tick();
    assert.equal(state(w, id), "sent");
    assert.ok(w.eventNames().includes("send_after_record"));
    w.adapter.sendImpl = undefined;
    await w.tick();
    assert.equal(w.adapter.typed.length, 0, "no automatic resend");
    w.h.core.resolveMessage(ctx(w.h.core, w.h.owner), id, "retry");
    await w.tick();
    assert.equal(w.adapter.typed.length, 1);
  } finally {
    await close(w.h);
  }
});

test("refused text fails the message; real Herdr errors fail it after the limit; a result resets the count; no pane never fails it", async () => {
  const w = await world();
  try {
    const refused = queue(w, w.h.developer.agentId, "one");
    w.adapter.sendImpl = async () => {
      throw new InvalidArgumentError("refused");
    };
    await w.tick();
    assert.equal(state(w, refused), "failed");

    const flaky = queue(w, w.h.developer.agentId, "two", w.h.pm.credential);
    w.h.core.resolveMessage(ctx(w.h.core, w.h.owner), refused, "cancel");
    w.adapter.sendImpl = async () => {
      throw new HerdrError("agent_gone", "gone");
    };
    for (let i = 0; i < FAILURE_LIMIT - 1; i += 1) await w.tick();
    assert.equal(state(w, flaky), "queued");
    w.adapter.sendImpl = async () => ({ sent: false, reason: "agent_busy" });
    await w.tick();
    assert.equal(state(w, flaky), "deferred", "a result resets the count");
    w.adapter.sendImpl = async () => {
      throw new HerdrError("agent_gone", "gone");
    };
    for (let i = 0; i < FAILURE_LIMIT - 1; i += 1) await w.tick();
    assert.equal(state(w, flaky), "deferred");
    await w.tick();
    assert.equal(state(w, flaky), "failed");

    const lonely = w.h.addMember("developer2", "Developer");
    const stranded = queue(w, lonely.agentId, "no pane", w.h.owner);
    for (let i = 0; i < 20; i += 1) await w.tick();
    assert.equal(state(w, stranded), "queued");
    assert.equal(
      w.events.filter((e) => e.event === "no_pane").length,
      1,
      "logged once",
    );
    assert.deepEqual(w.driver.snapshot().stuck, [], "no_pane is log-only");
    assert.equal(
      w.notifier.sent.filter((n) => n.kind === "delivery_stuck").length,
      0,
    );
  } finally {
    await close(w.h);
  }
});

test("a pane owned by another agent, a tainted pane and a bad agent name are skipped, never failed, and become stuck after ten ticks", async () => {
  const w = await world();
  try {
    const id = queue(w, w.h.developer.agentId);
    w.adapter.entries.set(w.adapter.panes.get(w.h.developer.agentId)!, {
      agent: "someone-else",
    });
    for (let i = 0; i < STUCK_AFTER_TICKS - 1; i += 1) await w.tick();
    assert.deepEqual(w.driver.snapshot().stuck, []);
    await w.tick();
    assert.deepEqual(w.driver.snapshot().stuck, [
      { messageId: id, reason: "pane_mismatch" },
    ]);
    assert.equal(w.adapter.sends, 0, "nothing typed on the wrong pane");
    assert.equal(state(w, id), "queued");
    const stuckNotes = w.notifier.sent.filter(
      (n) => n.kind === "delivery_stuck",
    );
    assert.equal(stuckNotes.length, 1);
    assert.equal(stuckNotes[0]!.detail, "pane_mismatch");
    await w.tick();
    assert.equal(
      w.notifier.sent.filter((n) => n.kind === "delivery_stuck").length,
      1,
      "once per message and reason",
    );

    w.adapter.entries.set(w.adapter.panes.get(w.h.developer.agentId)!, {
      agent: w.h.developer.agentId,
    });
    w.adapter.sendImpl = async () => {
      throw new PhaseError("tainted");
    };
    for (let i = 0; i < STUCK_AFTER_TICKS; i += 1) await w.tick();
    assert.deepEqual(w.driver.snapshot().stuck, [
      { messageId: id, reason: "pane_not_ready" },
    ]);
    assert.equal(state(w, id), "queued", "PhaseError never fails a message");

    w.adapter.sendImpl = async () => {
      throw new AgentPaneMismatch("moved");
    };
    await w.tick();
    assert.equal(state(w, id), "queued");
    w.adapter.sendImpl = async () => {
      throw new UnknownPaneError("gone");
    };
    await w.tick();
    assert.equal(state(w, id), "queued");
    w.adapter.sendImpl = undefined;
    await w.tick();
    assert.equal(state(w, id), "sent", "recovers when the pane is right");
    assert.deepEqual(w.driver.snapshot().stuck, []);
  } finally {
    await close(w.h);
  }
});

test("a recurrence of a stuck condition is reported again after it cleared in between", async () => {
  const w = await world();
  try {
    const id = queue(w, w.h.developer.agentId);
    const pane = w.adapter.panes.get(w.h.developer.agentId)!;
    const breakPane = (): void =>
      void w.adapter.entries.set(pane, { agent: "other" });
    const fixPane = (): void =>
      void w.adapter.entries.set(pane, { agent: w.h.developer.agentId });
    const stuckNotes = (): number =>
      w.notifier.sent.filter((n) => n.kind === "delivery_stuck").length;

    breakPane();
    for (let i = 0; i < STUCK_AFTER_TICKS; i += 1) await w.tick();
    assert.equal(stuckNotes(), 1);

    fixPane();
    w.adapter.sendImpl = async () => ({ sent: false, reason: "agent_busy" });
    await w.tick();
    assert.deepEqual(w.driver.snapshot().stuck, [], "the condition cleared");
    assert.equal(state(w, id), "deferred");

    breakPane();
    for (let i = 0; i < STUCK_AFTER_TICKS; i += 1) await w.tick();
    assert.equal(stuckNotes(), 2, "a second episode is reported again");
  } finally {
    await close(w.h);
  }
});

test("an over-long frame is failed without touching the adapter, even for an agent with no pane", async () => {
  const w = await world();
  try {
    const lonely = w.h.addMember("developer2", "Developer");
    const id = w.h.core.enqueueMessage(ctx(w.h.core, w.h.owner), {
      recipientAgentId: lonely.agentId,
      body: "a".repeat(MAX_TEXT_BYTES - 50),
    }).messageId;
    await w.tick();
    assert.equal(state(w, id), "failed");
    assert.equal(w.adapter.sends, 0);
  } finally {
    await close(w.h);
  }
});

test("an agent id the adapter cannot name is skipped and logged once", async () => {
  const w = await world();
  try {
    const core = w.h.core;
    core.createSeat(ctx(core, w.h.owner), {
      seatId: "odd-seat",
      name: "developer2",
      role: "Developer",
    });
    const actor = core.createActor(ctx(core, w.h.owner), {
      displayName: "odd",
      role: "Developer",
      seatId: "odd-seat",
    });
    // A colon is legal in a ledger id but not in a Herdr agent name.
    core.registerAgent(ctx(core, w.h.owner), {
      agentId: "odd:agent",
      roleName: "developer2",
      seatId: "odd-seat",
      actorId: actor.actorId,
    });
    w.adapter.register("odd:agent");
    const id = queue(w, "odd:agent", "hello", w.h.owner);
    await w.tick();
    await w.tick();
    assert.equal(
      w.adapter.sends,
      0,
      "nothing is typed for a name Herdr cannot use",
    );
    assert.equal(state(w, id), "queued");
    assert.equal(
      w.events.filter((e) => e.event === "bad_agent_name").length,
      1,
    );
  } finally {
    await close(w.h);
  }
});

test("observations are recorded through the adapter, a mismatch is logged once, and an agent with no pane is never observed", async () => {
  const w = await world();
  try {
    w.adapter.states.set(w.h.developer.agentId, "working");
    await w.tick();
    await w.tick();
    const db = openSqlite(path.join(w.h.stateDirectory, "controller.sqlite"), {
      readOnly: true,
    });
    try {
      const rows = db
        .prepare(
          "SELECT herdr_state FROM agent_state_history WHERE agent_id = ? ORDER BY sequence",
        )
        .all(w.h.developer.agentId) as Array<{ herdr_state: string }>;
      assert.equal(
        rows.filter((r) => r.herdr_state === "working").length,
        1,
        "a state is written only when it changes",
      );
      const pmRows = db
        .prepare(
          "SELECT COUNT(*) AS n FROM agent_state_history WHERE agent_id = ?",
        )
        .get(w.h.pm.agentId) as { n: number };
      assert.equal(pmRows.n, 0, "an agent with no pane is never observed");
    } finally {
      db.close();
    }
    const later = w.adapter.states;
    later.set(w.h.developer.agentId, new AgentPaneMismatch("moved"));
    await w.tick();
    await w.tick();
    assert.equal(w.events.filter((e) => e.event === "pane_mismatch").length, 1);
    later.set(w.h.developer.agentId, new HerdrError("agent_not_found", "gone"));
    await w.tick();
    await w.tick();
    assert.equal(
      w.events.filter((e) => e.event === "observe_failed").length,
      1,
    );
  } finally {
    await close(w.h);
  }
});

test("a stalled working agent is logged once and shown in the snapshot", async () => {
  const w = await world();
  try {
    w.adapter.states.set(w.h.developer.agentId, "working");
    await w.tick();
    w.advance((TIMERS.stallAfterSeconds + 1) * 1000);
    await w.tick();
    await w.tick();
    assert.deepEqual(w.driver.snapshot().stalledAgentIds, [
      w.h.developer.agentId,
    ]);
    assert.equal(w.events.filter((e) => e.event === "agent_stalled").length, 1);
  } finally {
    await close(w.h);
  }
});

test("a text left in the input line is cleared after the maximum deferral: recorded first, keys logged, operator told, then sent", async () => {
  const w = await world();
  try {
    const id = queue(w, w.h.developer.agentId);
    w.adapter.sendImpl = async () => ({
      sent: false,
      reason: "input_not_empty",
    });
    await w.tick();
    assert.equal(state(w, id), "deferred");
    const keys: unknown[] = [];
    w.adapter.clearImpl = async (input) => {
      assert.equal(input.maxDeferralMs, TIMERS.maxDeferralSeconds * 1000);
      assert.ok(input.deferredForMs >= input.maxDeferralMs);
      await input.discard("half typed text");
      await input.log({
        kind: "key",
        pane: input.paneId,
        key: "ctrl+u",
        reason: "clear",
      });
      keys.push("ctrl+u");
      return { cleared: true, text: "half typed text" };
    };
    await w.tick();
    assert.equal(w.adapter.clears, 0, "not before the maximum deferral");
    w.advance((TIMERS.maxDeferralSeconds + 1) * 1000);
    w.adapter.sendImpl = undefined;
    await w.tick();
    assert.equal(w.adapter.clears, 1);
    assert.equal(w.h.core.inputClears(w.h.owner, 10).length, 1);
    assert.equal(
      w.notifier.sent.filter((n) => n.kind === "input_cleared").length,
      1,
    );
    assert.equal(
      w.notifier.sent.find((n) => n.kind === "input_cleared")!.detail,
      String("half typed text".length),
    );
    const written = w.notifier.written.find(
      (n) => n.request.kind === "input_cleared",
    )!;
    assert.equal(written.recorded, true);
    assert.equal(state(w, id), "sent", "sent in the same tick after the clear");
    assert.equal(w.adapter.typed.length, 1);
    assert.equal(keys.length, 1);
    assert.ok(w.events.some((e) => e.event === "key"));
    assert.equal(
      w.h.core.message(id)!.lastNotifiedAt,
      null,
      "an input_cleared notice never moves a lastNotifiedAt",
    );
  } finally {
    await close(w.h);
  }
});

test("clear edge cases: an empty line records nothing, stale discards stop the keys, over-long text is truncated, and errors skip the agent", async () => {
  const w = await world();
  try {
    const id = queue(w, w.h.developer.agentId);
    w.adapter.sendImpl = async () => ({
      sent: false,
      reason: "input_not_empty",
    });
    await w.tick();
    w.adapter.sendImpl = async () => ({
      sent: false,
      reason: "input_not_empty",
    });
    w.advance((TIMERS.maxDeferralSeconds + 1) * 1000);

    w.adapter.clearImpl = async () => ({ cleared: false, text: "" });
    await w.tick();
    assert.equal(w.h.core.inputClears(w.h.owner, 10).length, 0);
    assert.equal(w.notifier.sent.length, 0);

    w.adapter.clearImpl = async (input) => {
      w.h.core.resolveMessage(ctx(w.h.core, w.h.owner), id, "cancel");
      await input.discard("stale");
      throw new Error("keys must not follow a stale discard");
    };
    await w.tick();
    assert.equal(state(w, id), "cancelled");
    assert.ok(w.eventNames().includes("stale_action"));
    assert.equal(w.h.core.inputClears(w.h.owner, 10).length, 0);
    assert.equal(rejections(w), 0);

    const next = queue(w, w.h.developer.agentId, "again");
    w.adapter.sendImpl = async () => ({
      sent: false,
      reason: "input_not_empty",
    });
    await w.tick();
    w.advance((TIMERS.maxDeferralSeconds + 1) * 1000);
    w.adapter.clearImpl = async (input) => {
      await input.discard("é".repeat(MAX_INPUT_CLEAR_BYTES));
      return { cleared: true, text: "é" };
    };
    await w.tick();
    const db = openSqlite(path.join(w.h.stateDirectory, "controller.sqlite"), {
      readOnly: true,
    });
    try {
      const row = db
        .prepare("SELECT text FROM message_input_clears WHERE message_id = ?")
        .get(next) as { text: string };
      assert.ok(Buffer.byteLength(row.text, "utf8") <= MAX_INPUT_CLEAR_BYTES);
      assert.ok(row.text.endsWith("[truncated]"));
      assert.ok(row.text.isWellFormed());
    } finally {
      db.close();
    }
  } finally {
    await close(w.h);
  }
});

test("pane text with a lone surrogate is recorded well formed, and text the core refuses ends in a counted failure", async () => {
  const w = await world();
  try {
    const id = queue(w, w.h.developer.agentId);
    w.adapter.sendImpl = async () => ({
      sent: false,
      reason: "input_not_empty",
    });
    await w.tick();
    w.advance((TIMERS.maxDeferralSeconds + 1) * 1000);
    w.adapter.clearImpl = async (input) => {
      await input.discard("a\ud800b");
      return { cleared: true, text: "a\ud800b" };
    };
    await w.tick();
    const db = openSqlite(path.join(w.h.stateDirectory, "controller.sqlite"), {
      readOnly: true,
    });
    try {
      const row = db
        .prepare("SELECT text FROM message_input_clears WHERE message_id = ?")
        .get(id) as { text: string };
      assert.equal(row.text, "a\ufffdb");
    } finally {
      db.close();
    }

    w.adapter.clearImpl = async () => {
      throw new TypeError("input text must be non-empty");
    };
    for (let i = 0; i < FAILURE_LIMIT; i += 1) await w.tick();
    assert.equal(state(w, id), "failed");
  } finally {
    await close(w.h);
  }
});

test("clear errors: not idle and unreadable lines skip the agent; a line that never clears ends in a recorded failure", async () => {
  const w = await world();
  try {
    const id = queue(w, w.h.developer.agentId);
    w.adapter.sendImpl = async () => ({
      sent: false,
      reason: "input_not_empty",
    });
    await w.tick();
    w.advance((TIMERS.maxDeferralSeconds + 1) * 1000);

    w.adapter.clearImpl = async () => {
      throw new NotIdle("busy");
    };
    await w.tick();
    assert.equal(state(w, id), "deferred");
    assert.equal(
      w.adapter.sends,
      1,
      "the agent is skipped for the rest of the tick",
    );

    w.adapter.clearImpl = async () => {
      throw new InputUnreadable("cannot read");
    };
    for (let i = 0; i < STUCK_AFTER_TICKS; i += 1) await w.tick();
    assert.deepEqual(w.driver.snapshot().stuck, [
      { messageId: id, reason: "input_unreadable" },
    ]);
    assert.equal(
      w.events.filter((e) => e.event === "input_unreadable").length,
      1,
      "logged once per message",
    );

    w.adapter.clearImpl = async () => {
      throw new ClearFailed("stuck");
    };
    for (let i = 0; i < FAILURE_LIMIT; i += 1) await w.tick();
    assert.equal(state(w, id), "failed");
  } finally {
    await close(w.h);
  }
});

test("an unreadable line seen while sending is classified stuck at once, not after the maximum deferral", async () => {
  const w = await world();
  try {
    const id = queue(w, w.h.developer.agentId);
    w.adapter.sendImpl = async () => ({
      sent: false,
      reason: "input_not_empty",
      detail: "the input line is unreadable",
    });
    for (let i = 0; i < STUCK_AFTER_TICKS; i += 1) await w.tick();
    assert.deepEqual(w.driver.snapshot().stuck, [
      { messageId: id, reason: "input_unreadable" },
    ]);
  } finally {
    await close(w.h);
  }
});

test("a PM message waiting past the notification time notifies once, records it, and repeats at the interval", async () => {
  const w = await world(TIMERS, undefined, 86_400);
  try {
    const id = w.h.core.enqueueMessage(ctx(w.h.core, w.h.owner), {
      recipientAgentId: w.h.pm.agentId,
      body: "for the PM",
    }).messageId;
    await w.tick();
    assert.equal(w.notifier.sent.length, 0, "not before pm_notify_after");
    w.advance((TIMERS.pmNotifyAfterSeconds + 1) * 1000);
    await w.tick();
    assert.equal(w.notifier.sent.length, 1);
    assert.equal(w.notifier.sent[0]!.kind, "pm_message");
    assert.equal(w.notifier.sent[0]!.repeat, false);
    assert.notEqual(w.h.core.message(id)!.lastNotifiedAt, null);
    assert.equal(w.notifier.written[0]!.recorded, true);
    await w.tick();
    assert.equal(w.notifier.sent.length, 1, "not again before the interval");
    w.advance((TIMERS.notifyIntervalSeconds + 1) * 1000);
    await w.tick();
    assert.equal(w.notifier.sent.length, 2);
    assert.equal(w.notifier.sent[1]!.repeat, true);
  } finally {
    await close(w.h);
  }
});

test("a notification whose channels all failed is not recorded and is offered again on the next tick", async () => {
  const w = await world();
  try {
    const id = w.h.core.enqueueMessage(ctx(w.h.core, w.h.owner), {
      recipientAgentId: w.h.pm.agentId,
      body: "for the PM",
    }).messageId;
    w.advance((TIMERS.pmNotifyAfterSeconds + 1) * 1000);
    w.notifier.results = [{ channel: "herdr", ok: false }];
    await w.tick();
    assert.equal(w.h.core.message(id)!.lastNotifiedAt, null);
    assert.equal(w.notifier.written[0]!.recorded, false);
    w.notifier.results = [
      { channel: "herdr", ok: false },
      { channel: "fallback", ok: true },
    ];
    await w.tick();
    assert.notEqual(
      w.h.core.message(id)!.lastNotifiedAt,
      null,
      "the fallback alone is enough to record",
    );
    assert.equal(w.notifier.sent.length, 2);
  } finally {
    await close(w.h);
  }
});

test("a message that moved during the notification is not recorded and leaves no rejection row", async () => {
  const w = await world();
  try {
    const id = w.h.core.enqueueMessage(ctx(w.h.core, w.h.owner), {
      recipientAgentId: w.h.pm.agentId,
      body: "for the PM",
    }).messageId;
    w.advance((TIMERS.pmNotifyAfterSeconds + 1) * 1000);
    w.notifier.onSend = () => {
      w.h.core.resolveMessage(ctx(w.h.core, w.h.owner), id, "cancel");
    };
    await w.tick();
    assert.equal(w.notifier.written[0]!.recorded, false);
    assert.equal(rejections(w), 0);
    assert.equal(w.h.core.message(id)!.lastNotifiedAt, null);
  } finally {
    await close(w.h);
  }
});

test("ticks never overlap, and a step that throws does not stop the tick", async () => {
  const w = await world();
  try {
    let release!: () => void;
    w.adapter.observeGate = new Promise<void>((resolve) => {
      release = resolve;
    });
    const first = w.tick();
    await w.tick();
    assert.equal(w.adapter.sends, 0, "the overlapping call returned at once");
    release();
    await first;

    w.adapter.observeGate = undefined;
    const good = queue(w, w.h.developer.agentId);
    w.adapter.sendImpl = async () => {
      throw new Error("boom");
    };
    await w.tick();
    assert.equal(state(w, good), "queued");
    w.adapter.sendImpl = undefined;
    await w.tick();
    assert.equal(state(w, good), "sent");
  } finally {
    await close(w.h);
  }
});

test("stopping the driver waits for a running tick and prevents later ones", async () => {
  const w = await world();
  try {
    let release!: () => void;
    w.adapter.observeGate = new Promise<void>((resolve) => {
      release = resolve;
    });
    const running = w.tick();
    let stopped = false;
    const stopping = w.driver.stop().then(() => {
      stopped = true;
    });
    await new Promise((resolve) => setTimeout(resolve, 30));
    assert.equal(stopped, false, "stop waits for the tick");
    release();
    await running;
    await stopping;
    assert.equal(stopped, true);
  } finally {
    await close(w.h);
  }
});

test("a configured timers object with more than the message timers (the finding deadline) still advances the message timers", async () => {
  const clock = { now: Date.parse("2026-01-01T00:00:00.000Z") };
  const h = await harness({ clock: () => new Date(clock.now) });
  try {
    const adapter = new StubAdapter();
    adapter.register(h.developer.agentId);
    const events: string[] = [];
    const driver = new DeliveryDriver({
      core: h.core,
      adapter,
      timers: { ...TIMERS, findingCheckSeconds: 1800 } as MessagingTimers,
      notifier: new StubNotifier(),
      credential: h.owner,
      now: () => clock.now,
      log: (event) => events.push(event),
    });
    const id = h.core.enqueueMessage(ctx(h.core, h.pm.credential), {
      recipientAgentId: h.developer.agentId,
      body: "do it",
    }).messageId;
    await driver.tick();
    assert.equal(h.core.message(id)!.state, "sent");
    clock.now += (TIMERS.workerAckTimeoutSeconds + 1) * 1000;
    await driver.tick();
    assert.equal(h.core.message(id)!.state, "unacked", "the timer advanced");
    assert.ok(!events.includes("advance_failed"), events.join(","));
  } finally {
    await close(h);
  }
});

test("the timers of a resolved capstan.toml, whatever else it holds, are accepted by the driver", async () => {
  const directory = mkdtempSync(path.join(tmpdir(), "capstan-driver-config-"));
  try {
    writeFileSync(
      path.join(directory, "capstan.toml"),
      'schema_version = 1\n\n[hosts.claude]\nkind = "claude"\n\n[roles.pm]\nkind = "PM"\nhost = "claude"\n\n[timers]\nfinding_check_seconds = 60\nworker_ack_timeout_seconds = 30\n',
      { mode: 0o600 },
    );
    const config = loadCapstanConfig(directory);
    assert.ok("findingCheckSeconds" in config.timers);
    const clock = { now: Date.parse("2026-01-01T00:00:00.000Z") };
    const h = await harness({ clock: () => new Date(clock.now) });
    try {
      const adapter = new StubAdapter();
      adapter.register(h.developer.agentId);
      const events: string[] = [];
      const driver = new DeliveryDriver({
        core: h.core,
        adapter,
        timers: config.timers,
        notifier: new StubNotifier(),
        credential: h.owner,
        now: () => clock.now,
        log: (event) => events.push(event),
      });
      const id = h.core.enqueueMessage(ctx(h.core, h.pm.credential), {
        recipientAgentId: h.developer.agentId,
        body: "do it",
      }).messageId;
      await driver.tick();
      clock.now += 31_000;
      await driver.tick();
      assert.equal(h.core.message(id)!.state, "unacked");
      assert.ok(!events.includes("advance_failed"));
    } finally {
      await close(h);
    }
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});

const WAKE_TIMERS: MessagingTimers = {
  ...TIMERS,
  pmWakeAfterSeconds: 20,
  pmWakeIntervalSeconds: 120,
};

test("an idle PM with an unread message is woken once, recorded first, and not again within the interval", async () => {
  const w = await world(WAKE_TIMERS);
  try {
    w.adapter.register(w.h.pm.agentId);
    const id = w.h.core.enqueueMessage(ctx(w.h.core, w.h.owner), {
      recipientAgentId: w.h.pm.agentId,
      body: "report",
    }).messageId;
    await w.tick();
    assert.equal(w.adapter.wakes.length, 0, "not before the wait");
    w.advance(21_000);
    await w.tick();
    assert.deepEqual(w.adapter.wakes, [
      {
        paneId: `w1:${w.h.pm.agentId}`,
        text: "Run cstan inbox: a teammate has written to you.",
      },
    ]);
    assert.ok(w.eventNames().includes("pm_wake_typed"));
    await w.tick();
    assert.equal(w.adapter.wakes.length, 1, "not within the interval");
    w.advance(121_000);
    await w.tick();
    assert.equal(w.adapter.wakes.length, 2);
    w.h.core.recordSent(ctx(w.h.core, w.h.owner), id);
    w.advance(121_000);
    await w.tick();
    assert.equal(
      w.adapter.wakes.length,
      2,
      "a message the PM read is not a reason",
    );
  } finally {
    await close(w.h);
  }
});

test("a wake the adapter declines is logged once and records nothing, so the next tick tries again", async () => {
  const w = await world(WAKE_TIMERS);
  try {
    w.adapter.register(w.h.pm.agentId);
    w.adapter.wakeOutcome = "input_not_empty";
    w.h.core.enqueueMessage(ctx(w.h.core, w.h.owner), {
      recipientAgentId: w.h.pm.agentId,
      body: "report",
    });
    w.advance(30_000);
    await w.tick();
    await w.tick();
    assert.equal(
      w.events.filter((e) => e.event === "wake_skipped").length,
      1,
      "logged once",
    );
    w.adapter.wakeOutcome = "sent";
    await w.tick();
    assert.equal(
      w.adapter.wakes.length,
      1,
      "the declined wakes were never recorded",
    );
  } finally {
    await close(w.h);
  }
});

test("a stalled worker makes the driver queue one Agent stalled notice for the PM", async () => {
  const w = await world();
  try {
    w.adapter.states.set(w.h.developer.agentId, "working");
    await w.tick();
    w.advance((TIMERS.stallAfterSeconds + 5) * 1000);
    await w.tick();
    await w.tick();
    const told = w.h.core
      .messagesFor(w.h.pm.agentId)
      .filter((m) => m.body.startsWith("Agent stalled"));
    assert.equal(told.length, 1);
    assert.match(told[0]!.body, new RegExp(w.h.developer.agentId));
  } finally {
    await close(w.h);
  }
});

const blockedNotices = (w: World) =>
  w.h.core
    .messagesFor(w.h.pm.agentId)
    .filter((m) => m.body.startsWith("Agent blocked"));

const UNREADABLE: SendOutcome = {
  sent: false,
  reason: "input_not_empty",
  detail: "the input line is unreadable",
};

test("an unreadable input queues one Agent blocked notice per episode for a worker, including the clear path", async () => {
  const w = await world();
  try {
    const dev = w.h.developer.agentId;
    const first = queue(w, dev);
    w.adapter.sendImpl = async () => UNREADABLE;
    await w.tick();
    assert.equal(blockedNotices(w).length, 1);
    assert.match(blockedNotices(w)[0]!.body, new RegExp(`${dev} cannot take`));
    await w.tick();
    await w.tick();
    w.advance((TIMERS.maxDeferralSeconds + 1) * 1000);
    w.adapter.clearImpl = async () => {
      throw new InputUnreadable("cannot read");
    };
    await w.tick();
    await w.tick();
    assert.equal(blockedNotices(w).length, 1, "same head, no repeat");

    // A busy deferral between unreadable ticks does not close the episode.
    w.adapter.sendImpl = async () => ({ sent: false, reason: "agent_busy" });
    await w.tick();
    w.adapter.sendImpl = async () => UNREADABLE;
    await w.tick();
    assert.equal(blockedNotices(w).length, 1);

    // Reading the input empty and sending closes it; a new head warns again.
    w.adapter.sendImpl = undefined;
    w.adapter.clearImpl = undefined;
    await w.tick();
    await w.tick();
    assert.equal(state(w, first), "sent");
    w.h.core.ackMessage(ctx(w.h.core, w.h.developer.credential), first);
    const second = queue(w, dev, "next");
    w.adapter.sendImpl = async () => UNREADABLE;
    await w.tick();
    assert.equal(state(w, second), "deferred");
    assert.equal(blockedNotices(w).length, 2);
  } finally {
    await close(w.h);
  }
});

test("an unreadable input queues one Agent blocked notice for a Supervisor", async () => {
  const w = await world();
  try {
    const supervisor = w.h.addMember("supervisor", "Supervisor");
    w.adapter.register(supervisor.agentId);
    queue(w, supervisor.agentId);
    w.adapter.sendImpl = async () => UNREADABLE;
    await w.tick();
    await w.tick();
    assert.equal(blockedNotices(w).length, 1);
    assert.match(blockedNotices(w)[0]!.body, new RegExp(supervisor.agentId));
  } finally {
    await close(w.h);
  }
});

test("a readable non-empty input closes the episode so a later unreadable head warns again", async () => {
  const w = await world();
  try {
    const dev = w.h.developer.agentId;
    queue(w, dev);
    w.adapter.sendImpl = async () => UNREADABLE;
    await w.tick();
    w.adapter.sendImpl = async () => ({
      sent: false,
      reason: "input_not_empty",
    });
    await w.tick();
    w.adapter.sendImpl = async () => UNREADABLE;
    await w.tick();
    assert.equal(
      blockedNotices(w).length,
      1,
      "same head message: the core dedupes it",
    );
  } finally {
    await close(w.h);
  }
});

class StubProbe implements ProcessActivityProbe {
  calls: string[] = [];
  cpuMs = 0;
  present = true;
  failure: Error | undefined;
  async sample(paneId: string): Promise<ProcessSample> {
    this.calls.push(paneId);
    if (this.failure) throw this.failure;
    return {
      processes: this.present
        ? [
            {
              pid: 7,
              ppid: 6,
              comm: "npm test",
              cpuMs: this.cpuMs,
              startKey: "k",
            },
          ]
        : [],
    };
  }
}

const stalledNotices = (w: World) =>
  w.h.core
    .messagesFor(w.h.pm.agentId)
    .filter((m) => m.body.startsWith("Agent stalled"));

/** Ticks every sample interval for `ms`, adding CPU to the stub child each time. */
async function run(w: World, probe: StubProbe, ms: number, cpuPerStep: number) {
  for (let spent = 0; spent < ms; spent += PROCESS_SAMPLE_MS) {
    w.advance(PROCESS_SAMPLE_MS);
    probe.cpuMs += cpuPerStep;
    await w.tick();
  }
}

test("a working agent whose child keeps using CPU is not reported stalled", async () => {
  const probe = new StubProbe();
  const w = await world(TIMERS, probe);
  try {
    w.adapter.states.set(w.h.developer.agentId, "working");
    await w.tick();
    await run(w, probe, (TIMERS.stallAfterSeconds + 120) * 1000, 500);
    assert.deepEqual(w.driver.snapshot().stalledAgentIds, []);
    assert.equal(stalledNotices(w).length, 0);
    assert.equal(
      w.events.filter((e) => e.event === "stall_suppressed").length,
      1,
    );
    assert.equal(w.eventNames().includes("agent_stalled"), false);
  } finally {
    await close(w.h);
  }
});

test("a hung child is reported stalled, measured from its last CPU activity", async () => {
  const probe = new StubProbe();
  const w = await world(TIMERS, probe);
  try {
    w.adapter.states.set(w.h.developer.agentId, "working");
    await w.tick();
    const busyMs = 600 * 1000;
    await run(w, probe, busyMs, 500);
    const lastBusy = w.clock.now;
    // Hung: the CPU total stays flat.
    await run(w, probe, (TIMERS.stallAfterSeconds - 60) * 1000, 0);
    assert.deepEqual(w.driver.snapshot().stalledAgentIds, []);
    await run(w, probe, 120 * 1000, 0);
    assert.deepEqual(w.driver.snapshot().stalledAgentIds, [
      w.h.developer.agentId,
    ]);
    assert.ok(w.clock.now - lastBusy >= TIMERS.stallAfterSeconds * 1000);
    assert.equal(stalledNotices(w).length, 1);
  } finally {
    await close(w.h);
  }
});

test("with no child, or a probe that throws, a real stall is still reported", async () => {
  for (const mode of ["none", "throws"] as const) {
    const probe = new StubProbe();
    if (mode === "none") probe.present = false;
    else probe.failure = new Error("boom");
    const w = await world(TIMERS, probe);
    try {
      w.adapter.states.set(w.h.developer.agentId, "working");
      await w.tick();
      await run(w, probe, (TIMERS.stallAfterSeconds + 30) * 1000, 0);
      assert.deepEqual(w.driver.snapshot().stalledAgentIds, [
        w.h.developer.agentId,
      ]);
      assert.equal(stalledNotices(w).length, 1);
      assert.equal(
        w.events.filter((e) => e.event === "process_probe_failed").length,
        mode === "throws" ? 1 : 0,
        "a failure is logged once",
      );
    } finally {
      await close(w.h);
    }
  }
});

test("an idle or paused agent is never sampled and sampling is throttled", async () => {
  const probe = new StubProbe();
  const w = await world(TIMERS, probe);
  try {
    await w.tick();
    await w.tick();
    assert.equal(probe.calls.length, 0, "idle");
    w.adapter.states.set(w.h.developer.agentId, "working");
    await w.tick();
    await w.tick();
    assert.equal(probe.calls.length, 1, "at most one sample per interval");
    w.advance(PROCESS_SAMPLE_MS);
    await w.tick();
    assert.equal(probe.calls.length, 2);
    w.h.core.pauseAgent(ctx(w.h.core, w.h.owner), {
      agentId: w.h.developer.agentId,
      reason: "hold",
    });
    w.advance(PROCESS_SAMPLE_MS * 2);
    await w.tick();
    assert.equal(probe.calls.length, 2, "paused");
  } finally {
    await close(w.h);
  }
});

/** A probe over a stubbed /proc whose test runner parent is flat while its exited children's CPU (cutime, cstime) may grow. */
function procProbe(childTicks: () => number): ProcessActivityProbe {
  const stat = (pid: number, ppid: number, comm: string, children = 0) =>
    parseProcStat(
      pid,
      `${pid} (${comm}) S ${ppid} ${pid} ${pid} 0 -1 0 0 0 0 0 10 5 ${children} 0 20 0 1 0 ${pid} 1000 10`,
    )!;
  return new HerdrProcessProbe(
    async () => ({
      code: 0,
      stdout: '{"result":{"process_info":{"shell_pid":1}}}',
      stderr: "",
    }),
    async () => [
      stat(1, 0, "bash"),
      stat(2, 1, "claude"),
      stat(3, 2, "zsh"),
      stat(4, 3, "node", childTicks()),
    ],
  );
}

test("an idle test-runner parent whose exited children keep using CPU is not reported stalled, and flat child totals are", async () => {
  for (const growing of [true, false]) {
    let ticks = 0;
    const w = await world(
      TIMERS,
      procProbe(() => ticks),
    );
    try {
      w.adapter.states.set(w.h.developer.agentId, "working");
      await w.tick();
      for (
        let spent = 0;
        spent < (TIMERS.stallAfterSeconds + 120) * 1000;
        spent += PROCESS_SAMPLE_MS
      ) {
        w.advance(PROCESS_SAMPLE_MS);
        if (growing) ticks += 50;
        await w.tick();
      }
      assert.equal(w.driver.snapshot().stalledAgentIds.length, growing ? 0 : 1);
    } finally {
      await close(w.h);
    }
  }
});

function pmMessage(w: World, body = "report"): string {
  return w.h.core.enqueueMessage(ctx(w.h.core, w.h.owner), {
    recipientAgentId: w.h.pm.agentId,
    body,
  }).messageId;
}

const staleSent = (w: World): NotificationRequest[] =>
  w.notifier.sent.filter((r) => r.kind === "pm_stale");

const count = (w: World, event: string): number =>
  w.events.filter((e) => e.event === event).length;

test("a wake the PM is not ready for is not recorded, and is tried again once the PM is idle with an empty line", async () => {
  const w = await world(WAKE_TIMERS);
  try {
    w.adapter.register(w.h.pm.agentId);
    const id = pmMessage(w);
    w.advance(30_000);
    for (const outcome of ["pm_not_idle", "input_not_empty"] as const) {
      w.adapter.wakeOutcome = outcome;
      await w.tick();
      assert.equal(w.adapter.wakes.length, 0, outcome);
    }
    assert.equal(count(w, "pm_wake_typed"), 0);
    w.adapter.wakeOutcome = "sent";
    await w.tick();
    assert.equal(w.adapter.wakes.length, 1);
    assert.equal(count(w, "pm_wake_typed"), 1);
    assert.equal(state(w, id), "queued");
  } finally {
    await close(w.h);
  }
});

test("a wake with no pull logs pm_wake_unanswered once and re-wakes after the backoff; a pull stops further wakes", async () => {
  const w = await world(WAKE_TIMERS);
  try {
    w.adapter.register(w.h.pm.agentId);
    const id = pmMessage(w);
    w.advance(21_000);
    await w.tick();
    assert.equal(w.adapter.wakes.length, 1);
    w.advance(60_000);
    await w.tick();
    assert.equal(count(w, "pm_wake_unanswered"), 0, "within the interval");
    w.advance(61_000);
    await w.tick();
    assert.equal(count(w, "pm_wake_unanswered"), 1);
    assert.equal(w.adapter.wakes.length, 2, "re-woken after the backoff");
    await w.tick();
    await w.tick();
    assert.equal(count(w, "pm_wake_unanswered"), 1, "logged once per wake");
    w.advance(241_000);
    await w.tick();
    assert.equal(count(w, "pm_wake_unanswered"), 2);
    assert.equal(w.adapter.wakes.length, 3, "the second wait is 240 s");
    w.h.core.recordSent(ctx(w.h.core, w.h.owner), id);
    w.advance(5_000_000);
    await w.tick();
    assert.equal(w.adapter.wakes.length, 3, "a pulled message stops the wakes");
    assert.equal(count(w, "pm_wake_unanswered"), 2);
  } finally {
    await close(w.h);
  }
});

test("a wake that throws after it was recorded is pm_wake_failed, never typed, and is retried at the next backoff step", async () => {
  const w = await world(WAKE_TIMERS);
  try {
    w.adapter.register(w.h.pm.agentId);
    pmMessage(w);
    w.advance(21_000);
    w.adapter.wakeThrowsAfterRecord = true;
    await w.tick();
    assert.equal(count(w, "pm_wake_failed"), 1);
    assert.equal(count(w, "pm_wake_typed"), 0);
    await w.tick();
    assert.equal(count(w, "pm_wake_failed"), 1, "not before the backoff");
    w.adapter.wakeThrowsAfterRecord = false;
    w.advance(121_000);
    await w.tick();
    assert.equal(w.adapter.wakes.length, 1);
    assert.equal(count(w, "pm_wake_typed"), 1);
  } finally {
    await close(w.h);
  }
});

test("a PM with only pulled messages gets no wake and no unanswered log, and a stale episode once the oldest is past the threshold", async () => {
  const w = await world(WAKE_TIMERS, undefined, 600);
  try {
    w.adapter.register(w.h.pm.agentId);
    const id = pmMessage(w);
    w.h.core.recordSent(ctx(w.h.core, w.h.owner), id);
    w.advance(300_000);
    await w.tick();
    assert.equal(staleSent(w).length, 0, "not stale yet");
    w.advance(301_000);
    await w.tick();
    assert.equal(w.adapter.wakes.length, 0);
    assert.equal(count(w, "pm_wake_unanswered"), 0);
    assert.deepEqual(
      staleSent(w).map((r) => [r.kind, r.messageId]),
      [["pm_stale", id]],
    );
    assert.equal(w.driver.snapshot().pmStale?.oldestMessageId, id);
  } finally {
    await close(w.h);
  }
});

test("a stale PM message notifies once per episode, again for a new episode, and never while the PM is paused", async () => {
  const w = await world(TIMERS, undefined, 600);
  try {
    w.adapter.register(w.h.pm.agentId);
    assert.equal(w.driver.snapshot().pmStale ?? null, null);
    const first = pmMessage(w, "one");
    w.advance(601_000);
    for (let i = 0; i < 5; i += 1) await w.tick();
    assert.equal(staleSent(w).length, 1, "once across many ticks");
    assert.equal(staleSent(w)[0]!.kind, "pm_stale");
    const snapshot = w.driver.snapshot().pmStale;
    assert.equal(snapshot?.oldestMessageId, first);
    assert.equal(snapshot?.pending, 1);
    assert.equal(snapshot?.notified, true);
    // The episode ends when the oldest message is no longer pending.
    w.h.core.recordSent(ctx(w.h.core, w.h.owner), first);
    w.h.core.ackMessage(ctx(w.h.core, w.h.pm.credential), first);
    await w.tick();
    assert.equal(w.driver.snapshot().pmStale, null);
    pmMessage(w, "two");
    w.advance(601_000);
    await w.tick();
    await w.tick();
    assert.equal(staleSent(w).length, 2, "a new episode fires again");
    // Paused: nothing fires for a new episode until resumed.
    const third = pmMessage(w, "three");
    w.h.core.pauseAgent(ctx(w.h.core, w.h.owner), {
      agentId: w.h.pm.agentId,
      reason: "hold",
    });
    w.advance(601_000);
    await w.tick();
    assert.equal(staleSent(w).length, 2, "none while paused");
    assert.ok(third);
  } finally {
    await close(w.h);
  }
});
