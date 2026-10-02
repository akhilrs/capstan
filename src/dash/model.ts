/** Pure view model: one operator `status` response in, display rows out. No I/O. */
import { clean, commitShort } from "./format.js";

export const WORKING_WINDOW_MS = 30_000;
/** The daemon returns at most this many reports, reviews, integrations and findings, newest first. */
export const PIPELINE_ITEMS = 20;
export const HIGHLIGHT_POLLS = 2;

export type Health = "healthy" | "evaluating" | "degraded" | "unknown";

export interface AgentRow {
  readonly id: string;
  readonly agentId: string;
  readonly roleName: string;
  readonly kind: string;
  readonly generation: number;
  readonly state: string;
  readonly lastActivityAt: string;
  /** Inferred: activity within 30 s, or a message in flight to the agent. */
  readonly working: boolean;
  readonly stalled: boolean;
  readonly lost: boolean;
  readonly blocked: boolean;
  readonly paneId: string | null;
  readonly queueDepth: number;
  readonly fingerprint: string;
}

export interface MessageRow {
  readonly id: string;
  readonly messageId: string;
  readonly recipientAgentId: string;
  readonly recipientActive: boolean;
  readonly state: string;
  readonly sequence: number;
  readonly queuedAt: string;
  readonly deferredReason: string | null;
  readonly stateReason: string | null;
  readonly notified: boolean;
  /** Why this is a delivery problem, or null. */
  readonly problem: string | null;
  readonly fingerprint: string;
}

export interface PipelineItem {
  readonly id: string;
  readonly stage: "report" | "review" | "integration";
  readonly state: string;
  readonly label: string;
  readonly createdAt: string;
  readonly fingerprint: string;
}

export interface PipelineStage {
  readonly counts: Readonly<Record<string, number>>;
  readonly total: number;
  /** The list reached the daemon's cap, so older rows may exist. */
  readonly capped: boolean;
  readonly items: readonly PipelineItem[];
}

export interface FindingRow {
  readonly id: string;
  readonly findingId: string;
  readonly targetAgentId: string;
  readonly severity: string;
  readonly state: string;
  readonly interventions: number;
  readonly stateReason: string | null;
  readonly needsOperator: boolean;
  readonly fingerprint: string;
}

export interface WorkRow {
  readonly id: string;
  readonly workItemId: string;
  readonly title: string;
  readonly state: string;
  readonly owner: string | null;
  readonly fingerprint: string;
}

export interface DashModel {
  readonly header: {
    readonly projectId: string;
    readonly runState: string;
    readonly supervisionEnabled: boolean;
    readonly health: Health;
    readonly healthReason: string | null;
    readonly targetEpoch: number | null;
    readonly checkpointEpoch: number | null;
    readonly replacementAttempts: number;
    readonly workers: number;
    readonly workerLimit: number | null;
  };
  readonly agents: readonly AgentRow[];
  readonly endedAgentsHidden: number;
  readonly pipeline: {
    readonly reports: PipelineStage;
    readonly reviews: PipelineStage;
    readonly integrations: PipelineStage;
  };
  readonly queue: {
    readonly messages: readonly MessageRow[];
    readonly truncated: boolean;
    readonly inputClears: readonly string[];
    readonly inputClearCount: number;
    readonly cleanupFailed: readonly string[];
    readonly orphanPanes: readonly string[];
  };
  readonly findings: readonly FindingRow[];
  readonly work: readonly WorkRow[];
  /** Samples for the sparklines. */
  readonly counts: { readonly unresolved: number; readonly working: number };
}

type Rec = Record<string, unknown>;

function list(value: unknown): Rec[] {
  return Array.isArray(value)
    ? (value.filter((v) => typeof v === "object" && v !== null) as Rec[])
    : [];
}
function text(value: unknown): string {
  return value === undefined || value === null ? "" : clean(value);
}
function textOrNull(value: unknown): string | null {
  return value === undefined || value === null ? null : clean(value);
}
function num(value: unknown): number {
  return typeof value === "number" && Number.isFinite(value) ? value : 0;
}
function numOrNull(value: unknown): number | null {
  return typeof value === "number" && Number.isFinite(value) ? value : null;
}

/** Workers are every active agent except the PM and the Supervisor, the same rule the launcher applies to `limits.max_workers`. */
export function isWorkerKind(kind: string): boolean {
  return kind !== "PM" && kind !== "Supervisor";
}

const PROBLEM_STATES: ReadonlySet<string> = new Set([
  "failed",
  "expired",
  "unacked",
]);

function stageOf(
  rows: readonly Rec[],
  stage: PipelineItem["stage"],
  idKey: string,
  label: (row: Rec) => string,
): PipelineStage {
  const counts: Record<string, number> = {};
  for (const row of rows) {
    const state = text(row.state);
    counts[state] = (counts[state] ?? 0) + 1;
  }
  const items = rows
    .map((row): PipelineItem => {
      const state = text(row.state);
      return {
        id: `${stage}:${text(row[idKey])}`,
        stage,
        state,
        label: label(row),
        createdAt: text(row.createdAt),
        fingerprint: `${state}|${label(row)}`,
      };
    })
    .sort((a, b) => b.createdAt.localeCompare(a.createdAt))
    .slice(0, PIPELINE_ITEMS);
  return {
    counts,
    total: rows.length,
    capped: rows.length >= PIPELINE_ITEMS,
    items,
  };
}

export function buildDashModel(
  status: Rec,
  nowMs: number,
  workerLimit: number | null,
): DashModel {
  const run = (status.run ?? {}) as Rec;
  const supervision = (status.supervision ?? {}) as Rec;
  const health = ((): Health => {
    const value = supervision.health;
    return value === "healthy" || value === "evaluating" || value === "degraded"
      ? value
      : "unknown";
  })();

  const rawMessages = list(status.messages);
  const stuck = new Map<string, string>();
  for (const entry of list(status.stuck))
    stuck.set(text(entry.messageId), text(entry.reason));
  const stalled = new Set(
    (Array.isArray(status.stalledAgentIds) ? status.stalledAgentIds : []).map(
      text,
    ),
  );
  const lost = new Set(
    (Array.isArray(status.lostAgentIds) ? status.lostAgentIds : []).map(text),
  );
  const paneOf = new Map<string, string>();
  for (const pane of list(status.panes))
    if (pane.paneId) paneOf.set(text(pane.agentId), text(pane.paneId));
  const agentRecords = list(status.agents);
  const activeAgents = new Set(
    agentRecords
      .filter((a) => a.state === "active")
      .map((a) => text(a.agentId)),
  );

  const depth = new Map<string, number>();
  const inFlight = new Set<string>();
  const stuckRecipients = new Set<string>();
  for (const m of rawMessages) {
    const to = text(m.recipientAgentId);
    depth.set(to, (depth.get(to) ?? 0) + 1);
    if (m.state === "sent" || m.state === "unacked") inFlight.add(to);
    if (stuck.has(text(m.messageId))) stuckRecipients.add(to);
  }
  const findings = list(status.agentFindings);
  const escalatedTargets = new Set(
    findings
      .filter((f) => f.state === "escalated")
      .map((f) => text(f.targetAgentId)),
  );

  const allAgents = agentRecords.map((a): AgentRow => {
    const agentId = text(a.agentId);
    const state = text(a.state);
    const lastActivityAt = text(a.lastActivityAt);
    const last = Date.parse(lastActivityAt);
    const working =
      state === "active" &&
      (inFlight.has(agentId) ||
        (!Number.isNaN(last) && nowMs - last < WORKING_WINDOW_MS));
    const row = {
      id: agentId,
      agentId,
      roleName: text(a.roleName),
      kind: text(a.kind),
      generation: num(a.generation),
      state,
      lastActivityAt,
      working,
      stalled: stalled.has(agentId),
      lost: lost.has(agentId),
      blocked: stuckRecipients.has(agentId) || escalatedTargets.has(agentId),
      paneId: paneOf.get(agentId) ?? null,
      queueDepth: depth.get(agentId) ?? 0,
    };
    return {
      ...row,
      fingerprint: [
        row.state,
        row.generation,
        row.working,
        row.stalled,
        row.lost,
        row.blocked,
        row.queueDepth,
      ].join("|"),
    };
  });
  const attention = (a: AgentRow) => (a.lost || a.stalled || a.blocked ? 0 : 1);
  const active = allAgents
    .filter((a) => a.state === "active")
    .map((a, index) => ({ a, index }))
    .sort((x, y) => attention(x.a) - attention(y.a) || x.index - y.index)
    .map(({ a }) => a);
  const ended = allAgents.filter((a) => a.state !== "active");

  const unsortedMessages = rawMessages.map((m): MessageRow => {
    const messageId = text(m.messageId);
    const state = text(m.state);
    const reason = stuck.get(messageId);
    const problem =
      reason !== undefined
        ? reason
        : PROBLEM_STATES.has(state)
          ? (textOrNull(m.stateReason) ?? state)
          : null;
    const row = {
      id: messageId,
      messageId,
      recipientAgentId: text(m.recipientAgentId),
      recipientActive: activeAgents.has(text(m.recipientAgentId)),
      state,
      sequence: num(m.sequence),
      queuedAt: text(m.queuedAt),
      deferredReason: textOrNull(m.deferredReason),
      stateReason: textOrNull(m.stateReason),
      notified: m.lastNotifiedAt != null,
      problem,
    };
    return {
      ...row,
      fingerprint: [
        row.state,
        row.deferredReason,
        row.stateReason,
        row.notified,
        row.problem,
      ].join("|"),
    };
  });

  const messages = unsortedMessages
    .map((m, index) => ({ m, index }))
    .sort(
      (x, y) =>
        Number(y.m.problem !== null) - Number(x.m.problem !== null) ||
        x.index - y.index,
    )
    .map(({ m }) => m);

  const findingRows = findings
    .filter((f) => f.state === "open" || f.state === "escalated")
    .sort(
      (x, y) =>
        Number(y.state === "escalated") - Number(x.state === "escalated"),
    )
    .map((f): FindingRow => {
      const row = {
        id: text(f.findingId),
        findingId: text(f.findingId),
        targetAgentId: text(f.targetAgentId),
        severity: text(f.severity),
        state: text(f.state),
        interventions: num(f.interventions),
        stateReason: textOrNull(f.stateReason),
        needsOperator: f.state === "escalated",
      };
      return {
        ...row,
        fingerprint: [row.state, row.interventions, row.stateReason].join("|"),
      };
    });

  const clears = list(status.inputClears);
  const orphanPanes = list(status.orphanPanes).map((o) =>
    `${text(o.agentId)} ${text(o.paneId)}`.trim(),
  );
  const cleanupFailed = list(status.cleanupFailed).map(
    (c) => `${text(c.agentId)}: ${text(c.reason)}`,
  );

  const work = list(status.work).map((w): WorkRow => {
    const state = text(w.state);
    return {
      id: text(w.workItemId),
      workItemId: text(w.workItemId),
      title: text(w.title),
      state,
      owner: textOrNull(w.owner),
      fingerprint: state,
    };
  });

  return {
    header: {
      projectId: text(status.projectId),
      runState: text(run.state),
      supervisionEnabled: supervision.enabled === true,
      health,
      healthReason: textOrNull(status.supervisionReason),
      targetEpoch: numOrNull(supervision.targetEpoch),
      checkpointEpoch: numOrNull(supervision.checkpointEpoch),
      replacementAttempts: num(supervision.replacementAttempts),
      workers: active.filter((a) => isWorkerKind(a.kind)).length,
      workerLimit,
    },
    agents: [...active, ...ended.slice(0, 5)],
    endedAgentsHidden: Math.max(0, ended.length - 5),
    pipeline: {
      reports: stageOf(
        list(status.reports),
        "report",
        "reportId",
        (r) => `${text(r.agentId)} ${commitShort(textOrNull(r.commitSha))}`,
      ),
      reviews: stageOf(
        list(status.reviews),
        "review",
        "reviewId",
        (r) => `${text(r.authorAgentId)} r${num(r.round)}`,
      ),
      integrations: stageOf(
        list(status.integrations),
        "integration",
        "integrationId",
        (i) => `${text(i.integrationId).slice(0, 8)}`,
      ),
    },
    queue: {
      messages,
      truncated: status.messagesTruncated === true,
      inputClears: clears
        .slice(0, 3)
        .map(
          (c) =>
            `message ${text(c.messageId).slice(0, 8)} ${text(c.recordedAt)}`,
        ),
      inputClearCount: clears.length,
      cleanupFailed,
      orphanPanes,
    },
    findings: findingRows,
    work,
    counts: {
      unresolved: messages.length,
      working: active.filter((a) => a.working).length,
    },
  };
}

/** Rows that changed since the previous poll keep their highlight for `HIGHLIGHT_POLLS` polls. */
export type ChangeState = {
  readonly fingerprints: ReadonlyMap<string, string>;
  readonly highlight: ReadonlyMap<string, number>;
};

export const EMPTY_CHANGES: ChangeState = {
  fingerprints: new Map(),
  highlight: new Map(),
};

export function trackChanges(
  previous: ChangeState,
  model: DashModel,
): ChangeState {
  const rows: { id: string; fingerprint: string }[] = [
    ...model.agents.map((r) => ({
      id: `a:${r.id}`,
      fingerprint: r.fingerprint,
    })),
    ...model.queue.messages.map((r) => ({
      id: `m:${r.id}`,
      fingerprint: r.fingerprint,
    })),
    ...model.findings.map((r) => ({
      id: `f:${r.id}`,
      fingerprint: r.fingerprint,
    })),
    ...model.work.map((r) => ({ id: `w:${r.id}`, fingerprint: r.fingerprint })),
    ...[
      ...model.pipeline.reports.items,
      ...model.pipeline.reviews.items,
      ...model.pipeline.integrations.items,
    ].map((r) => ({ id: `p:${r.id}`, fingerprint: r.fingerprint })),
  ];
  const fingerprints = new Map<string, string>();
  const highlight = new Map<string, number>();
  const first = previous.fingerprints.size === 0;
  for (const row of rows) {
    fingerprints.set(row.id, row.fingerprint);
    const before = previous.fingerprints.get(row.id);
    if (!first && before !== row.fingerprint)
      highlight.set(row.id, HIGHLIGHT_POLLS);
    else {
      const left = (previous.highlight.get(row.id) ?? 0) - 1;
      if (left > 0) highlight.set(row.id, left);
    }
  }
  return { fingerprints, highlight };
}

/** Messages shown in the queue panel: all of them, or only delivery problems. */
export function queueRows(
  model: DashModel,
  problemsOnly: boolean,
): readonly MessageRow[] {
  return problemsOnly
    ? model.queue.messages.filter((m) => m.problem !== null)
    : model.queue.messages;
}

/** Pipeline items of all three stages, newest first. */
export function pipelineItems(model: DashModel): readonly PipelineItem[] {
  return [
    ...model.pipeline.reports.items,
    ...model.pipeline.reviews.items,
    ...model.pipeline.integrations.items,
  ].sort((a, b) => b.createdAt.localeCompare(a.createdAt));
}

export interface HistorySample {
  readonly unresolved: number;
  readonly working: number;
  /** Seconds since the oldest unresolved message was queued; 0 when there is none. */
  readonly oldestSeconds: number;
}

export function historySample(model: DashModel, nowMs: number): HistorySample {
  let oldest = 0;
  for (const m of model.queue.messages) {
    const queued = Date.parse(m.queuedAt);
    if (!Number.isNaN(queued))
      oldest = Math.max(oldest, Math.floor((nowMs - queued) / 1000));
  }
  return {
    unresolved: model.counts.unresolved,
    working: model.counts.working,
    oldestSeconds: oldest,
  };
}
