import assert from "node:assert/strict";
import {
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  statSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { test } from "node:test";
import {
  NOTIFICATION_LOG_MAX_BYTES,
  createNotifier,
  type NotificationRequest,
} from "../src/notifier.js";

const REQUEST: NotificationRequest = {
  kind: "pm_message",
  messageId: "m-1",
  recipientAgentId: "pm-agent",
  repeat: false,
};

function setup(channels: { herdr: boolean; fallback: boolean }, fail = false) {
  const directory = mkdtempSync(path.join(tmpdir(), "capstan-notifier-"));
  const calls: Array<{ title: string; body: string }> = [];
  const logs: Array<{ event: string; details: Record<string, unknown> }> = [];
  const recordPath = path.join(directory, "notifications.jsonl");
  const notifier = createNotifier({
    adapter: {
      notify: async (title, body) => {
        calls.push({ title, body });
        if (fail) throw new Error("herdr is down");
      },
    },
    channels,
    recordPath,
    now: () => new Date("2026-01-01T00:00:00.000Z"),
    log: (event, details) => logs.push({ event, details }),
  });
  return {
    notifier,
    calls,
    logs,
    recordPath,
    directory,
    lines: () =>
      existsSync(recordPath)
        ? readFileSync(recordPath, "utf8")
            .trim()
            .split("\n")
            .map((line) => JSON.parse(line) as Record<string, unknown>)
        : [],
    cleanup: () => rmSync(directory, { recursive: true, force: true }),
  };
}

test("with the herdr channel only, the adapter is called and one herdr line is written", async () => {
  const n = setup({ herdr: true, fallback: false });
  try {
    const results = await n.notifier.send(REQUEST);
    assert.deepEqual(results, [{ channel: "herdr", ok: true }]);
    assert.equal(n.calls.length, 1);
    assert.equal(n.calls[0]!.title, "Capstan: PM message waiting");
    n.notifier.write(REQUEST, results, true);
    assert.deepEqual(n.lines(), [
      {
        ts: "2026-01-01T00:00:00.000Z",
        messageId: "m-1",
        channel: "herdr",
        ok: true,
        kind: "pm_message",
        repeat: false,
        recorded: true,
      },
    ]);
    assert.equal(statSync(n.recordPath).mode & 0o777, 0o600);
  } finally {
    n.cleanup();
  }
});

test("with the fallback only, the adapter is never called and a fallback line is written", async () => {
  const n = setup({ herdr: false, fallback: true });
  try {
    const results = await n.notifier.send(REQUEST);
    assert.deepEqual(results, [{ channel: "fallback", ok: true }]);
    assert.equal(n.calls.length, 0);
    n.notifier.write(REQUEST, results, true);
    assert.equal(n.lines().length, 1);
    assert.equal(n.lines()[0]!.channel, "fallback");
  } finally {
    n.cleanup();
  }
});

test("a failing herdr channel is never silent: an ok:false line, a log entry, and the fallback still counts", async () => {
  const n = setup({ herdr: true, fallback: true }, true);
  try {
    const results = await n.notifier.send(REQUEST);
    assert.deepEqual(results, [
      { channel: "herdr", ok: false },
      { channel: "fallback", ok: true },
    ]);
    assert.equal(n.logs[0]!.event, "notification_channel_failed");
    assert.equal(n.logs[0]!.details.channel, "herdr");
    n.notifier.write(REQUEST, results, false);
    assert.deepEqual(
      n.lines().map((l) => [l.channel, l.ok, l.recorded]),
      [
        ["herdr", false, false],
        ["fallback", true, false],
      ],
    );
  } finally {
    n.cleanup();
  }
});

test("titles and bodies follow the kind and never carry the cleared text", async () => {
  const n = setup({ herdr: true, fallback: false });
  try {
    await n.notifier.send({ ...REQUEST, kind: "input_cleared", detail: "17" });
    await n.notifier.send({
      ...REQUEST,
      kind: "delivery_stuck",
      detail: "pane_mismatch",
    });
    await n.notifier.send({ ...REQUEST, repeat: true });
    assert.deepEqual(
      n.calls.map((c) => c.title),
      [
        "Capstan: input line cleared",
        "Capstan: delivery stuck",
        "Capstan: PM message waiting",
      ],
    );
    assert.match(n.calls[0]!.body, /Cleared 17 characters/);
    assert.match(n.calls[1]!.body, /stuck: pane_mismatch/);
    assert.match(n.calls[2]!.body, /reminder/);
  } finally {
    n.cleanup();
  }
});

test("the record file rotates over its cap so repeated notices cannot grow it without bound", async () => {
  const n = setup({ herdr: false, fallback: true });
  try {
    writeFileSync(n.recordPath, "x".repeat(NOTIFICATION_LOG_MAX_BYTES + 1), {
      mode: 0o600,
    });
    n.notifier.write(REQUEST, [{ channel: "fallback", ok: true }], true);
    assert.equal(n.lines().length, 1);
    assert.equal(
      readFileSync(`${n.recordPath}.1`, "utf8").length,
      NOTIFICATION_LOG_MAX_BYTES + 1,
    );
  } finally {
    n.cleanup();
  }
});

test("a record file that cannot be written is logged, never thrown", () => {
  const n = setup({ herdr: false, fallback: true });
  try {
    mkdirSync(n.recordPath);
    n.notifier.write(REQUEST, [{ channel: "fallback", ok: true }], true);
    assert.equal(n.logs.at(-1)!.event, "notification_record_failed");
  } finally {
    n.cleanup();
  }
});
