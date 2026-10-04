/** Pure view model: one operator `status` response in, display rows out. No I/O. */
import { clean, commitShort } from "./format.js";

export const WORKING_WINDOW_MS = 30_000;
/** The daemon returns at most this many reports, reviews, integrations and findings, newest first. */
export const PIPELINE_ITEMS = 20;
export const HIGHLIGHT_POLLS = 2;

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
  /** When delivery to the agent was paused (its own pause, or the run's for a non-PM agent), or null. */
  readonly pausedAt: string | null;
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
  /** `ended` and `unknown` (not in the agent list) targets cannot act on the finding any more. */
  readonly targetState: "active" | "ended" | "unknown";
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

export interface SupervisionHeader {
  /** `[supervision] enabled` in the config. */
  readonly enabled: boolean;
  readonly supervisor: { readonly id: string; readonly state: string } | null;
  /** The newest routine check queued; ackedAt is null until it is acknowledged. */
  readonly lastCheck: {
    readonly state: string;
    readonly queuedAt: string;
    readonly ackedAt: string | null;
  } | null;
  readonly openFindings: number;
}

export interface DashModel {
  readonly header: {
    readonly projectId: string;
    readonly runState: string;
    /** The open run pause, or null. */
    readonly runPause: {
      readonly pausedAt: string;
      readonly reason: string;
      readonly actorId: string;
    } | null;
    /** The live supervision loop; null when the status carries no `supervisionState`. */
    readonly supervision: SupervisionHeader | null;
    readonly workers: number;
    readonly workerLimit: number | null;
    /** Whole minutes of full auto left; present only while it is on. */
    readonly fullAutoMinutes?: number;
    /** Session grants in force; present only when there are any. */
    readonly grants?: number;
  };
  /** Active agents first (attention-first), then every ended agent, most recent activity first. */
  readonly agents: readonly AgentRow[];
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

/** Workers are every active agent except the PM and the Supervisor, the same rule the launcher applies to `limits.max_workers`. */
export function isWorkerKind(kind: string): boolean {
  return kind !== "PM" && kind !== "Supervisor";
}

const PROBLEM_STATES: ReadonlySet<string> = new Set([
  "failed",
  "expired",
  "unacked",
]);

/** `reviewer-4 -> developer-1 r1`: who reviews, whose work and the round; an integration review names the integration. */
function reviewLabel(r: Rec): string {
  const reviewer = text(r.reviewerAgentId) || "no reviewer";
  const subject =
    text(r.authorAgentId) ||
    (text(r.integrationId) ? text(r.integrationId).slice(0, 8) : "");
  return `${reviewer}${subject === "" ? "" : ` -> ${subject}`} r${num(r.round)}`;
}

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

/** Full auto and grants from the Operator part of the status; nothing when there is none. */
function operatorHeader(operator: unknown): {
  fullAutoMinutes?: number;
  grants?: number;
} {
  const { fullAuto, grants } = (operator ?? {}) as Rec;
  const full = (fullAuto ?? {}) as Rec;
  return {
    ...(full.on === true
      ? { fullAutoMinutes: Math.ceil(num(full.remainingSeconds) / 60) }
      : {}),
    ...(list(grants).length > 0 ? { grants: list(grants).length } : {}),
  };
}

export function buildDashModel(
  status: Rec,
  nowMs: number,
  workerLimit: number | null,
): DashModel {
  const run = (status.run ?? {}) as Rec;
  const supervision = ((): SupervisionHeader | null => {
    const state = status.supervisionState;
    if (typeof state !== "object" || state === null) return null;
    const rec = state as Rec;
    const supervisor = rec.supervisor as Rec | null | undefined;
    const check = rec.lastCheck as Rec | null | undefined;
    return {
      enabled: rec.enabled === true,
      supervisor: supervisor
        ? { id: text(supervisor.agentId), state: text(supervisor.state) }
        : null,
      lastCheck: check
        ? {
            state: text(check.state),
            queuedAt: text(check.queuedAt),
            ackedAt: textOrNull(check.ackedAt),
          }
        : null,
      openFindings: num(rec.openFindings),
    };
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
  const pause = (status.pause ?? {}) as Rec;
  const runPauseRecord = (pause.run ?? null) as Rec | null;
  const pausedAtOf = new Map<string, string>();
  for (const entry of list(pause.agents))
    pausedAtOf.set(text(entry.agentId), text(entry.pausedAt));
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
      pausedAt:
        pausedAtOf.get(agentId) ??
        (runPauseRecord !== null && text(a.kind) !== "PM"
          ? text(runPauseRecord.pausedAt)
          : null),
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
        row.pausedAt !== null,
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
  const ended = allAgents
    .filter((a) => a.state !== "active")
    .map((a, index) => ({ a, index, at: Date.parse(a.lastActivityAt) }))
    .sort(
      (x, y) =>
        (Number.isNaN(y.at) ? -Infinity : y.at) -
          (Number.isNaN(x.at) ? -Infinity : x.at) || y.index - x.index,
    )
    .map(({ a }) => a);

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
        targetState: ((): FindingRow["targetState"] => {
          const target = text(f.targetAgentId);
          if (activeAgents.has(target)) return "active";
          return agentRecords.some((a) => text(a.agentId) === target)
            ? "ended"
            : "unknown";
        })(),
      };
      return {
        ...row,
        fingerprint: [
          row.state,
          row.interventions,
          row.stateReason,
          row.targetState,
        ].join("|"),
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
      runPause:
        runPauseRecord === null
          ? null
          : {
              pausedAt: text(runPauseRecord.pausedAt),
              reason: text(runPauseRecord.reason),
              actorId: text(runPauseRecord.actorId),
            },
      supervision,
      workers: active.filter((a) => isWorkerKind(a.kind)).length,
      workerLimit,
      ...operatorHeader(status.operator),
    },
    agents: [...active, ...ended],
    pipeline: {
      reports: stageOf(
        list(status.reports),
        "report",
        "reportId",
        (r) => `${text(r.agentId)} ${commitShort(textOrNull(r.commitSha))}`,
      ),
      reviews: stageOf(list(status.reviews), "review", "reviewId", reviewLabel),
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
/** How many of the most recently ended agents the panel shows unless "show all ended" is on. */
export const RECENT_ENDED_SHOWN = 5;

/**
 * The agents the panel lists: every active agent, any ended agent that is still flagged lost,
 * stalled or blocked, and the most recently ended few (the model keeps ended agents newest first).
 */
export function visibleAgents(
  agents: readonly AgentRow[],
  showAllEnded: boolean,
): readonly AgentRow[] {
  if (showAllEnded) return agents;
  let recent = 0;
  return agents.filter((a) => {
    if (a.state === "active" || a.lost || a.stalled || a.blocked) return true;
    return recent++ < RECENT_ENDED_SHOWN;
  });
}

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
