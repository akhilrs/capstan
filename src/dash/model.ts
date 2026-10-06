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
  /** What the agent works on, one line; `-` when nothing is known. */
  readonly task: string;
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
  /** The target is not active, so nobody can act on the finding any more (`targetState !== "active"`). */
  readonly stale: boolean;
  readonly fingerprint: string;
}

export type WaitingKind =
  "integration" | "proposal" | "agent-paused" | "run-paused";

/** Something that waits for the operator: a confirm, a proposal to decide, or a pause to lift. */
export interface WaitingItem {
  readonly id: string;
  readonly kind: WaitingKind;
  /** One line, without the age. */
  readonly label: string;
  /** ISO time the wait began, the source of the age shown; null when unknown. */
  readonly since: string | null;
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

export interface PmMailHeader {
  readonly pending: number;
  readonly oldestAgeSeconds: number;
  readonly stale: boolean;
}

/** One thing being worked on: `id` is the short name, `label` the full line (id, title, progress). */
export interface TaskEntry {
  readonly id: string;
  readonly label: string;
}

export interface DashModel {
  readonly header: {
    /** Active tasks; null when the status carries no task data (no header line), an empty list means idle. */
    readonly tasks: readonly TaskEntry[] | null;
    /** The PM's pending mail; null when the status carries none (no live PM, or not the operator). */
    readonly pmMail: PmMailHeader | null;
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
  /** What waits for the operator, in the order shown; empty when nothing does. */
  readonly waiting: readonly WaitingItem[];
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

export const TASK_NONE = "-";

/** `a b`, or whichever part is present; empty when neither is. */
function joinParts(...parts: readonly (string | null | undefined)[]): string {
  return parts
    .filter((p) => p !== null && p !== undefined && p !== "")
    .join(" ");
}

/** `(done/total)` from two counters, or "" when either is not a count. */
function progressOf(done: unknown, total: unknown): string {
  return typeof done === "number" &&
    typeof total === "number" &&
    Number.isFinite(done) &&
    Number.isFinite(total)
    ? `(${Math.max(0, Math.floor(done))}/${Math.max(0, Math.floor(total))})`
    : "";
}

const ACTIVE_PLAN_STATES: ReadonlySet<string> = new Set([
  "draft",
  "in_review",
  "approved",
]);

/** What the status says about who works on what, as lookups by agent id. */
interface TaskIndex {
  /** Header entries; null when the status has neither `activeTasks` nor `plans`. */
  readonly entries: readonly TaskEntry[] | null;
  /** Agent id -> `plan-19 · plan`, with ` +k` when it is the Architect of k more plans. */
  readonly architect: ReadonlyMap<string, string>;
  /** Agent id -> `plan-19/dash-ui PM-114 <title>`. */
  readonly assignee: ReadonlyMap<string, string>;
  /** Agent id -> `<nexora id or ref id> <title>`. */
  readonly requirement: ReadonlyMap<string, string>;
  /** `plan/package` -> the package label, for a pane whose task ref names a package. */
  readonly packageByRef: ReadonlyMap<string, string>;
  /** Requirement ref id -> label, for a pane whose task ref names a requirement but has no title. */
  readonly requirementByRef: ReadonlyMap<string, string>;
  /** Built from `status.plans` alone (an older daemon): a pane's own task fields come before the Architect. */
  readonly fromPlans: boolean;
}

function setFirst(map: Map<string, string>, key: string, value: string): void {
  if (key !== "" && !map.has(key)) map.set(key, value);
}

function architectLabels(
  plansOf: ReadonlyMap<string, readonly string[]>,
): Map<string, string> {
  const labels = new Map<string, string>();
  for (const [agentId, ids] of plansOf)
    labels.set(
      agentId,
      `${ids[0]} · plan${ids.length > 1 ? ` +${ids.length - 1}` : ""}`,
    );
  return labels;
}

function addArchitect(
  plansOf: Map<string, string[]>,
  agentId: string,
  planId: string,
): void {
  if (agentId !== "")
    plansOf.set(agentId, [...(plansOf.get(agentId) ?? []), planId]);
}

/**
 * The header entries and the per-agent task lookups from `status.activeTasks`. A status without it (an older
 * daemon) falls back to `status.plans`, which gives open plans and Architects but no packages or requirements;
 * with neither, or no open plan, `entries` is null. Every field is optional: a malformed one is skipped.
 */
export function taskIndexOf(status: Rec): TaskIndex {
  const plansOf = new Map<string, string[]>();
  const assignee = new Map<string, string>();
  const requirement = new Map<string, string>();
  const packageByRef = new Map<string, string>();
  const requirementByRef = new Map<string, string>();
  const entries: TaskEntry[] = [];
  const index = (
    found: readonly TaskEntry[] | null,
    fromPlans: boolean,
  ): TaskIndex => ({
    entries: found,
    architect: architectLabels(plansOf),
    assignee,
    requirement,
    packageByRef,
    requirementByRef,
    fromPlans,
  });
  const raw = status.activeTasks;
  if (typeof raw === "object" && raw !== null && !Array.isArray(raw)) {
    const active = raw as Rec;
    for (const plan of list(active.plans)) {
      const planId = text(plan.planId);
      if (planId === "") continue;
      entries.push({
        id: planId,
        label: joinParts(
          planId,
          text(plan.title),
          progressOf(plan.done, plan.total),
        ),
      });
      addArchitect(plansOf, text(plan.architectAgentId), planId);
      for (const pkg of list(plan.packages)) {
        const packageId = text(pkg.packageId);
        if (packageId === "") continue;
        const ref = `${planId}/${packageId}`;
        const label = joinParts(ref, text(pkg.nexoraId), text(pkg.title));
        if (!packageByRef.has(ref)) packageByRef.set(ref, label);
        setFirst(assignee, text(pkg.assigneeAgentId), label);
      }
    }
    for (const req of list(active.requirements)) {
      const refId = text(req.refId);
      const name = text(req.nexoraId) || refId;
      if (name === "") continue;
      const label = joinParts(name, text(req.title));
      entries.push({ id: name, label });
      if (refId !== "") requirementByRef.set(refId, label);
      for (const agentId of Array.isArray(req.agentIds) ? req.agentIds : [])
        setFirst(requirement, text(agentId), label);
    }
    return index(entries, false);
  }
  if (!Array.isArray(status.plans)) return index(null, false);
  for (const plan of list(status.plans)) {
    const planId = text(plan.planId);
    if (
      planId === "" ||
      plan.cancelled === true ||
      !ACTIVE_PLAN_STATES.has(text(plan.state)) ||
      list(plan.signoffs).length > 0
    )
      continue;
    addArchitect(plansOf, text(plan.architectAgentId), planId);
    const counts =
      typeof plan.packages === "object" &&
      plan.packages !== null &&
      !Array.isArray(plan.packages)
        ? (plan.packages as Rec)
        : {};
    let total = 0;
    for (const [state, n] of Object.entries(counts))
      if (state !== "cancelled") total += num(n);
    const done = num(counts.reviewed) + num(counts.integrated);
    entries.push({
      id: planId,
      label: joinParts(planId, text(plan.title), progressOf(done, total)),
    });
  }
  return index(entries.length === 0 ? null : entries, true);
}

/** The task a pane was spawned with: the package or requirement it names, else its ref and spawn title. */
function paneTask(pane: Rec | undefined, index: TaskIndex): string | null {
  if (pane === undefined) return null;
  const ref = text(pane.taskRef);
  if (ref === "") return null;
  const known = ref.includes("/")
    ? index.packageByRef.get(ref)
    : index.requirementByRef.get(ref);
  return known ?? (joinParts(ref, text(pane.taskTitle)) || null);
}

/** `review <author or integration id> r<round>` by reviewer, for each started review. */
function reviewSubjects(reviews: readonly Rec[]): Map<string, string> {
  const subjects = new Map<string, string>();
  for (const r of reviews) {
    if (r.state !== "started") continue;
    const subject = text(r.authorAgentId) || text(r.integrationId).slice(0, 8);
    if (subject !== "")
      setFirst(
        subjects,
        text(r.reviewerAgentId),
        `review ${subject} r${num(r.round)}`,
      );
  }
  return subjects;
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

/** The all-time per-state counts of one stage from `status.pipelineCounts`, or null when absent or malformed. */
function trueCounts(
  pipelineCounts: unknown,
  key: string,
): Record<string, number> | null {
  if (typeof pipelineCounts !== "object" || pipelineCounts === null)
    return null;
  const raw = (pipelineCounts as Rec)[key];
  if (typeof raw !== "object" || raw === null || Array.isArray(raw))
    return null;
  const counts: Record<string, number> = {};
  for (const [state, count] of Object.entries(raw as Rec)) {
    if (typeof count !== "number" || !Number.isFinite(count) || count < 0)
      return null;
    if (count > 0) counts[clean(state)] = Math.floor(count);
  }
  return counts;
}

function stageOf(
  rows: readonly Rec[],
  stage: PipelineItem["stage"],
  idKey: string,
  label: (row: Rec) => string,
  truth: Record<string, number> | null,
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
  if (truth !== null)
    return {
      counts: truth,
      total: Object.values(truth).reduce((sum, n) => sum + n, 0),
      capped: false,
      items,
    };
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

/** Confirms, proposals and pauses that wait for the operator. Each source is optional; a missing one adds nothing. */
function waitingItems(status: Rec, pause: Rec): WaitingItem[] {
  const items: WaitingItem[] = [];
  for (const i of list(status.awaitingConfirm)) {
    const id = text(i.integrationId);
    items.push({
      id: `integration:${id}`,
      kind: "integration",
      label: `integration ${id.slice(0, 8)} merged, review ${text(i.reviewState) || "none"}: cstan integrate confirm pending`,
      since: textOrNull(i.createdAt),
    });
  }
  for (const p of list(status.pendingProposals)) {
    const id = text(p.proposalId);
    items.push({
      id: `proposal:${id}`,
      kind: "proposal",
      label: `proposal ${id} ${text(p.kind)} by ${text(p.proposer)}: ${text(p.reason)}`,
      since: textOrNull(p.createdAt),
    });
  }
  const run = pause.run as Rec | null | undefined;
  if (typeof run === "object" && run !== null)
    items.push({
      id: "run-paused",
      kind: "run-paused",
      label: `run paused: ${text(run.reason)}`,
      since: textOrNull(run.pausedAt),
    });
  for (const a of list(pause.agents)) {
    const id = text(a.agentId);
    items.push({
      id: `agent-paused:${id}`,
      kind: "agent-paused",
      label: `${id} paused: ${text(a.reason)}`,
      since: textOrNull(a.pausedAt),
    });
  }
  return items;
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
  const paneRecords = new Map<string, Rec>();
  for (const pane of list(status.panes))
    paneRecords.set(text(pane.agentId), pane);
  const taskIndex = taskIndexOf(status);
  const subjects = reviewSubjects(list(status.reviews));
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
      task:
        state === "active"
          ? (taskIndex.assignee.get(agentId) ??
            (taskIndex.fromPlans
              ? (paneTask(paneRecords.get(agentId), taskIndex) ??
                taskIndex.architect.get(agentId))
              : (taskIndex.architect.get(agentId) ??
                taskIndex.requirement.get(agentId) ??
                paneTask(paneRecords.get(agentId), taskIndex))) ??
            subjects.get(agentId) ??
            TASK_NONE)
          : TASK_NONE,
    };
    return {
      ...row,
      fingerprint: [
        row.state,
        row.task,
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
    .map((f): FindingRow => {
      const targetState = ((): FindingRow["targetState"] => {
        const target = text(f.targetAgentId);
        if (activeAgents.has(target)) return "active";
        return agentRecords.some((a) => text(a.agentId) === target)
          ? "ended"
          : "unknown";
      })();
      const row = {
        id: text(f.findingId),
        findingId: text(f.findingId),
        targetAgentId: text(f.targetAgentId),
        severity: text(f.severity),
        state: text(f.state),
        interventions: num(f.interventions),
        stateReason: textOrNull(f.stateReason),
        needsOperator: f.state === "escalated",
        targetState,
        stale: targetState !== "active",
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
    })
    // Live findings first (escalated before open), then stale ones in the same order.
    .sort(
      (x, y) =>
        Number(x.stale) - Number(y.stale) ||
        Number(y.needsOperator) - Number(x.needsOperator),
    );

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

  const pmMailRec = status.pmMail as Rec | null | undefined;
  return {
    header: {
      tasks: taskIndex.entries,
      pmMail:
        typeof pmMailRec === "object" && pmMailRec !== null
          ? {
              pending: num(pmMailRec.pending),
              oldestAgeSeconds: num(pmMailRec.oldestAgeSeconds),
              stale: pmMailRec.stale === true,
            }
          : null,
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
        trueCounts(status.pipelineCounts, "reports"),
      ),
      reviews: stageOf(
        list(status.reviews),
        "review",
        "reviewId",
        reviewLabel,
        trueCounts(status.pipelineCounts, "reviews"),
      ),
      integrations: stageOf(
        list(status.integrations),
        "integration",
        "integrationId",
        (i) => `${text(i.integrationId).slice(0, 8)}`,
        trueCounts(status.pipelineCounts, "integrations"),
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
    waiting: waitingItems(status, pause),
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

/** The queue panel shrinks to a stub while no message is unresolved. */
export function queueCollapsed(model: DashModel): boolean {
  return model.queue.messages.length === 0;
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
