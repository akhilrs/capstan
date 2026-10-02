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
