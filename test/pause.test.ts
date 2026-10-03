import assert from "node:assert/strict";
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import path from "node:path";
import { test } from "node:test";
import { openSqlite } from "../src/controller/sqlite.js";
import { pauseLines } from "../src/cli.js";
import type { CommandResponse } from "../src/daemon.js";
import { plainLines } from "../src/dash/lines.js";
import { makeTheme } from "../src/dash/theme.js";
import { buildFrame } from "../src/dash/view.js";
import { DeliveryDriver, type DriverAdapter } from "../src/driver.js";
import {
  ControllerCore,
  RunPausedError,
  TransitionAuthorizationError,
} from "../src/controller/core.js";
import type {
  HerdrState,
  MessagingTimers,
} from "../src/controller/messaging.js";
import type { Notifier } from "../src/notifier.js";
import { close, call, ctx, harness, type Harness } from "./harness.js";
import { operatorWorld } from "./operator-harness.js";
import { NOW, iso, showcase } from "./dash-fixtures.js";
import { modelOf, viewOf } from "./dash-view-helpers.js";

const TIMERS: MessagingTimers = {
  maxDeferralSeconds: 120,
  maxBusyDeferralSeconds: 120,
  pmAckTimeoutSeconds: 600,
  pmNotifyAfterSeconds: 300,
  notifyIntervalSeconds: 600,
  stallAfterSeconds: 900,
  workerAckTimeoutSeconds: 600,
  pmWakeAfterSeconds: 60,
  pmWakeIntervalSeconds: 120,
};

class Adapter implements DriverAdapter {
  readonly typed: string[] = [];
  readonly states = new Map<string, HerdrState>();
  readonly panes = new Map<string, string>();
  busy = false;
  register(agentId: string): void {
    this.panes.set(agentId, `w1:${agentId}`);
  }
  paneForAgent(agentId: string): string | undefined {
    return this.panes.get(agentId);
  }
  paneEntry(paneId: string): { agent?: string } | undefined {
    const agent = [...this.panes].find(([, pane]) => pane === paneId)?.[0];
    return agent === undefined ? undefined : { agent };
  }
  async agentObservation(agentId: string): Promise<HerdrState> {
    return this.states.get(agentId) ?? "idle";
  }
  async guardedSend(input: {
    paneId: string;
    text: string;
    beforeSend: () => void | Promise<void>;
  }) {
    if (this.busy)
      return { sent: false as const, reason: "agent_busy" as const };
    await input.beforeSend();
    this.typed.push(input.text);
    return { sent: true as const };
  }
  async wakePm() {
    return { sent: false as const, reason: "pm_not_idle" as const };
  }
  async clearAfterDeferral() {
    return { cleared: true, text: "" };
  }
}

const notifier: Notifier = {
  async send() {
    return [];
  },
  write() {
    return undefined;
  },
};

interface World {
  h: Harness;
  adapter: Adapter;
  driver: DeliveryDriver;
  clock: { now: number };
}

async function world(): Promise<World> {
  const clock = { now: Date.parse("2026-01-01T00:00:00.000Z") };
  const h = await harness({ clock: () => new Date(clock.now) });
  const adapter = new Adapter();
  adapter.register(h.developer.agentId);
  adapter.register(h.pm.agentId);
  const driver = new DeliveryDriver({
    core: h.core,
    adapter,
    timers: TIMERS,
    notifier,
    credential: h.owner,
    now: () => clock.now,
  });
  return { h, adapter, driver, clock };
}

const send = (w: World, to: string, body: string): string =>
  w.h.core.enqueueMessage(ctx(w.h.core, w.h.pm.credential), {
    recipientAgentId: to,
    body,
  }).messageId;

const stateOf = (w: World, id: string): string => w.h.core.message(id)!.state;

const ok = (response: CommandResponse): unknown => {
  assert.equal(response.ok, true, JSON.stringify(response));
  return (response as { result: unknown }).result;
};
const code = (response: CommandResponse): string =>
  response.ok ? "ok" : response.code;
const text = (response: CommandResponse): string =>
  response.ok ? "" : response.message;

/** Pulls and acks the PM's queue until `messageId` is handed over (controller notices come first); false when it never is. */
function pullUntil(h: Harness, messageId: string): boolean {
  for (let i = 0; i < 10; i += 1) {
    const pulled = h.core.pullMessage(ctx(h.core, h.pm.credential)).message;
    if (pulled === null) return false;
    if (pulled.messageId === messageId) return true;
    h.core.ackMessage(ctx(h.core, h.pm.credential), pulled.messageId);
  }
  return false;
}

function rows<T>(h: Harness, sql: string, ...params: unknown[]): T[] {
  const db = openSqlite(path.join(h.stateDirectory, "controller.sqlite"), {
    readOnly: true,
  });
  try {
    return db.prepare(sql).all(...params) as T[];
  } finally {
    db.close();
  }
}

test("pause and resume: the operator and the PM may, a worker may not, a reason is required, doubles are refused", async () => {
  const h = await harness();
  try {
    assert.equal(
      code(await call(h, h.developer.credential, "pause", ["--reason", "x"])),
      "forbidden",
    );
    assert.equal(
      code(await call(h, h.developer.credential, "resume", ["--reason", "x"])),
      "forbidden",
    );
    assert.equal(code(await call(h, h.owner, "pause")), "invalid_request");
    assert.equal(
      code(await call(h, h.owner, "pause", ["--reason"])),
      "invalid_request",
    );
    assert.equal(
      code(await call(h, h.owner, "pause", ["--reason", ""])),
      "invalid_request",
    );
    assert.equal(
      code(await call(h, h.owner, "pause", ["--reason", "x".repeat(501)])),
      "invalid_request",
    );
    assert.equal(
      code(await call(h, h.owner, "resume", ["--reason", "x"])),
      "conflict",
      "resuming an active run",
    );

    ok(await call(h, h.owner, "pause", ["--reason", "check the plan"]));
    assert.equal(h.core.pauseState().run?.reason, "check the plan");
    const twice = await call(h, h.pm.credential, "pause", [
      "--reason",
      "again",
    ]);
    assert.equal(code(twice), "conflict");
    assert.match(text(twice), /already paused: check the plan/);
    ok(await call(h, h.pm.credential, "resume", ["--reason", "plan is fine"]));
    assert.equal(h.core.pauseState().run, null);
    assert.equal(h.core.statusSnapshot().run.state, "active");

    // The PM may pause and resume the run, and an agent, but not itself.
    ok(await call(h, h.pm.credential, "pause", ["--reason", "pm pause"]));
    ok(await call(h, h.pm.credential, "resume", ["--reason", "pm resume"]));
    ok(
      await call(h, h.pm.credential, "pause", [
        h.developer.agentId,
        "--reason",
        "hold dev",
      ]),
    );
    assert.equal(
      code(
        await call(h, h.pm.credential, "pause", [
          h.developer.agentId,
          "--reason",
          "twice",
        ]),
      ),
      "conflict",
    );
    assert.equal(
      code(
        await call(h, h.pm.credential, "pause", [
          h.pm.agentId,
          "--reason",
          "self",
        ]),
      ),
      "rejected",
    );
    assert.equal(
      code(await call(h, h.owner, "pause", ["nobody", "--reason", "x"])),
      "unknown_agent",
    );
    ok(
      await call(h, h.owner, "resume", [h.developer.agentId, "--reason", "go"]),
    );
    assert.equal(
      code(
        await call(h, h.owner, "resume", [
          h.developer.agentId,
          "--reason",
          "go",
        ]),
      ),
      "conflict",
    );
    assert.deepEqual(h.core.pauseState().agents, []);
    // The PM may not cancel or complete the run through the transition table.
    assert.throws(
      () => h.core.transitionRun(ctx(h.core, h.pm.credential), "canceling"),
      TransitionAuthorizationError,
    );
  } finally {
    await close(h);
  }
});

test("each pause and resume is a ledger event with its reason and actor; an operator run pause adds an operator action and notifies the PM", async () => {
  const h = await harness();
  try {
    ok(await call(h, h.owner, "pause", ["--reason", "run reason"]));
    ok(await call(h, h.owner, "resume", ["--reason", "run resume"]));
    ok(
      await call(h, h.owner, "pause", [
        h.developer.agentId,
        "--reason",
        "agent reason",
      ]),
    );
    ok(
      await call(h, h.pm.credential, "resume", [
        h.developer.agentId,
        "--reason",
        "agent resume",
      ]),
    );
    const events = rows<{
      entity_type: string;
      entity_id: string;
      payload_json: string;
      actor_id: string;
    }>(
      h,
      "SELECT entity_type, entity_id, payload_json, actor_id FROM controller_events WHERE (entity_type = 'run_control' OR entity_type = 'agent') ORDER BY sequence",
    )
      .map((row) => ({ ...row, payload: JSON.parse(row.payload_json) }))
      .filter((event) => event.payload.details?.reason !== undefined);
    const reasons = events.map((event) => event.payload.details.reason);
    assert.deepEqual(reasons.filter(Boolean), [
      "run reason",
      "run resume",
      "agent reason",
      "agent resume",
    ]);
    const agentEvents = events.filter((event) => event.entity_type === "agent");
    assert.equal(agentEvents[0]!.payload.details.scope, "agent");
    assert.equal(agentEvents[0]!.entity_id, h.developer.agentId);
    assert.equal(agentEvents[1]!.actor_id, h.pm.actorId);
    const actions = rows<{ payload_json: string }>(
      h,
      "SELECT payload_json FROM operator_actions WHERE action_type = 'run.transition' ORDER BY created_at",
    );
    assert.equal(actions.length, 2);
    assert.equal(JSON.parse(actions[0]!.payload_json).reason, "run reason");
    const pauses = rows<{
      scope: string;
      reason: string;
      resume_reason: string;
      resumed_by: string;
    }>(
      h,
      "SELECT scope, reason, resume_reason, resumed_by FROM pauses ORDER BY paused_at",
    );
    assert.equal(pauses.length, 2);
    assert.ok(
      pauses.every(
        (row) => row.resume_reason !== null && row.resumed_by !== null,
      ),
    );
    const notices = h.core.messagesFor(h.pm.agentId).map((m) => m.body);
    assert.ok(
      notices.some((body) => /operator paused the run: run reason/.test(body)),
    );
    assert.ok(
      notices.some((body) => /operator resumed the run: run resume/.test(body)),
    );
    assert.ok(
      notices.some((body) =>
        /operator paused developer-agent: agent reason/.test(body),
      ),
    );
  } finally {
    await close(h);
  }
});

test("a paused agent keeps three messages queued, none is typed or expires, and after resume they arrive in order, one per ack", async () => {
  const w = await world();
  try {
    const dev = w.h.developer.agentId;
    ok(await call(w.h, w.h.owner, "pause", [dev, "--reason", "look first"]));
    const ids = [
      send(w, dev, "one"),
      send(w, dev, "two"),
      send(w, dev, "three"),
    ];
    for (let i = 0; i < 4; i += 1) {
      await w.driver.tick();
      w.clock.now += 400_000;
    }
    assert.deepEqual(w.adapter.typed, []);
    assert.deepEqual(
      ids.map((id) => stateOf(w, id)),
      ["queued", "queued", "queued"],
    );
    assert.equal(w.h.core.messageRejections().length, 0);

    ok(await call(w.h, w.h.owner, "resume", [dev, "--reason", "carry on"]));
    await w.driver.tick();
    assert.equal(w.adapter.typed.length, 1);
    assert.match(w.adapter.typed[0]!, /\none\n/);
    for (const [index, body] of ["two", "three"].entries()) {
      await w.driver.tick();
      assert.equal(
        w.adapter.typed.length,
        index + 1,
        "the next waits for the ack",
      );
      w.h.core.ackMessage(ctx(w.h.core, w.h.developer.credential), ids[index]!);
      await w.driver.tick();
      assert.match(w.adapter.typed[index + 1]!, new RegExp(`\\n${body}\\n`));
    }
  } finally {
    await close(w.h);
  }
});

test("a run pause holds every worker but the PM keeps receiving", async () => {
  const w = await world();
  try {
    const dev = w.h.developer.agentId;
    const sup = w.h.addMember("supervisor", "Supervisor");
    w.adapter.register(sup.agentId);
    ok(await call(w.h, w.h.owner, "pause", ["--reason", "hold all"]));
    assert.equal(w.h.core.isDeliveryPaused(dev), true);
    assert.equal(w.h.core.isDeliveryPaused(sup.agentId), true);
    assert.equal(w.h.core.isDeliveryPaused(w.h.pm.agentId), false);
    const devMessage = send(w, dev, "worker task");
    const supMessage = send(w, sup.agentId, "check");
    const pmMessage = w.h.core.enqueueMessage(ctx(w.h.core, w.h.owner), {
      recipientAgentId: w.h.pm.agentId,
      body: "for the PM",
    }).messageId;
    await w.driver.tick();
    assert.deepEqual(w.adapter.typed, []);
    assert.equal(stateOf(w, devMessage), "queued");
    assert.equal(stateOf(w, supMessage), "queued");
    assert.equal(
      pullUntil(w.h, pmMessage),
      true,
      "the PM still gets its messages",
    );
    ok(await call(w.h, w.h.owner, "resume", ["--reason", "go"]));
    await w.driver.tick();
    assert.equal(w.adapter.typed.length, 2);
    assert.equal(stateOf(w, devMessage), "sent");
  } finally {
    await close(w.h);
  }
});

test("a paused PM is handed nothing new", async () => {
  const h = await harness();
  try {
    ok(await call(h, h.owner, "pause", [h.pm.agentId, "--reason", "quiet"]));
    const id = h.core.enqueueMessage(ctx(h.core, h.owner), {
      recipientAgentId: h.pm.agentId,
      body: "later",
    }).messageId;
    assert.equal(
      h.core.pullMessage(ctx(h.core, h.pm.credential)).message,
      null,
    );
    assert.equal(h.core.message(id)!.state, "queued");
    ok(await call(h, h.owner, "resume", [h.pm.agentId, "--reason", "back"]));
    assert.equal(pullUntil(h, id), true);
  } finally {
    await close(h);
  }
});

test("while the run is paused spawn, launch, request-review, integrate, plan assign and operator proposal runs are refused with the reason, and work again after resume", async () => {
  const h = await harness();
  try {
    ok(await call(h, h.owner, "pause", ["--reason", "audit in progress"]));
    const refused = [
      await call(h, h.owner, "spawn", ["developer"]),
      await call(h, h.pm.credential, "spawn", ["developer"]),
      await call(h, h.owner, "launch"),
      await call(h, h.pm.credential, "request-review", ["report-1"]),
      await call(h, h.pm.credential, "integrate", ["report-1"]),
    ];
    for (const response of refused) {
      assert.equal(code(response), "conflict");
      assert.match(text(response), /paused/);
      assert.match(text(response), /audit in progress/);
    }
    assert.throws(
      () =>
        h.core.assignPackage(ctx(h.core, h.pm.credential), {
          planId: "p",
          packageId: "k",
          agentId: h.developer.agentId,
        }),
      (error: Error) =>
        error instanceof RunPausedError &&
        /plan assign.*audit in progress/.test(error.message),
    );
    ok(await call(h, h.owner, "resume", ["--reason", "done"]));
    for (const response of [
      await call(h, h.owner, "spawn", ["developer"]),
      await call(h, h.owner, "launch"),
      await call(h, h.pm.credential, "request-review", ["report-1"]),
      await call(h, h.pm.credential, "integrate", ["report-1"]),
    ])
      assert.doesNotMatch(text(response), /paused/);
    assert.throws(
      () =>
        h.core.assignPackage(ctx(h.core, h.pm.credential), {
          planId: "p",
          packageId: "k",
          agentId: h.developer.agentId,
        }),
      (error: Error) =>
        !(error instanceof RunPausedError) &&
        /does not exist/.test(error.message),
    );

    // A package cannot be assigned to a paused agent.
    ok(
      await call(h, h.pm.credential, "pause", [
        h.developer.agentId,
        "--reason",
        "hold dev",
      ]),
    );
    assert.throws(
      () =>
        h.core.assignPackage(ctx(h.core, h.pm.credential), {
          planId: "p",
          packageId: "k",
          agentId: h.developer.agentId,
        }),
      /developer-agent is paused/,
    );
  } finally {
    await close(h);
  }
});

test("a Supervisor cannot raise a finding against a paused target", async () => {
  const h = await harness();
  try {
    const sup = h.addMember("supervisor", "Supervisor");
    ok(
      await call(h, h.owner, "pause", [
        h.developer.agentId,
        "--reason",
        "hold",
      ]),
    );
    const response = await call(h, sup.credential, "finding", [
      h.developer.agentId,
      "low",
      "evidence",
      "fix it",
      "when fixed",
    ]);
    assert.equal(response.ok, false);
    assert.match(text(response), /target is paused/);
  } finally {
    await close(h);
  }
});

test("no stalled, blocked, wake or notify output for a paused agent, a deferred head does not expire, and its clock restarts on resume", async () => {
  const w = await world();
  try {
    const dev = w.h.developer.agentId;
    const observe = (agentId: string, state: HerdrState): void =>
      void w.h.core.recordAgentObservation(
        ctx(w.h.core, w.h.owner),
        agentId,
        state,
      );
    const advance = () =>
      w.h.core.advanceMessaging(ctx(w.h.core, w.h.owner), TIMERS);
    // A deferred head on a busy worker.
    w.adapter.busy = true;
    const head = send(w, dev, "wait for idle");
    await w.driver.tick();
    assert.equal(stateOf(w, head), "deferred");
    ok(await call(w.h, w.h.owner, "pause", [dev, "--reason", "hold"]));
    observe(dev, "blocked");
    const pmHead = w.h.core.enqueueMessage(ctx(w.h.core, w.h.owner), {
      recipientAgentId: w.h.pm.agentId,
      body: "x",
    }).messageId;
    ok(
      await call(w.h, w.h.owner, "pause", [
        w.h.pm.agentId,
        "--reason",
        "pm hold",
      ]),
    );
    for (let i = 0; i < 6; i += 1) {
      w.clock.now += 1_000_000;
      observe(dev, "blocked");
      observe(w.h.pm.agentId, "idle");
      const evaluation = advance();
      assert.deepEqual(evaluation.applied, []);
      assert.deepEqual(evaluation.actions, []);
      assert.deepEqual(evaluation.attention, []);
      assert.deepEqual(evaluation.stalledAgentIds, []);
    }
    assert.equal(stateOf(w, head), "deferred", "no expiry during the pause");
    assert.equal(stateOf(w, pmHead), "queued");
    const queued = w.h.core.messagesFor(w.h.pm.agentId).length;
    // Resume: the deferral clock restarts, so nothing expires at once.
    ok(await call(w.h, w.h.owner, "resume", [dev, "--reason", "go"]));
    ok(
      await call(w.h, w.h.owner, "resume", [w.h.pm.agentId, "--reason", "go"]),
    );
    observe(dev, "idle");
    assert.deepEqual(advance().applied, []);
    w.clock.now += 130_000;
    assert.deepEqual(advance().applied, [head]);
    assert.equal(stateOf(w, head), "expired");
    assert.ok(w.h.core.messagesFor(w.h.pm.agentId).length >= queued);
  } finally {
    await close(w.h);
  }
});

test("a lost agent that is paused is marked (paused) in the PM notice", async () => {
  const w = await world();
  try {
    ok(
      await call(w.h, w.h.owner, "pause", [
        w.h.developer.agentId,
        "--reason",
        "hold",
      ]),
    );
    w.h.core.recordAgentLost(ctx(w.h.core, w.h.owner), {
      agentId: w.h.developer.agentId,
    });
    const notice = w.h.core
      .messagesFor(w.h.pm.agentId)
      .map((m) => m.body)
      .find((body) => / is lost/.test(body));
    assert.match(notice ?? "", /\(paused\) is lost/);
  } finally {
    await close(w.h);
  }
});

test("pauses survive a restart of the controller database and delivery stays held", async () => {
  const w = await world();
  const { h } = w;
  const dev = h.developer.agentId;
  ok(await call(h, h.owner, "pause", ["--reason", "run hold"]));
  ok(await call(h, h.pm.credential, "pause", [dev, "--reason", "agent hold"]));
  const before = h.core.pauseState();
  await h.server.close();
  h.core.close();
  const reopened = await ControllerCore.open({
    stateDirectory: h.stateDirectory,
    project: h.info,
  });
  try {
    const after = reopened.pauseState();
    assert.deepEqual(after, before);
    assert.equal(after.run?.reason, "run hold");
    assert.equal(after.agents[0]?.reason, "agent hold");
    assert.equal(reopened.isDeliveryPaused(dev), true);
    assert.equal(reopened.isDeliveryPaused(h.pm.agentId), false);
    assert.equal(reopened.statusSnapshot().pause.run?.reason, "run hold");
    reopened.resumeAgent(ctx(reopened, h.owner), {
      agentId: dev,
      reason: "back",
    });
    assert.equal(
      reopened.isDeliveryPaused(dev),
      true,
      "the run pause still holds",
    );
    reopened.transitionRun(ctx(reopened, h.owner), "active", "back");
    assert.equal(reopened.isDeliveryPaused(dev), false);
  } finally {
    reopened.close();
  }
});

test("--interrupt reaches each working non-PM agent through the launcher once and is refused without prompt_relay", async () => {
  const interrupted: string[] = [];
  const launcher = {
    interrupt: async (agentId: string) => {
      interrupted.push(agentId);
      return agentId === "developer-agent";
    },
  };
  const disabled = await harness({ commands: { launcher: launcher as never } });
  try {
    const refused = await call(disabled, disabled.owner, "pause", [
      "--reason",
      "stop",
      "--interrupt",
    ]);
    assert.equal(code(refused), "not_configured");
    assert.equal(
      disabled.core.pauseState().run,
      null,
      "a refused interrupt pauses nothing",
    );
    assert.deepEqual(interrupted, []);
  } finally {
    await close(disabled);
  }
  const enabled = await harness({
    commands: {
      launcher: launcher as never,
      config: { promptRelay: { enabled: true } } as never,
    },
  });
  try {
    enabled.addMember("developer2", "Developer");
    const result = ok(
      await call(enabled, enabled.owner, "pause", [
        "--reason",
        "stop",
        "--interrupt",
      ]),
    ) as { interrupted: string[] };
    assert.deepEqual(interrupted.sort(), [
      "developer-agent",
      "developer2-agent",
    ]);
    assert.deepEqual(result.interrupted, ["developer-agent"]);
    assert.equal(
      code(
        await call(enabled, enabled.developer.credential, "pause", [
          "--reason",
          "x",
          "--interrupt",
        ]),
      ),
      "forbidden",
    );
    // resume takes no --interrupt
    assert.equal(
      code(
        await call(enabled, enabled.owner, "resume", [
          "--reason",
          "x",
          "--interrupt",
        ]),
      ),
      "invalid_request",
    );
  } finally {
    await close(enabled);
  }
});

test("status carries the pause with reason, actor and time, and the CLI prints PAUSED lines", async () => {
  const h = await harness();
  try {
    ok(await call(h, h.owner, "pause", ["--reason", "run hold"]));
    ok(
      await call(h, h.pm.credential, "pause", [
        h.developer.agentId,
        "--reason",
        "agent hold",
      ]),
    );
    const status = ok(await call(h, h.owner, "status")) as {
      pause: {
        run: { reason: string; actorId: string; pausedAt: string };
        agents: Array<{ agentId: string; reason: string }>;
      };
    };
    assert.equal(status.pause.run.reason, "run hold");
    assert.ok(status.pause.run.actorId.length > 0);
    assert.ok(!Number.isNaN(Date.parse(status.pause.run.pausedAt)));
    assert.equal(status.pause.agents[0]!.agentId, h.developer.agentId);
    const lines = pauseLines(
      status,
      Date.parse(status.pause.run.pausedAt) + 125_000,
    );
    assert.equal(lines.length, 2);
    assert.match(lines[0]!, /^PAUSED \(2m\): run hold by \S+$/);
    assert.match(
      lines[1]!,
      /^PAUSED \(2m\) developer-agent: agent hold by \S+$/,
    );
    assert.deepEqual(pauseLines({ pause: { run: null, agents: [] } }, 0), []);
  } finally {
    await close(h);
  }
});

const root = path.resolve(import.meta.dirname, "..", "..");
function matchesGolden(name: string, lines: readonly string[]): void {
  const file = path.join(root, "test", "golden", `${name}.txt`);
  const body = `${lines.join("\n")}\n`;
  if (process.env.UPDATE_GOLDEN === "1") {
    mkdirSync(path.dirname(file), { recursive: true });
    writeFileSync(file, body);
    return;
  }
  assert.ok(existsSync(file), `${file} is missing; run with UPDATE_GOLDEN=1`);
  assert.equal(body, readFileSync(file, "utf8"), name);
}

test("dash shows the run banner and a paused marker with age on agent rows (golden)", () => {
  const status = showcase({
    run: { state: "paused", stateVersion: 4 },
    pause: {
      run: {
        scope: "run",
        agentId: null,
        reason: "waiting for the user",
        actorId: "owner",
        pausedAt: iso(300),
      },
      agents: [
        {
          scope: "agent",
          agentId: "developer-2",
          reason: "inspect",
          actorId: "owner",
          pausedAt: iso(120),
        },
      ],
    },
  });
  const model = modelOf(status);
  assert.equal(model.header.runPause?.reason, "waiting for the user");
  const byId = new Map(model.agents.map((a) => [a.agentId, a]));
  assert.equal(byId.get("developer-2")!.pausedAt, iso(120));
  assert.equal(
    byId.get("developer-1")!.pausedAt,
    iso(300),
    "a worker is held by the run pause",
  );
  assert.equal(byId.get("pm-1")!.pausedAt, null, "the PM keeps receiving");
  const theme = makeTheme({ noColor: true, reducedMotion: true });
  const lines = plainLines(buildFrame(model, viewOf(120, 36), theme).lines);
  assert.match(lines.join("\n"), /PAUSED 5m: waiting for the user/);
  assert.match(lines.join("\n"), /paused 2m/);
  matchesGolden("dash-120x36-paused", lines);
  assert.ok(NOW > 0);
});

test("while the run is paused a one-off approval still runs, an auto-approved proposal is refused and recorded, and both run after resume", async () => {
  const w = await operatorWorld({
    operator: { maxPendingProposals: 30, autoApprove: ["ls -l"] },
  });
  try {
    const core = w.h.core;
    const decide = (id: string, sha: string, ...extra: string[]) =>
      call(w.h, w.h.pm.credential, "op", [
        "decide",
        id,
        "approve",
        "--hash",
        sha.slice(0, 12),
        ...extra,
      ]);
    ok(await call(w.h, w.h.owner, "pause", ["--reason", "audit"]));
    const oneOff = w.propose("echo one");
    ok(await decide(oneOff.proposalId, oneOff.commandSha));
    const grant = w.propose("echo two");
    ok(await decide(grant.proposalId, grant.commandSha, "--session", "exact"));
    const viaRule = w.propose("ls -l");
    assert.equal(viaRule.state, "approved");
    const viaGrant = w.propose("echo two");
    assert.equal(viaGrant.state, "approved");
    assert.notEqual(viaGrant.autoRule, null);
    await w.service.drain();
    await w.service.drain();
    assert.deepEqual(
      w.runs.map((r) => r.command),
      ["echo one", "echo two"].map(
        (c) => w.runs.find((r) => r.command === c)?.command,
      ),
    );
    assert.equal(w.runs.length, 2, "only the two one-off decisions ran");
    assert.equal(w.service.show(oneOff.proposalId)?.state, "finished");
    assert.equal(w.service.show(grant.proposalId)?.state, "finished");
    for (const refused of [viaRule, viaGrant]) {
      assert.equal(w.service.show(refused.proposalId)?.state, "cancelled");
      const events = rows<{ payload_json: string }>(
        w.h,
        "SELECT payload_json FROM controller_events WHERE entity_type = 'operator_proposal' AND entity_id = ?",
        refused.proposalId,
      ).map((row) => JSON.parse(row.payload_json).details);
      assert.ok(
        events.some(
          (d) => d?.reason === "run_paused" && /paused: audit/.test(d.error),
        ),
        "the refusal is recorded on the proposal",
      );
    }
    assert.ok(
      core
        .messagesFor(w.h.pm.agentId)
        .some((m) => /not run: the run is paused \(audit\)/.test(m.body)),
    );
    ok(await call(w.h, w.h.owner, "resume", ["--reason", "done"]));
    const after = w.propose("ls -l");
    await w.service.drain();
    await w.service.drain();
    assert.equal(w.service.show(after.proposalId)?.state, "finished");
  } finally {
    await w.stop();
  }
});

test("resume, status, observe, inbox, ack, send, wait, release, replace, pm-restart and op decide, cancel and show keep working while paused", async () => {
  for (const scope of ["run", "agent"] as const) {
    const w = await operatorWorld({ operator: { maxPendingProposals: 30 } });
    try {
      const h = w.h;
      const target = scope === "run" ? [] : [h.developer.agentId];
      ok(await call(h, h.owner, "pause", [...target, "--reason", "hold"]));
      const notPaused = (response: CommandResponse, name: string): void =>
        assert.doesNotMatch(text(response), /paused/, name);
      for (const [credential, name, args] of [
        [h.owner, "status", []],
        [h.pm.credential, "status", []],
        [h.developer.credential, "status", []],
        [h.pm.credential, "inbox", []],
        [h.developer.credential, "inbox", []],
        [h.owner, "inbox", [h.pm.agentId]],
        [h.developer.credential, "send", ["@pm", "question"]],
        [h.pm.credential, "observe", [h.developer.agentId]],
        [h.owner, "peek", [h.developer.agentId]],
        [h.pm.credential, "release", [h.developer.agentId]],
        [h.pm.credential, "replace", [h.developer.agentId]],
        [h.owner, "pm-restart", []],
      ] as const) {
        const response = await call(h, credential, name, [...args]);
        notPaused(response, `${scope} ${name}`);
      }
      // The PM's own wait and pull still hand it messages (a run pause never holds the PM).
      if (scope === "run") {
        for (const m of h.core
          .messagesFor(h.pm.agentId)
          .filter((m) => m.state === "sent"))
          h.core.ackMessage(ctx(h.core, h.pm.credential), m.messageId);
        h.core.enqueueMessage(ctx(h.core, h.owner), {
          recipientAgentId: h.pm.agentId,
          body: "fresh",
        });
        const wait = ok(await call(h, h.pm.credential, "wait")) as {
          timedOut: boolean;
        };
        assert.equal(wait.timedOut, false);
      }
      const first = h.core
        .messagesFor(h.pm.agentId)
        .find((m) => m.state === "sent");
      if (first !== undefined)
        notPaused(
          await call(h, h.pm.credential, "ack", [first.messageId]),
          "ack",
        );
      const proposal = w.propose("echo x");
      const second = w.propose("echo y");
      ok(await call(h, h.owner, "op", ["show", proposal.proposalId]));
      ok(await call(h, h.owner, "op", ["cancel", second.proposalId]));
      ok(
        await call(h, h.pm.credential, "op", [
          "decide",
          proposal.proposalId,
          "deny",
        ]),
      );
      ok(await call(h, h.owner, "resume", [...target, "--reason", "back"]));
    } finally {
      await w.stop();
    }
  }
});

test("a paused run refuses plan assign even with --early, before any dependency check", async () => {
  const h = await harness();
  try {
    ok(await call(h, h.owner, "pause", ["--reason", "audit in progress"]));
    assert.throws(
      () =>
        h.core.assignPackage(ctx(h.core, h.pm.credential), {
          planId: "p",
          packageId: "k",
          agentId: h.developer.agentId,
          early: "start before the dependency is accepted",
        }),
      (error: Error) =>
        error instanceof RunPausedError &&
        /audit in progress/.test(error.message),
    );
  } finally {
    await close(h);
  }
});

test("ending a paused agent closes its pause in the same transaction, and the id can be paused again", async () => {
  const h = await harness();
  try {
    const dev = h.developer.agentId;
    ok(await call(h, h.pm.credential, "pause", [dev, "--reason", "hold"]));
    assert.equal(h.core.statusSnapshot().pause.agents.length, 1);
    h.core.endAgent(ctx(h.core, h.owner), dev);
    assert.deepEqual(h.core.statusSnapshot().pause.agents, []);
    assert.equal(h.core.isDeliveryPaused(dev), false);
    const closed = rows<{ resume_reason: string; resumed_by: string }>(
      h,
      "SELECT resume_reason, resumed_by FROM pauses WHERE agent_id = ?",
      dev,
    );
    assert.equal(closed.length, 1);
    assert.equal(closed[0]!.resume_reason, "agent ended");
    const events = rows<{ payload_json: string }>(
      h,
      "SELECT payload_json FROM controller_events WHERE entity_type = 'agent' AND entity_id = ? ORDER BY sequence",
      dev,
    ).map((row) => JSON.parse(row.payload_json));
    assert.ok(
      events.some(
        (e) =>
          e.action === "agent.resume" && e.details.reason === "agent ended",
      ),
    );
    // Resume stays strict: an ended agent cannot be resumed.
    assert.equal(
      code(await call(h, h.owner, "resume", [dev, "--reason", "x"])),
      "rejected",
    );
    // A replacement agent with the same id would be pausable again: the open-pause index is free.
    const sentinel = rows<{ n: number }>(
      h,
      "SELECT COUNT(*) AS n FROM pauses WHERE agent_id = ? AND resumed_at IS NULL",
      dev,
    );
    assert.equal(sentinel[0]!.n, 0);
  } finally {
    await close(h);
  }
});
