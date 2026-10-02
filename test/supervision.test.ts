import assert from "node:assert/strict";
import { test } from "node:test";
import {
  SUPERVISOR_IDLE_MS,
  startSupervision,
  type SupervisionLauncher,
} from "../src/supervision.js";
import { close, ctx, harness, type Harness } from "./harness.js";

const sleep = (ms: number): Promise<void> =>
  new Promise((resolve) => setTimeout(resolve, ms));

async function until(check: () => boolean, label: string): Promise<void> {
  for (let i = 0; i < 200; i += 1) {
    if (check()) return;
    await sleep(10);
  }
  assert.fail(`timed out waiting for ${label}`);
}

interface Setup {
  readonly h: Harness;
  readonly spawns: string[];
  readonly released: string[];
  readonly log: Array<{ event: string; details: Record<string, unknown> }>;
  clock: { now: number };
  launcher: SupervisionLauncher;
  failSpawn: boolean;
}

async function setup(): Promise<Setup> {
  const h = await harness();
  const s: Setup = {
    h,
    spawns: [],
    released: [],
    log: [],
    clock: { now: Date.now() },
    failSpawn: false,
    launcher: {
      async spawn(role) {
        s.spawns.push(role);
        if (s.failSpawn) throw new Error("no room");
        h.addMember("supervisor", "Supervisor");
        return { state: "started" };
      },
      async release(agentId) {
        s.released.push(agentId);
        h.core.endAgent(ctx(h.core, h.owner), agentId);
      },
    },
  };
  return s;
}

function start(
  s: Setup,
  overrides: {
    enabled?: boolean;
    role?: string | undefined;
    checkSeconds?: number;
  } = {},
) {
  return startSupervision({
    core: s.h.core,
    launcher: s.launcher,
    credential: s.h.owner,
    supervision: {
      enabled: overrides.enabled ?? true,
      checkSeconds: overrides.checkSeconds ?? 300,
    },
    supervisorRole: "role" in overrides ? overrides.role : "supervisor",
    intervalMs: 5,
    now: () => s.clock.now,
    log: (event, details) => s.log.push({ event, details }),
  });
}

const checksTo = (s: Setup, agentId: string) =>
  s.h.core
    .messagesFor(agentId)
    .filter((m) => m.body.startsWith("Routine check"));

test("with no active worker the Supervisor is not started", async () => {
  const s = await setup();
  try {
    s.h.core.endAgent(ctx(s.h.core, s.h.owner), s.h.developer.agentId);
    const handle = start(s);
    await sleep(60);
    assert.deepEqual(s.spawns, []);
    await handle.stop();
  } finally {
    await close(s.h);
  }
});

test("the Supervisor is spawned from its role when none runs, then checked, and not spawned twice", async () => {
  const s = await setup();
  try {
    const handle = start(s);
    await until(() => s.spawns.length === 1, "the spawn");
    const supervisor = s.h.core
      .listAgents()
      .find((a) => a.kind === "Supervisor")!;
    await until(
      () => checksTo(s, supervisor.agentId).length === 1,
      "the first check",
    );
    await sleep(60);
    assert.deepEqual(s.spawns, ["supervisor"]);
    assert.equal(
      checksTo(s, supervisor.agentId).length,
      1,
      "one check per interval",
    );
    await handle.stop();
  } finally {
    await close(s.h);
  }
});

test("a failed spawn is logged and tried again only after a minute", async () => {
  const s = await setup();
  try {
    s.failSpawn = true;
    const handle = start(s);
    await until(() => s.spawns.length === 1, "the first try");
    await sleep(80);
    assert.equal(s.spawns.length, 1, "no retry within the minute");
    assert.ok(s.log.some((e) => e.event === "supervisor_spawn_failed"));
    s.clock.now += 61_000;
    s.failSpawn = false;
    await until(() => s.spawns.length === 2, "the retry");
    await handle.stop();
  } finally {
    await close(s.h);
  }
});

test("it does nothing when supervision is off, the role is not configured or no PM is active", async () => {
  for (const overrides of [{ enabled: false }, { role: undefined }]) {
    const s = await setup();
    try {
      const handle = start(s, overrides);
      await sleep(60);
      assert.deepEqual(s.spawns, [], JSON.stringify(overrides));
      await handle.stop();
    } finally {
      await close(s.h);
    }
  }
  const s = await setup();
  try {
    s.h.core.endAgent(ctx(s.h.core, s.h.owner), s.h.pm.agentId);
    const handle = start(s);
    await sleep(60);
    assert.deepEqual(s.spawns, [], "no PM");
    await handle.stop();
  } finally {
    await close(s.h);
  }
});

test("a Supervisor with no active worker for ten minutes is released, and not before", async () => {
  const s = await setup();
  try {
    const supervisor = s.h.addMember("supervisor", "Supervisor");
    s.h.core.endAgent(ctx(s.h.core, s.h.owner), s.h.developer.agentId);
    const handle = start(s);
    await sleep(60);
    assert.deepEqual(s.released, [], "not before ten minutes");
    s.clock.now += SUPERVISOR_IDLE_MS + 1000;
    await until(() => s.released.length === 1, "the release");
    assert.deepEqual(s.released, [supervisor.agentId]);
    await handle.stop();
  } finally {
    await close(s.h);
  }
});

test("no tick runs after stop", async () => {
  const s = await setup();
  try {
    const handle = start(s);
    await handle.stop();
    const before = s.spawns.length;
    await sleep(40);
    assert.equal(s.spawns.length, before);
  } finally {
    await close(s.h);
  }
});
