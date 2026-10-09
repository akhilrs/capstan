import assert from "node:assert/strict";
import { test } from "node:test";
import path from "node:path";
import { openSqlite, type Database } from "../src/controller/sqlite.js";
import { openDatabase } from "../src/controller/database.js";
import { promptHash, type CapturedPrompt } from "../src/herdr/prompt-relay.js";
import { close, ctx, harness, type Harness } from "./harness.js";

const OPTIONS = [
  { number: 1, text: "Yes", acceptsText: false, widensPermissions: false },
  { number: 2, text: "No", acceptsText: true, widensPermissions: true },
];

function prompt(agentId: string): CapturedPrompt {
  const base = {
    agentId,
    paneId: "w1:p1",
    hostKind: "claude",
    text: "Run it?",
    options: OPTIONS,
  };
  return { ...base, promptSha: promptHash(base) };
}

async function withLedger(
  run: (
    h: Harness,
    raw: <T>(use: (db: Database) => T) => T,
    clock: { now: number },
  ) => void | Promise<void>,
): Promise<void> {
  const clock = { now: Date.parse("2026-01-01T00:00:00.000Z") };
  const h = await harness({ clock: () => new Date(clock.now) });
  h.core.configurePromptRelay({ enabled: true, captureTtlSeconds: 600 });
  const raw = <T>(use: (db: Database) => T): T => {
    const db = openSqlite(path.join(h.stateDirectory, "controller.sqlite"));
    db.pragma("foreign_keys = ON");
    try {
      return use(db);
    } finally {
      db.close();
    }
  };
  try {
    await run(h, raw, clock);
  } finally {
    await close(h);
  }
}

const capture = (h: Harness) =>
  h.core.recordPromptCapture(ctx(h.core, h.pm.credential), {
    prompt: prompt(h.developer.agentId),
  });

test("migrations are registered through 0037 and every earlier migration is still recorded", async () => {
  await withLedger((h, raw) => {
    const versions = raw((db) =>
      db
        .prepare("SELECT version FROM schema_migrations ORDER BY version")
        .all(),
    ) as Array<{ version: number }>;
    assert.deepEqual(
      versions.map((entry) => entry.version),
      Array.from({ length: 37 }, (_, index) => index + 1),
    );
    assert.ok(h.core.stateVersion > 0);
  });
});

test("an existing ledger that lacks 0030 migrates, keeps its data and backfills the PM grant", async () => {
  await withLedger(async (h, raw) => {
    raw((db) => {
      db.exec(`ALTER TABLE agent_panes DROP COLUMN terminal_id;
        ALTER TABLE orphan_panes DROP COLUMN terminal_id;
        ALTER TABLE agent_panes DROP COLUMN task_ref;
        ALTER TABLE agent_panes DROP COLUMN task_title;
        ALTER TABLE messages DROP COLUMN action_needed;
        DROP TABLE pauses;
        DELETE FROM transition_rules WHERE role = 'PM' AND entity_type = 'run_control';
        DELETE FROM capability_grants WHERE capability = 'run:control' AND actor_id IN (SELECT actor_id FROM actors WHERE role = 'PM');
        DELETE FROM role_capabilities WHERE role = 'PM' AND capability = 'run:control';
        DROP INDEX actors_by_seat_active;
        DROP INDEX assignments_by_seat_authority;
        DELETE FROM schema_migrations WHERE version >= 30;
        DROP TABLE prompt_relays;
        DELETE FROM role_capabilities WHERE capability = 'prompt:relay';
        DELETE FROM capability_grants WHERE capability = 'prompt:relay';`);
    });
    const database = await openDatabase(
      path.join(h.stateDirectory, "controller.sqlite"),
    );
    try {
      const grants = database
        .prepare(
          "SELECT actor_id FROM capability_grants WHERE capability = 'prompt:relay' AND revoked_at IS NULL",
        )
        .all() as Array<{ actor_id: string }>;
      assert.deepEqual(
        grants.map((entry) => entry.actor_id),
        [h.pm.actorId],
      );
      assert.equal(
        (
          database
            .prepare(
              "SELECT COUNT(*) AS n FROM role_capabilities WHERE capability = 'prompt:relay' AND role = 'PM'",
            )
            .get() as { n: number }
        ).n,
        1,
      );
      assert.equal(
        (
          database.prepare("SELECT COUNT(*) AS n FROM prompt_relays").get() as {
            n: number;
          }
        ).n,
        0,
      );
    } finally {
      database.close();
    }
  });
});

test("only an active PM captures, and only with the relay enabled", async () => {
  await withLedger((h) => {
    assert.throws(() =>
      h.core.recordPromptCapture(ctx(h.core, h.developer.credential), {
        prompt: prompt(h.developer.agentId),
      }),
    );
    h.core.configurePromptRelay({ enabled: false, captureTtlSeconds: 600 });
    assert.throws(() => capture(h), /not_configured/);
    assert.equal(h.core.listPromptRelays().length, 0);
  });
});

test("a capture records the prompt, its options with their flags and a ttl, and checkPromptAnswer enforces expiry", async () => {
  await withLedger((h, _raw, clock) => {
    const record = capture(h);
    assert.equal(record.state, "captured");
    assert.deepEqual(record.options, OPTIONS);
    assert.equal(record.expiresAt, "2026-01-01T00:10:00.000Z");
    assert.equal(record.capturedByActorId, h.pm.actorId);
    const check = () =>
      h.core.checkPromptAnswer(record.relayId, record.hash12, {
        kind: "option",
        number: 2,
      });
    assert.equal(check().relayId, record.relayId);
    clock.now += 600_000;
    assert.throws(check, /capture_expired/);
    assert.deepEqual(h.core.expirePromptCaptures(ctx(h.core, h.owner)), [
      record.relayId,
    ]);
    assert.equal(h.core.promptRelay(record.relayId)?.outcomeReason, "expired");
  });
});

test("the full hash prefix rule: a short or wrong hash is refused, a longer prefix of the stored hash is accepted", async () => {
  await withLedger((h) => {
    const record = capture(h);
    for (const hash of ["", record.hash12.slice(0, 11), "f".repeat(12)])
      assert.throws(
        () =>
          h.core.checkPromptAnswer(record.relayId, hash, {
            kind: "esc",
          }),
        /hash_mismatch/,
      );
    assert.ok(
      h.core.checkPromptAnswer(record.relayId, record.promptSha, {
        kind: "esc",
      }),
    );
  });
});

test("beginPromptAnswer re-checks the hash inside its transaction and leaves the row captured on refusal", async () => {
  await withLedger((h) => {
    const record = capture(h);
    assert.throws(
      () =>
        h.core.beginPromptAnswer(ctx(h.core, h.pm.credential), {
          relayId: record.relayId,
          hash: "0".repeat(12),
          answer: { kind: "esc" },
        }),
      /hash_mismatch/,
    );
    assert.equal(h.core.promptRelay(record.relayId)?.state, "captured");
  });
});

test("the ledger refuses a backward state, a changed answer, a non-PM answerer, a changed capture and a delete", async () => {
  await withLedger((h, raw) => {
    const record = capture(h);
    const id = record.relayId;
    const update = (sql: string, ...values: unknown[]) =>
      raw((db) => db.prepare(sql).run(...values));
    // Identity and captured fields are immutable.
    for (const [column, value] of [
      ["prompt_text", "other"],
      ["prompt_sha", "0".repeat(64)],
      ["options_json", "[]"],
      ["agent_id", "x"],
      ["captured_by_actor_id", h.developer.actorId],
      ["expires_at", "2030-01-01T00:00:00.000Z"],
    ] as const)
      assert.throws(
        () => update(`UPDATE prompt_relays SET ${column} = ?`, value),
        /immutable/,
        column,
      );
    // A non-PM actor cannot be the answerer.
    assert.throws(
      () =>
        update(
          "UPDATE prompt_relays SET state = 'typing', answer_kind = 'esc', answer_widens_permissions = 0, answered_by_actor_id = ?",
          h.developer.actorId,
        ),
      /active PM actor/,
    );
    assert.equal(h.core.promptRelay(id)?.state, "captured");
    h.core.beginPromptAnswer(ctx(h.core, h.pm.credential), {
      relayId: id,
      hash: record.hash12,
      answer: { kind: "option", number: 2 },
    });
    // The recorded answer cannot change.
    for (const sql of [
      "UPDATE prompt_relays SET answer_option = 1",
      "UPDATE prompt_relays SET answer_kind = 'esc'",
      "UPDATE prompt_relays SET answer_widens_permissions = 0",
      "UPDATE prompt_relays SET answered_by_actor_id = NULL",
    ])
      assert.throws(() => update(sql), /final/, sql);
    // State cannot move backward or skip.
    assert.throws(
      () => update("UPDATE prompt_relays SET state = 'captured'"),
      /only moves forward/,
    );
    assert.throws(
      () => update("UPDATE prompt_relays SET state = 'expired'"),
      /only moves forward/,
    );
    h.core.finishPromptAnswer(ctx(h.core, h.owner), {
      relayId: id,
      outcome: { typed: true, keys: ["down", "enter"] },
    });
    for (const state of ["captured", "typing", "refused", "failed", "expired"])
      assert.throws(
        () => update("UPDATE prompt_relays SET state = ?", state),
        /only moves forward/,
        state,
      );
    assert.throws(
      () => update("UPDATE prompt_relays SET keys_json = '[]'"),
      /final/,
    );
    assert.throws(
      () => raw((db) => db.exec("DELETE FROM prompt_relays")),
      /immutable/,
    );
    assert.equal(h.core.promptRelay(id)?.state, "answered");
  });
});

test("a row is captured by a PM actor, starts captured, and an agent has one open row", async () => {
  await withLedger((h, raw) => {
    const record = capture(h);
    const insert = (actorId: string, state: string, relayId: string) =>
      raw((db) =>
        db
          .prepare(
            `INSERT INTO prompt_relays(project_id, relay_id, sequence, agent_id, pane_id, host_kind, prompt_text, options_json,
               prompt_sha, captured_by_actor_id, captured_at, expires_at, state)
             SELECT project_id, ?, 99, agent_id, pane_id, host_kind, prompt_text, options_json, prompt_sha, ?, captured_at, expires_at, ?
             FROM prompt_relays WHERE relay_id = ?`,
          )
          .run(relayId, actorId, state, record.relayId),
      );
    assert.throws(
      () => insert(h.developer.actorId, "captured", "relay-x"),
      /captured by a PM actor/,
    );
    assert.throws(
      () => insert(h.pm.actorId, "typing", "relay-x"),
      /starts captured/,
    );
    assert.throws(() => insert(h.pm.actorId, "captured", "relay-x"), /UNIQUE/);
  });
});

test("size limits are enforced by the table", async () => {
  await withLedger((h, raw) => {
    const record = capture(h);
    assert.throws(
      () =>
        h.core.recordPromptCapture(ctx(h.core, h.pm.credential), {
          prompt: { ...prompt(h.developer.agentId), text: "x".repeat(8193) },
        }),
      /at most 8192 bytes/,
    );
    assert.throws(
      () =>
        raw((db) =>
          db
            .prepare(
              "UPDATE prompt_relays SET state = 'refused', answer_kind = 'text', answer_option = 2, answer_widens_permissions = 1, answer_text = ?, answered_by_actor_id = ?, answered_at = 'x', outcome_reason = 'r', keys_json = '[]'",
            )
            .run("y".repeat(1001), h.pm.actorId),
        ),
      /CHECK/,
    );
    assert.equal(h.core.promptRelay(record.relayId)?.state, "captured");
  });
});
