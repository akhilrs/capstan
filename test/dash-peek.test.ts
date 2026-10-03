import assert from "node:assert/strict";
import { test } from "node:test";
import type { CommandDependencies } from "../src/commands.js";
import { OBSERVE_RATE_LIMIT } from "../src/observe.js";
import type { CommandResponse } from "../src/daemon.js";
import { call, close, ctx, harness } from "./harness.js";

type Launcher = NonNullable<CommandDependencies["launcher"]>;

function launcherStub(seen: Array<[string, number]>): Launcher {
  const unused = async (): Promise<never> => {
    throw new Error("not used by these tests");
  };
  return {
    launchPm: unused,
    restartPm: unused,
    spawn: unused,
    release: unused,
    replace: unused,
    observe: async (agentId: string, lines: number) => {
      seen.push([agentId, lines]);
      return {
        agentId,
        roleName: "developer",
        kind: "Developer",
        state: "active",
        agentStatus: "idle",
        text: "screen text",
      };
    },
    capturePrompt: unused,
    answerPrompt: unused,
    status: () => ({ cleanupFailed: [], orphanPanes: [] }),
  };
}

const code = (response: CommandResponse): string =>
  response.ok ? "ok" : response.code;

test("the operator reads an agent's screen through peek, with the same note and line rules as observe", async () => {
  const seen: Array<[string, number]> = [];
  const h = await harness({ commands: { launcher: launcherStub(seen) } });
  try {
    const answer = await call(h, h.owner, "peek", [h.developer.agentId, "25"]);
    assert.ok(answer.ok);
    const result = answer.result as Record<string, unknown>;
    assert.equal(result.text, "screen text");
    assert.match(String(result.note), /not verified/);
    assert.deepEqual(seen, [[h.developer.agentId, 25]]);
    await call(h, h.owner, "peek", [h.developer.agentId]);
    assert.deepEqual(seen[1], [h.developer.agentId, 40]);
  } finally {
    await close(h);
  }
});

test("peek is for the operator only, and observe keeps refusing the operator", async () => {
  const seen: Array<[string, number]> = [];
  const h = await harness({ commands: { launcher: launcherStub(seen) } });
  try {
    for (const credential of [h.pm.credential, h.developer.credential])
      assert.equal(
        code(await call(h, credential, "peek", [h.developer.agentId])),
        "forbidden",
      );
    assert.equal(
      code(await call(h, h.owner, "observe", [h.developer.agentId])),
      "forbidden",
    );
    assert.equal(
      code(await call(h, h.pm.credential, "observe", [h.developer.agentId])),
      "ok",
    );
    assert.deepEqual(seen, [[h.developer.agentId, 40]]);
  } finally {
    await close(h);
  }
});

test("peek refuses a bad agent id, bad line counts and a missing launcher, and rate-limits the operator", async () => {
  const seen: Array<[string, number]> = [];
  const h = await harness({ commands: { launcher: launcherStub(seen) } });
  const bare = await harness();
  try {
    for (const args of [[], ["a b"], ["x", "0"], ["x", "121"], ["x", "1", "2"]])
      assert.equal(
        code(await call(h, h.owner, "peek", args)),
        "invalid_request",
        JSON.stringify(args),
      );
    assert.equal(
      code(await call(bare, bare.owner, "peek", ["developer-agent"])),
      "not_configured",
    );
    for (let i = 0; i < OBSERVE_RATE_LIMIT; i += 1)
      assert.equal(code(await call(h, h.owner, "peek", ["x"])), "ok");
    const limited = await call(h, h.owner, "peek", ["x"]);
    assert.ok(!limited.ok);
    assert.match(limited.message, /observe_rate_limit/);
  } finally {
    await close(h);
    await close(bare);
  }
});

test("the operator's status carries the reason supervision is degraded; an agent's status does not", async () => {
  const h = await harness();
  try {
    const before = await call(h, h.owner, "status");
    assert.ok(before.ok);
    assert.equal(
      (before.result as Record<string, unknown>).supervisionReason,
      null,
    );
    h.core.markSupervisionDegraded(
      ctx(h.core, h.owner),
      "forced Supervisor evaluation failure",
    );
    const after = await call(h, h.owner, "status");
    assert.ok(after.ok);
    assert.equal(
      (after.result as Record<string, unknown>).supervisionReason,
      "forced Supervisor evaluation failure",
    );
    const agent = await call(h, h.pm.credential, "status");
    assert.ok(agent.ok);
    assert.ok(!("supervisionReason" in (agent.result as object)));
    assert.throws(() => h.core.supervisionReason(h.pm.credential));
  } finally {
    await close(h);
  }
});
