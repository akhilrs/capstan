/** Operator status responses in the shape the daemon returns, for the dashboard tests. */
export type Status = Record<string, unknown>;

export const NOW = Date.parse("2026-10-02T12:00:00Z");
export const iso = (secondsAgo: number): string =>
  new Date(NOW - secondsAgo * 1000).toISOString();

export function healthy(overrides: Status = {}): Status {
  return {
    projectId: "p1",
    run: { state: "active", stateVersion: 3 },
    supervision: {
      enabled: true,
      health: "healthy",
      targetEpoch: 4,
      checkpointEpoch: 4,
      checkpointAssignmentId: null,
      replacementAttempts: 0,
    },
    supervisionReason: null,
    agents: [
      {
        agentId: "pm-agent",
        roleName: "pm",
        kind: "PM",
        generation: 1,
        state: "active",
        lastActivityAt: iso(500),
      },
      {
        agentId: "developer-agent",
        roleName: "developer",
        kind: "Developer",
        generation: 2,
        state: "active",
        lastActivityAt: iso(5),
      },
    ],
    messages: [],
    messagesTruncated: false,
    stalledAgentIds: [],
    lostAgentIds: [],
    stuck: [],
    inputClears: [],
    panes: [{ agentId: "developer-agent", paneId: "w1:p2" }],
    reviews: [],
    integrations: [],
    agentFindings: [],
    reports: [],
    cleanupFailed: [],
    orphanPanes: [],
    work: [],
    ...overrides,
  };
}

export function message(
  id: string,
  state: string,
  overrides: Status = {},
): Status {
  return {
    messageId: id,
    recipientAgentId: "developer-agent",
    state,
    sequence: 1,
    queuedAt: iso(60),
    deferredReason: null,
    stateReason: null,
    lastNotifiedAt: null,
    ...overrides,
  };
}

export function degraded(): Status {
  return healthy({
    supervision: {
      enabled: true,
      health: "degraded",
      targetEpoch: 4,
      checkpointEpoch: null,
      checkpointAssignmentId: null,
      replacementAttempts: 1,
    },
    supervisionReason: "forced Supervisor evaluation failure",
  });
}

export function troubled(): Status {
  return healthy({
    messages: [
      message("m-stuck", "sent", { sequence: 1 }),
      message("m-failed", "failed", { sequence: 2, stateReason: "pane_gone" }),
      message("m-fine", "deferred", {
        sequence: 3,
        deferredReason: "agent_busy",
      }),
    ],
    stuck: [{ messageId: "m-stuck", reason: "pane_mismatch" }],
    stalledAgentIds: ["developer-agent"],
    lostAgentIds: ["gone-agent"],
    agentFindings: [
      {
        findingId: "f-1234567890",
        targetAgentId: "developer-agent",
        severity: "high",
        state: "escalated",
        interventions: 2,
        stateReason: "no ack",
      },
      {
        findingId: "f-closed",
        targetAgentId: "developer-agent",
        severity: "low",
        state: "resolved",
        interventions: 1,
        stateReason: null,
      },
    ],
    reports: [
      {
        reportId: "r1",
        agentId: "developer-agent",
        commitSha: "0123456789abcdef0123456789abcdef01234567",
        state: "accepted",
        createdAt: iso(300),
      },
      {
        reportId: "r2",
        agentId: "developer-agent",
        commitSha: "fedcba9876543210fedcba9876543210fedcba98",
        state: "rejected",
        createdAt: iso(100),
      },
    ],
    reviews: [
      {
        reviewId: "v1",
        authorAgentId: "developer-agent",
        round: 1,
        state: "started",
        createdAt: iso(200),
      },
    ],
    integrations: [
      { integrationId: "i-0123456789", state: "merged", createdAt: iso(50) },
    ],
    orphanPanes: [{ agentId: "x-agent", paneId: "w1:p9" }],
    cleanupFailed: [{ agentId: "y-agent", reason: "worktree not removable" }],
  });
}

const agent = (
  agentId: string,
  kind: string,
  generation: number,
  state: string,
  secondsAgo: number,
): Status => ({
  agentId,
  roleName: kind.toLowerCase(),
  kind,
  generation,
  state,
  lastActivityAt: iso(secondsAgo),
});

/** The sample run used by the golden screens: a PM, a Supervisor, three workers (one stalled), a stuck message, an escalated finding and a degraded reason. */
export function showcase(overrides: Status = {}): Status {
  const report = (
    id: string,
    who: string,
    sha: string,
    state: string,
    ago: number,
  ) => ({
    reportId: id,
    agentId: who,
    commitSha: sha.padEnd(40, "0"),
    state,
    createdAt: iso(ago),
  });
  const review = (
    id: string,
    who: string,
    round: number,
    state: string,
    ago: number,
  ) => ({
    reviewId: id,
    authorAgentId: who,
    round,
    state,
    createdAt: iso(ago),
  });
  const integration = (id: string, state: string, ago: number) => ({
    integrationId: id,
    state,
    createdAt: iso(ago),
  });
  return healthy({
    projectId: "capstan",
    supervision: {
      enabled: true,
      health: "degraded",
      targetEpoch: 4,
      checkpointEpoch: 4,
      checkpointAssignmentId: null,
      replacementAttempts: 0,
    },
    supervisionReason: "lost contact with developer-2 (pane p4 not responding)",
    agents: [
      agent("pm-1", "PM", 1, "active", 95),
      agent("supervisor-1", "Supervisor", 1, "active", 45),
      agent("developer-1", "Developer", 2, "active", 3),
      agent("developer-2", "Developer", 1, "active", 221),
      agent("designer-1", "Designer", 1, "active", 1),
      agent("developer-0", "Developer", 1, "ended", 840),
    ],
    panes: [
      { agentId: "pm-1", paneId: "p1" },
      { agentId: "supervisor-1", paneId: "p2" },
      { agentId: "developer-1", paneId: "p3" },
      { agentId: "developer-2", paneId: "p4" },
      { agentId: "designer-1", paneId: "p5" },
    ],
    messages: [
      message("0192f4c1-3a7e-7b2d", "unacked", {
        sequence: 41,
        recipientAgentId: "developer-2",
        queuedAt: iso(221),
        lastNotifiedAt: iso(219),
        stateReason: "no acknowledgement after 3 attempts",
      }),
      message("0192f4c9-61d0-7c13", "queued", {
        sequence: 42,
        recipientAgentId: "developer-2",
        queuedAt: iso(190),
        deferredReason: "recipient busy",
      }),
      message("0192f510-0b4e-79aa", "sent", {
        sequence: 44,
        recipientAgentId: "developer-1",
        queuedAt: iso(8),
        lastNotifiedAt: iso(7),
      }),
      message("0192f511-7c20-70f4", "sent", {
        sequence: 45,
        recipientAgentId: "designer-1",
        queuedAt: iso(2),
        lastNotifiedAt: iso(2),
      }),
      message("0192f511-b9e2-71c8", "queued", {
        sequence: 46,
        recipientAgentId: "pm-1",
        queuedAt: iso(1),
      }),
    ],
    stuck: [
      {
        messageId: "0192f4c1-3a7e-7b2d",
        reason: "no acknowledgement after 3 attempts",
      },
    ],
    stalledAgentIds: ["developer-2"],
    inputClears: [{ messageId: "0192f3aa-0000", recordedAt: iso(600) }],
    agentFindings: [
      {
        findingId: "0192a1-aaaa",
        targetAgentId: "developer-2",
        severity: "high",
        state: "escalated",
        interventions: 2,
        stateReason: "no progress after 2 interventions",
      },
      {
        findingId: "0192a4-bbbb",
        targetAgentId: "developer-1",
        severity: "low",
        state: "open",
        interventions: 0,
        stateReason: "scope drift: edits files outside the task",
      },
    ],
    reports: [
      report("r1", "designer-1", "8bf60fa", "accepted", 72),
      report("r2", "developer-1", "9abf487", "accepted", 180),
      report("r3", "developer-1", "45fe0de", "rejected", 660),
      report("r4", "developer-2", "fb4a3d5", "accepted", 900),
      report("r5", "developer-0", "5451671", "accepted", 1560),
      report("r6", "developer-0", "3f1d9a0", "accepted", 1740),
    ],
    reviews: [
      review("v1", "developer-1", 2, "started", 40),
      review("v2", "developer-1", 1, "findings", 360),
      review("v3", "designer-1", 1, "passed", 540),
      review("v4", "developer-2", 1, "passed", 1080),
      review("v5", "developer-0", 1, "passed", 1440),
    ],
    integrations: [
      integration("7c1e0f2a", "running", 5),
      integration("5d02b9c4", "merged", 240),
      integration("31aa8e07", "merged", 960),
    ],
    ...overrides,
  });
}

/** History for the graphs: 150 samples (five minutes at 2 s), shaped like a queue that built up while one agent stalled. */
export function showcaseRings() {
  const n = 150;
  const wiggle = [0, 0, 1, 0, -1, 0, 1, 1, 0, -1, 0, 1];
  const unresolved = Array.from({ length: n }, (_, i) =>
    Math.max(
      0,
      Math.min(
        6,
        Math.round(0.3 + 4.6 * Math.pow(i / n, 0.8) + wiggle[i % 12]! * 0.8),
      ),
    ),
  );
  unresolved.splice(-6, 6, 5, 5, 5, 5, 5, 5);
  const working = Array.from({ length: n }, (_, i) =>
    Math.max(
      0,
      Math.min(
        4,
        Math.round(2 + Math.sin(i / 7) * 1.3 + Math.cos(i / 3) * 0.4),
      ),
    ),
  );
  working.splice(-4, 4, 3, 3, 3, 3);
  const oldest = Array.from({ length: n }, (_, i) =>
    i < 50 ? (i % 25) * 4 : Math.min(221, (i - 30) * 2),
  );
  return { unresolved, working, oldest };
}

/** The run behind the real-terminal defect report: one active PM, nine ended agents, reviews by named reviewers, confirmed integrations and an escalated finding on an agent that has ended. */
export function crowded(overrides: Status = {}): Status {
  const ended = (id: string, kind: string, ago: number) =>
    agent(id, kind, 1, "ended", ago);
  return healthy({
    projectId: "capstan",
    supervision: {
      enabled: false,
      health: "degraded",
      targetEpoch: 0,
      checkpointEpoch: null,
      checkpointAssignmentId: null,
      replacementAttempts: 0,
    },
    agents: [
      ended("supervisor-1", "Supervisor", 4000),
      ended("designer-1", "Designer", 3500),
      ended("developer-1", "Developer", 3000),
      ended("reviewer-1", "Reviewer", 2500),
      ended("reviewer-2", "Reviewer", 2000),
      ended("reviewer-3", "Reviewer", 1500),
      ended("reviewer-4", "Reviewer", 900),
      ended("reviewer-5", "Reviewer", 600),
      ended("reviewer-6", "Reviewer", 300),
      agent("pm-1", "PM", 1, "active", 2),
    ],
    panes: [{ agentId: "pm-1", paneId: "w1:p1" }],
    reports: [
      {
        reportId: "r1",
        agentId: "designer-1",
        commitSha: "8bf60fa".padEnd(40, "0"),
        state: "accepted",
        createdAt: iso(3400),
      },
      {
        reportId: "r2",
        agentId: "developer-1",
        commitSha: "9abf487".padEnd(40, "0"),
        state: "accepted",
        createdAt: iso(2900),
      },
      {
        reportId: "r3",
        agentId: "developer-1",
        commitSha: "45fe0de".padEnd(40, "0"),
        state: "rejected",
        createdAt: iso(2800),
      },
    ],
    reviews: [1, 2, 3, 4, 5, 6].map((n) => ({
      reviewId: `v${n}`,
      reportId: n < 4 ? `r${n}` : null,
      integrationId: n >= 4 ? `i${n - 3}0123456789` : null,
      round: 1,
      state: n === 3 ? "findings" : "passed",
      authorAgentId:
        n < 4 ? ["designer-1", "developer-1", "developer-1"][n - 1]! : null,
      reviewerAgentId: `reviewer-${n}`,
      createdAt: iso(2600 - n * 300),
    })),
    integrations: [1, 2, 3].map((n) => ({
      integrationId: `i${n}0123456789`,
      state: "confirmed",
      createdAt: iso(1200 - n * 200),
    })),
    agentFindings: [
      {
        findingId: "0192a1-aaaa",
        targetAgentId: "designer-1",
        severity: "medium",
        state: "escalated",
        interventions: 2,
        stateReason: "no progress after 2 interventions",
      },
    ],
    ...overrides,
  });
}
