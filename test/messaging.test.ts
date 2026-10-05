import assert from "node:assert/strict";
import { test } from "node:test";
import {
  MESSAGE_STATES,
  PM_WAKE_MAX_INTERVAL_SECONDS,
  countedMillis,
  evaluateMessaging,
  isFinalState,
  isLegalTransition,
  isUnresolvedState,
  queueHead,
  resolutionTarget,
  suppressActiveStalls,
  type AgentFacts,
  type MessageFacts,
  type MessageState,
  type MessagingEvaluation,
  type MessagingTimers,
} from "../src/controller/messaging.js";

const SECOND = 1000;
const timers: MessagingTimers = {
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

const LEGAL: ReadonlyArray<[MessageState, MessageState]> = [
  ["queued", "deferred"],
  ["queued", "sent"],
  ["queued", "failed"],
  ["queued", "cancelled"],
  ["deferred", "sent"],
  ["deferred", "expired"],
  ["deferred", "failed"],
  ["deferred", "cancelled"],
  ["sent", "acked"],
  ["sent", "unacked"],
  ["sent", "cancelled"],
  ["unacked", "acked_late"],
  ["unacked", "cancelled"],
  ["expired", "cancelled"],
  ["failed", "cancelled"],
];

test("a transition is legal only from its defined source states", () => {
  const legal = new Set(LEGAL.map(([from, to]) => `${from}>${to}`));
  for (const from of MESSAGE_STATES)
    for (const to of MESSAGE_STATES)
      assert.equal(
        isLegalTransition(from, to),
        legal.has(`${from}>${to}`),
        `${from} to ${to}`,
      );
});

test("final and unresolved states partition the states around queued", () => {
  assert.deepEqual(MESSAGE_STATES.filter(isFinalState), [
    "acked",
    "acked_late",
    "cancelled",
  ]);
  assert.deepEqual(MESSAGE_STATES.filter(isUnresolvedState), [
    "deferred",
    "sent",
    "unacked",
    "expired",
    "failed",
  ]);
  assert.ok(!isFinalState("queued") && !isUnresolvedState("queued"));
  for (const state of MESSAGE_STATES.filter(isFinalState))
    for (const to of MESSAGE_STATES)
      assert.equal(isLegalTransition(state, to), false);
});

test("resolutions apply to the states the plan names", () => {
  for (const state of MESSAGE_STATES) {
    assert.equal(
      resolutionTarget("retry", state),
      isUnresolvedState(state) ? "queued" : undefined,
      `retry from ${state}`,
    );
    for (const decision of ["skip", "cancel"] as const)
      assert.equal(
        resolutionTarget(decision, state),
        isUnresolvedState(state) || state === "queued"
          ? "cancelled"
          : undefined,
        `${decision} from ${state}`,
      );
  }
});

test("the head of a queue is the oldest message that is not final", () => {
  const message = (sequence: number, state: MessageState) => ({
    sequence,
    state,
  });
  assert.equal(queueHead([]), undefined);
  assert.equal(
    queueHead([message(1, "acked"), message(2, "cancelled")]),
    undefined,
  );
  assert.equal(
    queueHead([message(3, "queued"), message(1, "acked"), message(2, "sent")])
      ?.sequence,
    2,
  );
  assert.equal(
    queueHead([message(2, "queued"), message(1, "expired")])?.sequence,
    1,
  );
});

test("the ack rule counts time when the agent is not working and all time inside a wait", () => {
  const observations = [
    { state: "idle" as const, atMs: 0 },
    { state: "working" as const, atMs: 100 * SECOND },
    { state: "idle" as const, atMs: 400 * SECOND },
  ];
  assert.equal(
    countedMillis("ack", 0, 100 * SECOND, observations, []),
    100 * SECOND,
  );
  assert.equal(
    countedMillis("ack", 100 * SECOND, 400 * SECOND, observations, []),
    0,
  );
  assert.equal(
    countedMillis("ack", 0, 500 * SECOND, observations, []),
    200 * SECOND,
  );
  const waits = [{ startMs: 150 * SECOND, endMs: 250 * SECOND }];
  assert.equal(
    countedMillis("ack", 100 * SECOND, 400 * SECOND, observations, waits),
    100 * SECOND,
  );
});

test("the stall rule counts only working time outside a wait", () => {
  const observations = [{ state: "working" as const, atMs: 0 }];
  assert.equal(
    countedMillis("stall", 0, 500 * SECOND, observations, []),
    500 * SECOND,
  );
  const waits = [{ startMs: 100 * SECOND, endMs: 300 * SECOND }];
  assert.equal(
    countedMillis("stall", 0, 500 * SECOND, observations, waits),
    300 * SECOND,
  );
  const open = [{ startMs: 100 * SECOND, endMs: null }];
  assert.equal(
    countedMillis("stall", 0, 5000 * SECOND, observations, open),
    100 * SECOND,
  );
  assert.equal(countedMillis("stall", 0, 500 * SECOND, [], []), 0);
});

test("counted time treats a reversed interval and an unobserved agent safely", () => {
  assert.equal(countedMillis("ack", 10, 5, [], []), 0);
  assert.equal(countedMillis("ack", 5, 5, [], []), 0);
  assert.equal(countedMillis("ack", 0, 1000, [], []), 1000);
});

function pm(overrides: Partial<AgentFacts> = {}): AgentFacts {
  return {
    agentId: "pm",
    kind: "PM",
    lastActivityMs: 0,
    observations: [{ state: "idle", atMs: 0 }],
    waits: [],
    paused: false,
    ...overrides,
  };
}

function worker(overrides: Partial<AgentFacts> = {}): AgentFacts {
  return { ...pm(), agentId: "dev", kind: "Developer", ...overrides };
}

function facts(overrides: Partial<MessageFacts>): MessageFacts {
  return {
    messageId: "m1",
    recipientAgentId: "pm",
    state: "sent",
    sequence: 1,
    queuedMs: 0,
    sentMs: 0,
    deferredMs: null,
    deferredReason: null,
    inputClearRecorded: false,
    lastNotifiedMs: null,
    wakeCount: 0,
    lastWakeMs: null,
    ...overrides,
  };
}

test("a PM message becomes unacked only after counted ack time reaches the timeout", () => {
  const working = pm({ observations: [{ state: "working", atMs: 0 }] });
  assert.deepEqual(
    evaluateMessaging([working], [facts({})], 5000 * SECOND, timers)
      .transitions,
    [],
  );
  const idle = pm();
  assert.deepEqual(
    evaluateMessaging([idle], [facts({})], 599 * SECOND, timers).transitions,
    [],
  );
  assert.deepEqual(
    evaluateMessaging([idle], [facts({})], 600 * SECOND, timers).transitions,
    [{ messageId: "m1", to: "unacked" }],
  );
});

test("time inside a registered wait counts toward the PM ack timeout", () => {
  const waiting = pm({
    observations: [{ state: "working", atMs: 0 }],
    waits: [{ startMs: 0, endMs: null }],
  });
  assert.equal(
    evaluateMessaging([waiting], [facts({})], 599 * SECOND, timers).transitions
      .length,
    0,
  );
  assert.deepEqual(
    evaluateMessaging([waiting], [facts({})], 600 * SECOND, timers).transitions,
    [{ messageId: "m1", to: "unacked" }],
  );
});

test("a worker message becomes unacked by wall-clock time", () => {
  const busy = worker({ observations: [{ state: "working", atMs: 0 }] });
  const message = facts({ recipientAgentId: "dev" });
  assert.equal(
    evaluateMessaging([busy], [message], 599 * SECOND, timers).transitions
      .length,
    0,
  );
  assert.deepEqual(
    evaluateMessaging([busy], [message], 600 * SECOND, timers).transitions,
    [{ messageId: "m1", to: "unacked" }],
  );
});

test("a deferred worker message expires when busy and clears input when the line is not empty", () => {
  const base = {
    recipientAgentId: "dev",
    state: "deferred" as const,
    sentMs: null,
    deferredMs: 0,
  };
  for (const reason of ["agent_busy", "agent_blocked"] as const)
    assert.deepEqual(
      evaluateMessaging(
        [worker()],
        [facts({ ...base, deferredReason: reason })],
        120 * SECOND,
        timers,
      ).transitions,
      [{ messageId: "m1", to: "expired" }],
    );
  const early = evaluateMessaging(
    [worker()],
    [facts({ ...base, deferredReason: "agent_busy" })],
    119 * SECOND,
    timers,
  );
  assert.deepEqual(early.transitions, []);
  const cleared = evaluateMessaging(
    [worker()],
    [facts({ ...base, deferredReason: "input_not_empty" })],
    120 * SECOND,
    timers,
  );
  assert.deepEqual(cleared.transitions, []);
  assert.deepEqual(cleared.actions, [
    { kind: "clear_then_send", messageId: "m1", notifyOperator: true },
  ]);
});

test("clear_then_send notifies the operator only until the input text has been recorded", () => {
  const base = {
    recipientAgentId: "dev",
    state: "deferred" as const,
    sentMs: null,
    deferredMs: 0,
    deferredReason: "input_not_empty" as const,
  };
  const before = evaluateMessaging(
    [worker()],
    [facts(base)],
    500 * SECOND,
    timers,
  );
  assert.deepEqual(before.actions, [
    { kind: "clear_then_send", messageId: "m1", notifyOperator: true },
  ]);
  const after = evaluateMessaging(
    [worker()],
    [facts({ ...base, inputClearRecorded: true })],
    500 * SECOND,
    timers,
  );
  assert.deepEqual(after.actions, [
    { kind: "clear_then_send", messageId: "m1", notifyOperator: false },
  ]);
});

test("the PM notification counts like the ack timer, repeats on the interval and only the head notifies", () => {
  const queued = facts({ state: "queued", sentMs: null });
  const later = facts({
    messageId: "m2",
    state: "queued",
    sequence: 2,
    sentMs: null,
  });
  const working = pm({ observations: [{ state: "working", atMs: 0 }] });
  assert.deepEqual(
    evaluateMessaging([working], [queued], 5000 * SECOND, timers).actions,
    [],
  );
  const inWait = pm({
    observations: [{ state: "working", atMs: 0 }],
    waits: [{ startMs: 0, endMs: null }],
  });
  assert.deepEqual(
    evaluateMessaging([inWait], [queued], 299 * SECOND, timers).actions,
    [],
  );
  assert.deepEqual(
    evaluateMessaging([inWait], [queued, later], 300 * SECOND, timers).actions,
    [{ kind: "notify_operator", messageId: "m1", repeat: false }],
  );
  const notified = facts({
    state: "queued",
    sentMs: null,
    lastNotifiedMs: 300 * SECOND,
  });
  assert.deepEqual(
    evaluateMessaging([pm()], [notified], 899 * SECOND, timers).actions,
    [],
  );
  assert.deepEqual(
    evaluateMessaging([pm()], [notified], 900 * SECOND, timers).actions,
    [{ kind: "notify_operator", messageId: "m1", repeat: true }],
  );
});

test("worker queues never raise the operator notification", () => {
  const message = facts({
    recipientAgentId: "dev",
    state: "queued",
    sentMs: null,
  });
  assert.deepEqual(
    evaluateMessaging([worker()], [message], 5000 * SECOND, timers).actions,
    [],
  );
});

test("stall timers skip registered waits, including an open one, and stop at the threshold", () => {
  const working = { state: "working" as const, atMs: 0 };
  const plain = worker({ observations: [working] });
  assert.deepEqual(
    evaluateMessaging([plain], [], 899 * SECOND, timers).stalledAgentIds,
    [],
  );
  assert.deepEqual(
    evaluateMessaging([plain], [], 900 * SECOND, timers).stalledAgentIds,
    ["dev"],
  );
  const waiting = worker({
    observations: [working],
    waits: [{ startMs: 10 * SECOND, endMs: null }],
  });
  assert.deepEqual(
    evaluateMessaging([waiting], [], 100_000 * SECOND, timers).stalledAgentIds,
    [],
  );
  const returned = worker({
    observations: [working],
    waits: [{ startMs: 10 * SECOND, endMs: 1000 * SECOND }],
  });
  assert.deepEqual(
    evaluateMessaging([returned], [], 1000 * SECOND, timers).stalledAgentIds,
    [],
  );
  assert.deepEqual(
    evaluateMessaging([returned], [], 1890 * SECOND, timers).stalledAgentIds,
    ["dev"],
  );
  const resting = worker({ observations: [{ state: "idle", atMs: 0 }] });
  assert.deepEqual(
    evaluateMessaging([resting], [], 100_000 * SECOND, timers).stalledAgentIds,
    [],
  );
});

test("evaluation never yields a send or a resend at any time", () => {
  const states: MessageState[] = [
    "queued",
    "deferred",
    "sent",
    "unacked",
    "expired",
    "failed",
  ];
  for (const state of states)
    for (let seconds = 0; seconds <= 200_000; seconds += 997) {
      const result = evaluateMessaging(
        [pm(), worker()],
        [
          facts({
            state,
            sentMs: 0,
            deferredMs: 0,
            deferredReason: "input_not_empty",
          }),
          facts({
            messageId: "w1",
            recipientAgentId: "dev",
            sequence: 2,
            state,
            sentMs: 0,
            deferredMs: 0,
            deferredReason: "agent_busy",
          }),
        ],
        seconds * SECOND,
        timers,
      );
      for (const action of result.actions)
        assert.ok(["clear_then_send", "notify_operator"].includes(action.kind));
      for (const transition of result.transitions)
        assert.ok(["unacked", "expired"].includes(transition.to));
    }
});

test("suppressActiveStalls drops a stalled agent with recent child activity, keeps an older one and never drops blocked attention", () => {
  const evaluation: MessagingEvaluation = {
    transitions: [{ messageId: "m1", to: "unacked" }],
    actions: [{ kind: "wake_pm", messageId: "m2" }],
    stalledAgentIds: ["dev-1", "dev-2", "dev-3"],
    attention: [
      { agentId: "dev-1", kind: "stalled", episodeMs: 1 },
      { agentId: "dev-2", kind: "stalled", episodeMs: 1 },
      { agentId: "dev-1", kind: "blocked", episodeMs: 1 },
    ],
  };
  const now = 10_000 * SECOND;
  const result = suppressActiveStalls(
    evaluation,
    new Map([
      ["dev-1", now - 899 * SECOND],
      ["dev-2", now - 900 * SECOND],
    ]),
    now,
    timers,
  );
  assert.deepEqual(result.stalledAgentIds, ["dev-2", "dev-3"]);
  assert.deepEqual(result.attention, [
    { agentId: "dev-2", kind: "stalled", episodeMs: 1 },
    { agentId: "dev-1", kind: "blocked", episodeMs: 1 },
  ]);
  assert.equal(result.transitions, evaluation.transitions);
  assert.equal(result.actions, evaluation.actions);
});

const WAKE_TIMERS: MessagingTimers = {
  ...timers,
  pmWakeAfterSeconds: 20,
  pmNotifyAfterSeconds: 100_000,
};
const wakeIds = (
  agents: AgentFacts[],
  messages: MessageFacts[],
  nowMs: number,
): string[] =>
  evaluateMessaging(agents, messages, nowMs, WAKE_TIMERS)
    .actions.filter((a) => a.kind === "wake_pm")
    .map((a) => a.messageId);

test("a PM whose head is sent or unacked still gets a wake for a later queued message", () => {
  for (const state of ["sent", "unacked"] as const)
    assert.deepEqual(
      wakeIds(
        [pm()],
        [
          facts({ messageId: "m1", sequence: 1, state }),
          facts({ messageId: "m2", sequence: 2, state: "queued" }),
        ],
        21 * SECOND,
      ),
      ["m2"],
      state,
    );
  assert.deepEqual(
    wakeIds([pm()], [facts({ messageId: "m1", state: "sent" })], 600 * SECOND),
    [],
    "a pulled message is never a reason to wake",
  );
  assert.deepEqual(
    wakeIds(
      [pm()],
      [
        facts({ messageId: "m3", sequence: 3, state: "queued" }),
        facts({ messageId: "m2", sequence: 2, state: "queued" }),
      ],
      21 * SECOND,
    ),
    ["m2"],
    "the oldest queued message is the target",
  );
});

test("wakes keep coming after 5 with the interval doubling up to the cap", () => {
  const expected = [120, 240, 480, 960, 1800, 1800, 1800];
  assert.equal(PM_WAKE_MAX_INTERVAL_SECONDS, 1800);
  expected.forEach((seconds, index) => {
    const lastWakeMs = 1_000_000 * SECOND;
    const message = facts({
      state: "queued",
      wakeCount: index + 1,
      lastWakeMs,
    });
    assert.deepEqual(
      wakeIds([pm()], [message], lastWakeMs + (seconds - 1) * SECOND),
      [],
      `wake ${index + 2} is not due early`,
    );
    assert.deepEqual(
      wakeIds([pm()], [message], lastWakeMs + seconds * SECOND),
      ["m1"],
      `wake ${index + 2} is due after ${seconds} s`,
    );
  });
});

test("no wake while the PM works, is blocked or unknown, or is paused", () => {
  const queued = [facts({ state: "queued" })];
  for (const state of ["working", "blocked", "unknown"] as const)
    assert.deepEqual(
      wakeIds(
        [pm({ observations: [{ state, atMs: 0 }] })],
        queued,
        600 * SECOND,
      ),
      [],
      state,
    );
  assert.deepEqual(
    wakeIds([pm({ observations: [] })], queued, 600 * SECOND),
    [],
  );
  assert.deepEqual(wakeIds([pm({ paused: true })], queued, 600 * SECOND), []);
  assert.deepEqual(wakeIds([pm()], queued, 600 * SECOND), ["m1"]);
});

test("the operator notification targets the oldest pending PM message, not the head", () => {
  const result = evaluateMessaging(
    [pm()],
    [
      facts({ messageId: "m1", sequence: 1, state: "deferred", sentMs: null }),
      facts({ messageId: "m2", sequence: 2, state: "sent" }),
      facts({ messageId: "m3", sequence: 3, state: "queued" }),
    ],
    400 * SECOND,
    { ...timers, pmAckTimeoutSeconds: 100_000 },
  );
  assert.deepEqual(
    result.actions.filter((a) => a.kind === "notify_operator"),
    [{ kind: "notify_operator", messageId: "m2", repeat: false }],
  );
});
