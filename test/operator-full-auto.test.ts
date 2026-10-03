import assert from "node:assert/strict";
import { test } from "node:test";
import type { RunOptions, RunResult } from "../src/command-runner.js";
import type { CommandResponse } from "../src/daemon.js";
import { restartNoticeToPm } from "../src/restart.js";
import { buildDashModel } from "../src/dash/model.js";
import { operatorLines } from "../src/watch.js";
import { call, close, harness } from "./harness.js";
import {
  OK_RESULT,
  operatorWorld,
  type OperatorWorld,
} from "./operator-harness.js";

function answered(response: CommandResponse): Record<string, unknown> {
  assert.equal(response.ok, true, JSON.stringify(response));
  return (response as unknown as { result: Record<string, unknown> }).result;
}

function refusal(response: CommandResponse): string {
  assert.equal(response.ok, false, "the call was expected to be refused");
  const failed = response as { code: string; message: string };
  return `${failed.code}: ${failed.message}`;
}

async function withWorld(
  run: (w: OperatorWorld) => Promise<void>,
  options: Parameters<typeof operatorWorld>[0] = {},
): Promise<void> {
  const w = await operatorWorld({
    ...options,
    operator: { maxPendingProposals: 30, ...options.operator },
  });
  try {
    await run(w);
  } finally {
    await w.stop();
  }
}

const op = (w: OperatorWorld, credential: string, ...args: string[]) =>
  call(w.h, credential, "op", args);

const on = (w: OperatorWorld, ...minutes: string[]) =>
  op(
    w,
    w.h.pm.credential,
    "full-auto",
    "on",
    ...minutes,
    "--asked-user",
    "yes, for thirty minutes",
  );

const bodiesOf = (w: OperatorWorld, agentId: string): string[] =>
  w.h.core.messagesFor(agentId).map((message) => message.body);

const eventDetails = (w: OperatorWorld, entityType: string) =>
  w.raw((db) =>
    (
      db
        .prepare(
          "SELECT payload_json FROM controller_events WHERE entity_type = ? ORDER BY sequence",
        )
        .all(entityType) as { payload_json: string }[]
    ).map(
      (row) =>
        (JSON.parse(row.payload_json) as { details: Record<string, unknown> })
          .details,
    ),
  );

test("B2 on: only an active PM, with the user's words and minutes in range, switches full auto on", async () => {
  await withWorld(async (w) => {
    assert.match(
      refusal(await op(w, w.h.owner, "full-auto", "on", "--asked-user", "yes")),
      /full_auto_requires_pm/,
    );
    assert.match(
      refusal(
        await op(
          w,
          w.h.developer.credential,
          "full-auto",
          "on",
          "--asked-user",
          "yes",
        ),
      ),
      /^forbidden/,
    );
    for (const args of [
      ["on"],
      ["on", "--asked-user"],
      ["on", "--asked-user", "  "],
      ["on", "5", "10", "--asked-user", "yes"],
    ])
      assert.match(
        refusal(await op(w, w.h.pm.credential, "full-auto", ...args)),
        /^invalid_request/,
      );
    for (const minutes of ["0", "121"])
      assert.match(refusal(await on(w, minutes)), /full_auto_minutes/);
    assert.deepEqual(
      answered(await op(w, w.h.pm.credential, "full-auto", "status")),
      {
        on: false,
      },
    );
  });
});

test("B2 on: the default minutes apply when none are given, and the status shows the time left", async () => {
  await withWorld(async (w) => {
    assert.deepEqual(answered(await on(w)), {
      on: true,
      minutes: 30,
      remainingSeconds: 1800,
    });
    w.clock.advance(10 * 60_000);
    for (const credential of [
      w.h.pm.credential,
      w.h.owner,
      w.h.developer.credential,
    ])
      assert.deepEqual(
        answered(await op(w, credential, "full-auto", "status")),
        { on: true, minutes: 30, remainingSeconds: 1200 },
      );
  });
});

test("B2 on: the PM and the Operator agent are told, and the ledger records the user's words", async () => {
  await withWorld(async (w) => {
    answered(await on(w, "45"));
    for (const agentId of [w.h.pm.agentId, w.h.developer.agentId])
      assert.ok(
        bodiesOf(w, agentId).some((body) =>
          /^FULL AUTO is ON for 45 minutes\./.test(body),
        ),
        agentId,
      );
    const [event] = eventDetails(w, "operator_full_auto");
    assert.equal(event?.event, "operator.full_auto_on");
    assert.equal(event?.minutes, 45);
    assert.equal(event?.askedUser, "yes, for thirty minutes");
  });
});

test("B2: while on, a proposal is approved at propose time with no allowlist, denylist or PM, pushes and deletes included, and the run is marked", async () => {
  await withWorld(async (w) => {
    answered(await on(w));
    for (const command of [
      "git push origin main",
      "rm -rf build",
      "echo hi | cat; whoami",
    ]) {
      const proposal = w.propose(command);
      assert.equal(proposal.state, "approved", command);
      assert.equal(proposal.autoRule, "full-auto");
      await w.service.drain();
      const done = w.service.show(proposal.proposalId)!;
      assert.equal(done.state, "finished");
      assert.equal(done.run?.fullAuto, true);
    }
    assert.deepEqual(
      w.runs.map((run) => run.command),
      ["git push origin main", "rm -rf build", "echo hi | cat; whoami"],
    );
    const proposalEvents = eventDetails(w, "operator_proposal").filter(
      (details) => details.fullAuto === true,
    );
    assert.ok(
      proposalEvents.length >= 6,
      "propose, claim and finish events carry full_auto",
    );
    const runs = w.raw((db) =>
      db
        .prepare("SELECT full_auto FROM operator_runs ORDER BY proposal_id")
        .all(),
    );
    assert.deepEqual(runs, [
      { full_auto: 1 },
      { full_auto: 1 },
      { full_auto: 1 },
    ]);
    assert.ok(
      bodiesOf(w, w.h.pm.agentId).some((body) => /\[full auto\]/.test(body)),
    );
    assert.ok(
      bodiesOf(w, w.h.pm.agentId).some((body) => /FULL AUTO is on/.test(body)),
    );
  });
});

test("B2: a run before full auto and a run after it are not marked", async () => {
  await withWorld(async (w) => {
    const before = w.propose("ls");
    w.approve(before);
    await w.service.drain();
    answered(await on(w));
    answered(await op(w, w.h.pm.credential, "full-auto", "off"));
    const after = w.propose("ls");
    assert.equal(after.state, "proposed");
    assert.equal(w.service.show(before.proposalId)?.run?.fullAuto, false);
  });
});

test("B2 off: the PM and the operator CLI switch it off at once, the Operator agent cannot, and both sides are told", async () => {
  await withWorld(async (w) => {
    answered(await on(w));
    assert.match(
      refusal(await op(w, w.h.developer.credential, "full-auto", "off")),
      /^forbidden/,
    );
    assert.deepEqual(answered(await op(w, w.h.owner, "full-auto", "off")), {
      on: false,
    });
    assert.equal(w.propose("git push").state, "proposed");
    for (const agentId of [w.h.pm.agentId, w.h.developer.agentId])
      assert.ok(
        bodiesOf(w, agentId).some((body) => /^FULL AUTO is OFF\./.test(body)),
      );
    answered(await on(w));
    answered(await op(w, w.h.pm.credential, "full-auto", "off"));
    assert.deepEqual(
      eventDetails(w, "operator_full_auto").map((details) => details.event),
      [
        "operator.full_auto_on",
        "operator.full_auto_off",
        "operator.full_auto_on",
        "operator.full_auto_off",
      ],
    );
    assert.deepEqual(answered(await op(w, w.h.owner, "full-auto", "off")), {
      on: false,
    });
    assert.equal(eventDetails(w, "operator_full_auto").length, 4);
  });
});

test("B2 expiry: the operator tick switches full auto off at the end of the time box and tells both sides", async () => {
  await withWorld(async (w) => {
    answered(await on(w, "10"));
    w.clock.advance(9 * 60_000 + 59_000);
    assert.equal(w.propose("echo a").autoRule, "full-auto");
    w.clock.advance(1000);
    await w.service.tick();
    assert.deepEqual(w.service.fullAutoStatus(), { on: false });
    assert.equal(w.propose("echo b").state, "proposed");
    assert.deepEqual(
      eventDetails(w, "operator_full_auto").map((details) => details.event),
      ["operator.full_auto_on", "operator.full_auto_expired"],
    );
    for (const agentId of [w.h.pm.agentId, w.h.developer.agentId])
      assert.ok(
        bodiesOf(w, agentId).some((body) =>
          /^FULL AUTO expired and is OFF\./.test(body),
        ),
      );
  });
});

test("B2: a proposal approved under full auto that has not started when it goes off is refused, not run", async () => {
  let release!: (result: RunResult) => void;
  const blocked = new Promise<RunResult>((resolve) => {
    release = resolve;
  });
  const seen: RunOptions[] = [];
  await withWorld(
    async (w) => {
      answered(await on(w));
      const first = w.propose("sleep 1");
      const second = w.propose("git push origin main");
      assert.equal(second.autoRule, "full-auto");
      await new Promise((resolve) => setImmediate(resolve));
      assert.equal(w.service.show(first.proposalId)?.state, "running");
      assert.equal(w.service.show(second.proposalId)?.state, "approved");
      answered(await op(w, w.h.owner, "full-auto", "off"));
      release(OK_RESULT);
      await w.service.drain();
      assert.equal(w.service.show(first.proposalId)?.state, "finished");
      assert.equal(w.service.show(second.proposalId)?.state, "cancelled");
      assert.equal(seen.length, 1);
      const details = eventDetails(w, "operator_proposal").find(
        (entry) => entry.reason === "full_auto_ended",
      );
      assert.ok(details);
      for (const agentId of [w.h.pm.agentId, w.h.developer.agentId])
        assert.ok(
          bodiesOf(w, agentId).some((body) =>
            /op-2 was not run: full auto ended before it started/.test(body),
          ),
          agentId,
        );
    },
    {
      runCommand: async (run) => {
        seen.push(run);
        return blocked;
      },
    },
  );
});

test("B2: a time box that ends before an approved proposal starts also refuses it", async () => {
  let release!: (result: RunResult) => void;
  const blocked = new Promise<RunResult>((resolve) => {
    release = resolve;
  });
  await withWorld(
    async (w) => {
      answered(await on(w, "5"));
      w.propose("sleep 1");
      const second = w.propose("rm -rf build");
      await new Promise((resolve) => setImmediate(resolve));
      w.clock.advance(5 * 60_000);
      release(OK_RESULT);
      await w.service.drain();
      assert.equal(w.service.show(second.proposalId)?.state, "cancelled");
      assert.deepEqual(w.service.fullAutoStatus(), { on: false });
    },
    { runCommand: async () => blocked },
  );
});

test("B2: full auto is not persisted: a restart leaves it off, records a startup event and the restart notice says so", async () => {
  await withWorld(async (w) => {
    answered(await on(w));
    await w.service.recover();
    // A new service starts with full auto off; the same service object proves the event, a new one the memory.
    const events = eventDetails(w, "operator_full_auto");
    assert.equal(events.at(-1)?.event, "operator.full_auto_off");
    assert.equal(events.at(-1)?.reason, "startup");
  });
  await withWorld(async (w) => {
    assert.deepEqual(w.service.fullAutoStatus(), { on: false });
    assert.equal(w.propose("echo a").state, "proposed");
  });
  for (const outcome of ["ok", "rolled_back", "down"] as const)
    assert.match(
      restartNoticeToPm("op-3", {
        outcome,
        reason: "r",
        manualRecovery: "m",
      } as Parameters<typeof restartNoticeToPm>[1]),
      /Full auto ended with the restart and is off/,
    );
});

test("B2: restart proposals under full auto are approved at propose time and still run through the restart code", async () => {
  const restarted: string[] = [];
  await withWorld(
    async (w) => {
      assert.equal(
        w.service.propose(w.h.developer.credential, {
          kind: "restart",
          command: "",
          reason: "new build",
          forceRestart: false,
        }).state,
        "proposed",
      );
      answered(await on(w));
      const restart = w.service.propose(w.h.developer.credential, {
        kind: "restart",
        command: "",
        reason: "new build",
        forceRestart: true,
      });
      assert.equal(restart.state, "approved");
      assert.equal(restart.autoRule, "full-auto");
      await w.service.drain();
      assert.deepEqual(restarted, [restart.proposalId]);
    },
    {
      service: {
        restart: async (proposal) => {
          restarted.push(proposal.proposalId);
        },
        restartPreflight: () => ({ warning: null }),
      },
    },
  );
});

test("B2: a clock jump of the wall clock does not change the time box (monotonic source)", async () => {
  let monotonic = 0;
  await withWorld(
    async (w) => {
      answered(await on(w, "10"));
      w.clock.advance(24 * 3600_000);
      monotonic += 5 * 60_000;
      assert.deepEqual(w.service.fullAutoStatus(), {
        on: true,
        minutes: 10,
        remainingSeconds: 300,
      });
      monotonic += 5 * 60_000;
      assert.deepEqual(w.service.fullAutoStatus(), { on: false });
    },
    { service: { monotonicNow: () => monotonic } },
  );
});

test("the dash status carries full auto and the grants for the operator, and nothing for others", async () => {
  await withWorld(async (w) => {
    answered(await on(w));
    const status = answered(await call(w.h, w.h.owner, "status", [])) as {
      operator?: { fullAuto: { on: boolean }; grants: unknown[] };
    };
    assert.equal(status.operator?.fullAuto.on, true);
    assert.deepEqual(status.operator?.grants, []);
    const agentStatus = answered(
      await call(w.h, w.h.pm.credential, "status", []),
    );
    assert.equal("operator" in agentStatus, false);
  });
});

test("AC6: watch and the dash model show the full auto time left and the grants, and nothing without an Operator", () => {
  const operator = {
    fullAuto: { on: true, minutes: 30, remainingSeconds: 1190 },
    grants: [
      { grantId: "grant-1", kind: "prefix", text: "ls -l", expiresAt: "t" },
    ],
  };
  assert.deepEqual(operatorLines(operator), [
    "FULL AUTO ON: 20 minutes left",
    'grant grant-1 (prefix) "ls -l" until t',
  ]);
  assert.deepEqual(operatorLines(undefined), []);
  assert.deepEqual(operatorLines({ fullAuto: { on: false }, grants: [] }), []);
  const base = { projectId: "p", run: { state: "active" } };
  const withOperator = buildDashModel({ ...base, operator }, 0, null).header;
  assert.equal(withOperator.fullAutoMinutes, 20);
  assert.equal(withOperator.grants, 1);
  const without = buildDashModel(base, 0, null).header;
  assert.equal("fullAutoMinutes" in without, false);
  assert.equal("grants" in without, false);
});

test("AC9: status carries no operator key when [operator] is absent", async () => {
  const h = await harness();
  try {
    const status = answered(await call(h, h.owner, "status", []));
    assert.equal("operator" in status, false);
  } finally {
    await close(h);
  }
});
