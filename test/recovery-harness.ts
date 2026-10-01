/**
 * The failure-injection harness for pane and process failures. A stand-in
 * "physical world" holds the panes and records every physical delivery by
 * instruction id (the message id). The controller side (ledger and delivery
 * driver) can be crashed and reopened on the same ledger while the world
 * stays, as a real daemon kill leaves the panes running.
 *
 * The oracle (`deliveries`, `retries`, `sentEvents`, `assertNoDuplicate`):
 * the number of physical deliveries of an instruction id is at most one plus
 * the number of `resolve retry` decisions the ledger holds for it, and the
 * count of typed sends is what the fault plan says.
 */
import assert from "node:assert/strict";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import Database from "better-sqlite3";
import { ControllerCore } from "../src/controller/core.js";
import type {
  HerdrState,
  MessagingTimers,
} from "../src/controller/messaging.js";
import type { InitialProject } from "../src/controller/types.js";
import { DeliveryDriver, type DriverAdapter } from "../src/driver.js";
import type { KeyLogger, SendOutcome } from "../src/herdr/adapter.js";
import { HerdrError } from "../src/herdr/runner.js";
import type { ChannelResult, Notifier } from "../src/notifier.js";
import { ctx, projectInfo, type Member } from "./harness.js";

export const TIMERS: MessagingTimers = {
  maxDeferralSeconds: 120,
  pmAckTimeoutSeconds: 600,
  pmNotifyAfterSeconds: 300,
  notifyIntervalSeconds: 600,
  stallAfterSeconds: 900,
  workerAckTimeoutSeconds: 600,
};

const FRAME = /^\[capstan message ([0-9a-f-]{36}) from /;

/** What the stand-in agent in a pane has received, in order, by instruction id. */
export interface PaneRecord {
  alive: boolean;
  readonly received: string[];
  state: HerdrState;
  /** An error observation throws instead of answering (a pane that is up but Herdr cannot read). */
  observeError: Error | undefined;
}

/** A crash point in the delivery of the next message. */
export type CrashPoint = "after_record_before_typing" | "after_typing";

export class World implements DriverAdapter {
  readonly panes = new Map<string, PaneRecord>();
  readonly #agentPane = new Map<string, string>();
  crashAt: CrashPoint | undefined;
  /** Called at a crash point; the harness closes the controller there, as a dying process would. */
  onCrash: (() => void) | undefined;
  typedSends = 0;

  addAgent(agentId: string): void {
    const paneId = `w1:${agentId}`;
    this.#agentPane.set(agentId, paneId);
    this.panes.set(paneId, {
      alive: true,
      received: [],
      state: "idle",
      observeError: undefined,
    });
  }

  pane(agentId: string): PaneRecord {
    return this.panes.get(this.#agentPane.get(agentId)!)!;
  }

  /** The pane is closed or its process is gone: Herdr no longer finds the agent. */
  kill(agentId: string): void {
    this.pane(agentId).alive = false;
  }

  paneForAgent(agentId: string): string | undefined {
    return this.#agentPane.get(agentId);
  }

  paneEntry(paneId: string): { agent?: string } | undefined {
    for (const [agent, id] of this.#agentPane)
      if (id === paneId) return { agent };
    return undefined;
  }

  async agentObservation(agentId: string): Promise<HerdrState> {
    const pane = this.pane(agentId);
    if (!pane.alive)
      throw new HerdrError("agent_not_found", "Herdr failed: agent_not_found");
    if (pane.observeError !== undefined) throw pane.observeError;
    return pane.state;
  }

  async guardedSend(input: {
    paneId: string;
    text: string;
    beforeSend: () => void | Promise<void>;
  }): Promise<SendOutcome> {
    const pane = this.panes.get(input.paneId)!;
    if (!pane.alive)
      throw new HerdrError("agent_not_found", "Herdr failed: agent_not_found");
    await input.beforeSend();
    if (this.crashAt === "after_record_before_typing") {
      this.crashAt = undefined;
      this.onCrash?.();
      throw new Error("the controller died before typing");
    }
    const id = FRAME.exec(input.text)?.[1];
    assert.ok(id, "a delivery carries a frame with the instruction id");
    pane.received.push(id);
    this.typedSends += 1;
    if (this.crashAt === "after_typing") {
      this.crashAt = undefined;
      this.onCrash?.();
      throw new Error("the controller died after typing");
    }
    return { sent: true };
  }

  async clearAfterDeferral(input: {
    paneId: string;
    deferredForMs: number;
    maxDeferralMs: number;
    discard: (text: string) => void | Promise<void>;
    log: KeyLogger;
  }): Promise<{ cleared: boolean; text: string }> {
    await input.discard("");
    return { cleared: true, text: "" };
  }

  /** Physical deliveries of an instruction id, over all panes. */
  deliveries(id: string): number {
    let count = 0;
    for (const pane of this.panes.values())
      count += pane.received.filter((received) => received === id).length;
    return count;
  }
}

class Silent implements Notifier {
  async send(): Promise<readonly ChannelResult[]> {
    return [{ channel: "herdr", ok: true }];
  }
  write(): void {}
}

export interface RecoveryWorld {
  readonly world: World;
  readonly stateDirectory: string;
  readonly info: InitialProject;
  readonly owner: string;
  readonly pm: Member;
  readonly developer: Member;
  core: ControllerCore;
  driver: DeliveryDriver;
  readonly events: Array<{ event: string; details: Record<string, unknown> }>;
  readonly clock: { now: number };
  tick(times?: number): Promise<void>;
  advance(ms: number): void;
  /** Queues an instruction from the PM (or the operator when `from` is "operator"). */
  send(to: string, body: string, from?: "pm" | "operator"): string;
  /** The recipient acknowledges. */
  ack(id: string, by?: Member): void;
  /** The controller dies and is started again on the same ledger; the panes keep running. */
  crash(): Promise<void>;
  state(id: string): string;
  /** Number of `resolve retry` decisions the ledger holds for the instruction. */
  retries(id: string): number;
  /** Number of `sent` transitions the ledger holds for the instruction. */
  sentEvents(id: string): number;
  /** Number of `agent.lost` events the ledger holds for the agent. */
  lostRows(agentId: string): number;
  /** Fails unless the physical deliveries never exceed one plus the recorded retries. */
  assertNoDuplicate(id: string): void;
  addWorker(name: string): Member;
  close(): void;
}

export async function recoveryWorld(): Promise<RecoveryWorld> {
  const stateDirectory = mkdtempSync(path.join(tmpdir(), "capstan-recovery-"));
  const info = projectInfo();
  const clock = { now: Date.parse("2026-10-01T12:00:00.000Z") };
  const open = (): Promise<ControllerCore> =>
    ControllerCore.open({
      stateDirectory,
      project: info,
      clock: () => new Date(clock.now),
    });
  let core = await open();
  const owner = info.ownerCredential;
  core.syncRoleDefinitions(ctx(core, owner), [
    { name: "pm", kind: "PM", host: "claude", configHash: "a".repeat(64) },
    {
      name: "developer",
      kind: "Developer",
      host: "claude",
      configHash: "b".repeat(64),
    },
  ]);
  const member = (name: string, kind: "PM" | "Developer"): Member => {
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
      roleName: kind === "PM" ? "pm" : "developer",
      seatId,
      actorId: actor.actorId,
    });
    return { agentId, credential: actor.credential, actorId: actor.actorId };
  };
  const pm = member("pm", "PM");
  const developer = member("developer", "Developer");
  const world = new World();
  world.addAgent(pm.agentId);
  world.addAgent(developer.agentId);
  const events: RecoveryWorld["events"] = [];
  const makeDriver = (): DeliveryDriver =>
    new DeliveryDriver({
      core,
      adapter: world,
      timers: TIMERS,
      notifier: new Silent(),
      credential: owner,
      now: () => clock.now,
      log: (event, details) => events.push({ event, details }),
    });
  const raw = <T>(run: (db: Database.Database) => T): T => {
    const db = new Database(path.join(stateDirectory, "controller.sqlite"), {
      readonly: true,
    });
    try {
      return run(db);
    } finally {
      db.close();
    }
  };
  const w: RecoveryWorld = {
    world,
    stateDirectory,
    info,
    owner,
    pm,
    developer,
    core,
    driver: makeDriver(),
    events,
    clock,
    async tick(times = 1) {
      for (let i = 0; i < times; i += 1) await w.driver.tick();
    },
    advance(ms) {
      clock.now += ms;
    },
    send(to, body, from = "pm") {
      return core.enqueueMessage(
        ctx(core, from === "pm" ? pm.credential : owner),
        { recipientAgentId: to, body },
      ).messageId;
    },
    ack(id, by = developer) {
      core.ackMessage(ctx(core, by.credential), id);
    },
    async crash() {
      await w.driver.stop();
      try {
        core.close();
      } catch {
        // already closed at the crash point
      }
      core = await open();
      w.core = core;
      w.driver = makeDriver();
    },
    state(id) {
      return core.message(id)!.state;
    },
    retries(id) {
      return raw(
        (db) =>
          (
            db
              .prepare(
                "SELECT COUNT(*) AS n FROM message_resolutions WHERE message_id = ? AND decision = 'retry'",
              )
              .get(id) as { n: number }
          ).n,
      );
    },
    sentEvents(id) {
      return raw(
        (db) =>
          (
            db
              .prepare(
                "SELECT COUNT(*) AS n FROM controller_events WHERE entity_type = 'message' AND entity_id = ? AND to_state = 'sent'",
              )
              .get(id) as { n: number }
          ).n,
      );
    },
    lostRows(agentId) {
      return raw(
        (db) =>
          (
            db
              .prepare(
                "SELECT COUNT(*) AS n FROM controller_events WHERE entity_type = 'agent' AND entity_id = ? AND to_state = 'lost'",
              )
              .get(agentId) as { n: number }
          ).n,
      );
    },
    assertNoDuplicate(id) {
      const delivered = world.deliveries(id);
      const allowed = 1 + w.retries(id);
      assert.ok(
        delivered <= allowed,
        `instruction ${id} was delivered ${delivered} times; the ledger allows ${allowed}`,
      );
    },
    addWorker(name) {
      core.syncRoleDefinitions(ctx(core, owner), [
        { name: "pm", kind: "PM", host: "claude", configHash: "a".repeat(64) },
        {
          name: "developer",
          kind: "Developer",
          host: "claude",
          configHash: "b".repeat(64),
        },
      ]);
      const created = member(name, "Developer");
      world.addAgent(created.agentId);
      return created;
    },
    close() {
      try {
        core.close();
      } catch {
        // closed by a crash point
      }
      rmSync(stateDirectory, { recursive: true, force: true });
    },
  };
  world.onCrash = () => core.close();
  return w;
}
