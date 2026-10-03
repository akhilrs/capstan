import assert from "node:assert/strict";
import { test } from "node:test";
import type { CommandResponse } from "../src/daemon.js";
import { call, close, ctx, harness } from "./harness.js";
import {
  commandConfig,
  operatorConfig,
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

const approveCall = (
  w: OperatorWorld,
  proposalId: string,
  hash: string,
  ...extra: string[]
) =>
  call(w.h, w.h.pm.credential, "op", [
    "decide",
    proposalId,
    "approve",
    "--hash",
    hash.slice(0, 12),
    ...extra,
  ]);

/** Proposes `command`, has the PM approve it with `session`, and waits for the run. */
async function grant(
  w: OperatorWorld,
  command: string,
  ...session: string[]
): Promise<Record<string, unknown>> {
  const proposal = w.propose(command);
  const result = answered(
    await approveCall(w, proposal.proposalId, proposal.commandSha, ...session),
  );
  await w.service.drain();
  return result;
}

const eventsOf = (w: OperatorWorld, entityType: string): string[] =>
  w.raw((db) =>
    (
      db
        .prepare(
          "SELECT payload_json FROM controller_events WHERE entity_type = ? ORDER BY sequence",
        )
        .all(entityType) as { payload_json: string }[]
    ).map(
      (row) =>
        (JSON.parse(row.payload_json) as { details: { event: string } }).details
          .event,
    ),
  );

test("B1 exact: an identical command runs with no new approval, any change needs one", async () => {
  await withWorld(async (w) => {
    const approved = await grant(w, "ls -l docs", "--session", "exact");
    assert.deepEqual(approved.sessionGrant, {
      grantId: "grant-1",
      kind: "exact",
      text: "ls -l docs",
      sourceProposalId: "op-1",
      createdAt: "2026-01-01T00:00:00.000Z",
      expiresAt: "2026-01-01T01:00:00.000Z",
      endedReason: null,
    });
    const again = w.propose("ls -l docs");
    assert.equal(again.state, "approved");
    assert.equal(again.autoRule, "session:grant-1");
    await w.service.drain();
    assert.equal(w.service.show(again.proposalId)?.state, "finished");
    assert.equal(w.runs.length, 2);
    for (const changed of ["ls -l doc", "ls -l docs ", "ls  -l docs"]) {
      const proposal = w.propose(changed);
      assert.equal(proposal.state, "proposed", changed);
      assert.equal(proposal.autoRule, null);
    }
    assert.equal(w.runs.length, 2);
  });
});

test("B1 exact: a restart is never granted", async () => {
  await withWorld(
    async (w) => {
      const restart = w.service.propose(w.h.developer.credential, {
        kind: "restart",
        command: "",
        reason: "new build",
        forceRestart: false,
      });
      assert.match(
        refusal(
          await approveCall(
            w,
            restart.proposalId,
            restart.commandSha,
            "--session",
            "exact",
          ),
        ),
        /session_not_for_restart/,
      );
      assert.equal(w.service.show(restart.proposalId)?.state, "proposed");
      assert.deepEqual(w.service.grants(), []);
    },
    { service: { restart: async () => undefined } },
  );
});

test("B1 prefix: whole-token matching, and extra tokens still go through the denylist", async () => {
  await withWorld(async (w) => {
    const approved = await grant(w, "ls -l docs", "--session", "prefix=ls -l");
    assert.equal((approved.sessionGrant as { text: string }).text, "ls -l");
    for (const command of [
      "ls -l",
      "ls -l docs",
      "ls -l docs/readme.md",
      "ls -l a b",
    ]) {
      const proposal = w.propose(command);
      assert.equal(proposal.state, "approved", command);
      assert.equal(proposal.autoRule, "session:grant-1");
    }
    for (const command of [
      "ls -la",
      "lsx -l",
      "ls",
      "ls -l ; echo hi",
      "ls -l | cat",
      "ls -l rm",
      "ls -l push",
      "ls -l -f",
      "ls -l --output=x",
      "ls -l\nwhoami",
      "ls -l $(whoami)",
    ]) {
      const proposal = w.propose(command);
      assert.equal(proposal.state, "proposed", command);
      assert.equal(proposal.autoRule, null, command);
    }
  });
});

test("B1 prefix: a prefix is refused when it holds a denylist word, is empty, is not simple or is too wide", async () => {
  await withWorld(async (w) => {
    const proposal = w.propose("git status");
    for (const [prefix, pattern] of [
      ["git status push", /always needs a human decision/],
      ["rm", /always needs a human decision/],
      ["git status -f", /always needs a human decision/],
      ["", /session_prefix_refused/],
      ["git status; ls", /one line of letters/],
      ["git status | cat", /one line of letters/],
      ["git", /name the subcommand/],
      ["git  status", /single spaces/],
      ["git status ", /single spaces/],
      ["FOO=1 git status", /assignment/],
      ["cstan", /name the subcommand/],
      ["git log", /does not start/ as unknown as RegExp],
    ] as const) {
      const message = refusal(
        await approveCall(
          w,
          proposal.proposalId,
          proposal.commandSha,
          "--session",
          `prefix=${prefix}`,
        ),
      );
      assert.match(message, /^rejected: session_prefix/, prefix);
      if (prefix !== "" && prefix !== "git log")
        assert.match(message, pattern, prefix);
      assert.equal(w.service.show(proposal.proposalId)?.state, "proposed");
    }
    assert.deepEqual(w.service.grants(), []);
    answered(
      await approveCall(
        w,
        proposal.proposalId,
        proposal.commandSha,
        "--session",
        "prefix=git status",
      ),
    );
    assert.equal(w.service.grants()[0]?.text, "git status");
  });
});

test("B1 session options are checked and only the PM approves with them", async () => {
  await withWorld(async (w) => {
    const proposal = w.propose("ls -l docs");
    for (const bad of [
      ["--session"],
      ["--session", "forever"],
      ["--session", "exact", "extra"],
      ["--force", "x"],
    ])
      assert.match(
        refusal(
          await approveCall(
            w,
            proposal.proposalId,
            proposal.commandSha,
            ...bad,
          ),
        ),
        /^invalid_request/,
      );
    assert.match(
      refusal(
        await call(w.h, w.h.owner, "op", [
          "decide",
          proposal.proposalId,
          "approve",
          "--hash",
          proposal.commandSha.slice(0, 12),
          "--session",
          "exact",
        ]),
      ),
      /approve_requires_pm/,
    );
    assert.equal(w.service.show(proposal.proposalId)?.state, "proposed");
    answered(
      await call(w.h, w.h.pm.credential, "op", [
        "decide",
        proposal.proposalId,
        "deny",
      ]),
    );
    assert.deepEqual(w.service.grants(), []);
  });
});

test("B1 end: release of the Operator agent ends its grants", async () => {
  await withWorld(async (w) => {
    await grant(w, "ls -l docs", "--session", "exact");
    assert.equal(w.service.grants().length, 1);
    w.h.core.endAgent(ctx(w.h.core, w.h.owner), w.h.developer.agentId);
    assert.deepEqual(w.service.grants(), []);
    assert.equal(w.service.listGrants()[0]?.endedReason, "released");
  });
});

test("B1 end: a controller restart ends every grant and a command after it needs approval", async () => {
  await withWorld(async (w) => {
    await grant(w, "ls -l docs", "--session", "exact");
    await w.service.recover();
    assert.deepEqual(w.service.grants(), []);
    assert.equal(w.service.listGrants()[0]?.endedReason, "restart");
    const again = w.propose("ls -l docs");
    assert.equal(again.state, "proposed");
    assert.deepEqual(eventsOf(w, "operator_grant"), [
      "operator.grant_created",
      "operator.grant_ended",
    ]);
  });
});

test("B1 end: the time cap ends a grant, with no help from the tick", async () => {
  await withWorld(
    async (w) => {
      await grant(w, "ls -l docs", "--session", "exact");
      w.clock.advance(29 * 60_000);
      assert.equal(w.propose("ls -l docs").autoRule, "session:grant-1");
      w.clock.advance(60_000);
      assert.deepEqual(w.service.grants(), []);
      assert.equal(w.propose("ls -l docs").state, "proposed");
      await w.service.tick();
      assert.equal(w.service.listGrants()[0]?.endedReason, "expired");
      assert.deepEqual(eventsOf(w, "operator_grant"), [
        "operator.grant_created",
        "operator.grant_used",
        "operator.grant_expired",
      ]);
      const notices = w.h.core
        .messagesFor(w.h.developer.agentId)
        .map((message) => message.body);
      assert.ok(
        notices.some((body) => /grant-1 reached its time cap/.test(body)),
      );
    },
    { operator: { sessionGrantMaxMinutes: 30 } },
  );
});

test("B1 end: revoke by the PM or the operator CLI ends a grant at once, a stranger cannot", async () => {
  await withWorld(async (w) => {
    await grant(w, "ls -l docs", "--session", "exact");
    assert.match(
      refusal(
        await call(w.h, w.h.developer.credential, "op", ["revoke", "grant-1"]),
      ),
      /^forbidden/,
    );
    const revoked = answered(
      await call(w.h, w.h.owner, "op", ["revoke", "grant-1"]),
    );
    assert.equal(revoked.endedReason, "revoked");
    assert.match(
      refusal(await call(w.h, w.h.pm.credential, "op", ["revoke", "grant-1"])),
      /grant_not_active/,
    );
    assert.match(
      refusal(await call(w.h, w.h.pm.credential, "op", ["revoke", "grant-9"])),
      /unknown_grant/,
    );
    assert.equal(w.propose("ls -l docs").state, "proposed");
    assert.deepEqual(eventsOf(w, "operator_grant"), [
      "operator.grant_created",
      "operator.grant_revoked",
    ]);
  });
});

test("B1 grants are listed for the PM and the operator CLI, and every use is audited", async () => {
  await withWorld(async (w) => {
    await grant(w, "ls -l docs", "--session", "prefix=ls -l");
    w.propose("ls -l a");
    const listed = answered(await call(w.h, w.h.owner, "op", ["grants"]));
    assert.equal((listed.grants as unknown[]).length, 1);
    answered(await call(w.h, w.h.pm.credential, "op", ["grants"]));
    assert.match(
      refusal(await call(w.h, w.h.developer.credential, "op", ["grants"])),
      /^forbidden/,
    );
    assert.deepEqual(eventsOf(w, "operator_grant"), [
      "operator.grant_created",
      "operator.grant_used",
    ]);
    const shown = answered(
      await call(w.h, w.h.pm.credential, "op", ["show", "op-1"]),
    );
    assert.equal((shown.sessionGrant as { text: string }).text, "ls -l");
    const notice = w.h.core
      .messagesFor(w.h.developer.agentId)
      .map((message) => message.body)
      .find((body) => body.includes("session grant grant-1"));
    assert.match(notice ?? "", /starts with the whole words/);
  });
});

test("a grant is checked again when the proposal is written: one that ended in between approves nothing", async () => {
  await withWorld(async (w) => {
    await grant(w, "ls -l docs", "--session", "exact");
    w.clock.advance(61 * 60_000);
    const record = w.h.core.proposeOperatorAction(
      ctx(w.h.core, w.h.developer.credential),
      {
        kind: "command",
        command: "ls -l docs",
        reason: "check it",
        autoRule: "session:grant-1",
        maxPending: 5,
      },
    );
    assert.equal(record.state, "proposed");
    assert.equal(record.autoRule, null);
  });
});

test("AC9: with [operator] absent or disabled the grant and full-auto commands answer not_configured", async () => {
  for (const config of [
    undefined,
    commandConfig(operatorConfig({ enabled: false })),
  ]) {
    const h = await harness(
      config === undefined ? {} : { commands: { config } },
    );
    try {
      for (const args of [
        ["grants"],
        ["revoke", "grant-1"],
        ["full-auto", "status"],
        ["full-auto", "off"],
        ["full-auto", "on", "--asked-user", "yes"],
      ])
        for (const credential of [h.owner, h.pm.credential])
          assert.match(
            refusal(await call(h, credential, "op", args)),
            /^not_configured/,
          );
    } finally {
      await close(h);
    }
  }
});

test("grant expiry follows the injected monotonic clock: the wall clock standing still cannot extend a grant", async () => {
  let monotonic = 0;
  await withWorld(
    async (w) => {
      await grant(w, "ls -l docs", "--session", "exact");
      monotonic += 29 * 60_000;
      assert.equal(w.propose("ls -l docs").autoRule, "session:grant-1");
      monotonic += 60_000;
      assert.deepEqual(w.service.grants(), []);
      assert.equal(w.propose("ls -l docs").state, "proposed");
      await w.service.tick();
      assert.equal(w.service.listGrants()[0]?.endedReason, "expired");
    },
    {
      operator: { sessionGrantMaxMinutes: 30 },
      service: { monotonicNow: () => monotonic },
    },
  );
});
