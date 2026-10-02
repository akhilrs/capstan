import type { PackageProgress } from "./controller/core.js";

/** The Nexora statuses the PM writes; the same list is the CHECK on `external_links.synced_state`. */
export const NEXORA_STATES = [
  "backlog",
  "todo",
  "in_progress",
  "in_review",
  "completed",
  "wont_do",
] as const;
export type NexoraState = (typeof NEXORA_STATES)[number];

export const EXTERNAL_REF_KINDS = ["requirement", "plan", "package"] as const;
export type ExternalRefKind = (typeof EXTERNAL_REF_KINDS)[number];

/** Nexora display ids look like PM-47; the prefix is Nexora's, not the project code. */
export const NEXORA_ID_PATTERN = /^[A-Za-z][A-Za-z0-9]{0,15}-[0-9]{1,9}$/;

export function isNexoraState(value: unknown): value is NexoraState {
  return (NEXORA_STATES as readonly unknown[]).includes(value);
}

/** What the ledger knows about one package: its derived progress plus whether it was cancelled or confirmed. */
export interface PackageFacts {
  readonly progress: PackageProgress;
  /** The package or its whole plan was cancelled. */
  readonly cancelled: boolean;
  /** The package's report is in a confirmed integration. */
  readonly confirmed: boolean;
}

type PackageStanding = PackageProgress | "confirmed" | "cancelled";

/** First match wins: a confirmed package is finished work that a later cancel does not unwind. */
function standing(facts: PackageFacts): PackageStanding {
  if (facts.confirmed) return "confirmed";
  if (facts.cancelled) return "cancelled";
  return facts.progress;
}

const PACKAGE_WANTED: Readonly<Record<PackageStanding, NexoraState>> = {
  unassigned: "todo",
  assigned: "in_progress",
  reported: "in_progress",
  findings: "in_progress",
  reviewed: "in_review",
  integrated: "in_review",
  confirmed: "completed",
  cancelled: "wont_do",
};

export function wantedPackageState(facts: PackageFacts): NexoraState {
  return PACKAGE_WANTED[standing(facts)];
}

export interface PlanFacts {
  readonly state: "draft" | "in_review" | "approved" | "superseded";
  readonly cancelled: boolean;
  readonly packages: readonly PackageFacts[];
}

/** The parent item of a plan; null for a superseded plan, which keeps what it had and is not drift-checked. Rows are evaluated in order. */
export function wantedPlanState(facts: PlanFacts): NexoraState | null {
  if (facts.state === "superseded") return null;
  const standings = facts.packages.map(standing);
  const live = standings.filter((s) => s !== "cancelled");
  const allConfirmed =
    standings.length > 0 && standings.every((s) => s === "confirmed");
  if (facts.cancelled && !allConfirmed) return "wont_do";
  if (facts.state === "approved" && live.length === 0) return "wont_do";
  if (live.length > 0 && live.every((s) => s === "confirmed"))
    return "completed";
  if (
    facts.state === "draft" ||
    facts.state === "in_review" ||
    live.every((s) => s === "unassigned")
  )
    return "todo";
  if (
    live.every(
      (s) => s === "reviewed" || s === "integrated" || s === "confirmed",
    )
  )
    return "in_review";
  return "in_progress";
}

export interface RequirementFacts {
  /** A developer is bound with `link bind`. */
  readonly bound: boolean;
  /** What the PM last wrote; a recorded wont_do is the only record of a cancelled requirement. */
  readonly syncedState: NexoraState;
  /** The bound agent's latest accepted report is in a confirmed integration. */
  readonly reportConfirmed: boolean;
  /** The latest finished review of that report passed. */
  readonly reviewPassed: boolean;
}

/** The item of a small-tier requirement; null while no developer is bound, so there is no wanted state and no drift. Rows are evaluated in order. */
export function wantedRequirementState(
  facts: RequirementFacts,
): NexoraState | null {
  if (!facts.bound) return null;
  if (facts.syncedState === "wont_do" && !facts.reportConfirmed)
    return "wont_do";
  if (facts.reportConfirmed) return "completed";
  if (facts.reviewPassed) return "in_review";
  return "in_progress";
}
