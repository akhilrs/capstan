import { createHash } from "node:crypto";
import type { ControllerCore } from "./core.js";
import {
  validateWorkflowPlan,
  type ValidatedWorkflowPlan,
  type WorkflowSlice,
} from "./workflow.js";
import type { AssignmentResult, MutationContext, Role } from "./types.js";

export interface SchedulerSeatSpec {
  readonly seatId: string;
  readonly name: string;
  readonly displayName: string;
}

export interface SchedulerSeats {
  readonly PM: SchedulerSeatSpec;
  readonly Developer: SchedulerSeatSpec;
  readonly Verifier: SchedulerSeatSpec;
  readonly Supervisor: SchedulerSeatSpec;
}

export interface SchedulerDispatch {
  readonly slice: WorkflowSlice;
  readonly assignment: AssignmentResult;
  readonly developerCredential: string;
  /** Create the durable delivery context after runtime provisioning has finished. */
  readonly getMutationContext: () => MutationContext;
}

export interface SchedulerMutationContextStore {
  /** Return the original context on retries; persist new contexts before returning. */
  getOrCreate(key: string, create: () => MutationContext): MutationContext;
}

export interface SchedulerOptions {
  readonly plan: ValidatedWorkflowPlan;
  readonly core: ControllerCore;
  readonly operatorCredential: string;
  readonly seats: SchedulerSeats;
  readonly mutationContexts: SchedulerMutationContextStore;
  /** Durable run start timestamp supplied by the caller on every process start. */
  readonly startedAtMs: number;
  /** Must confirm the validated plan was accepted by the PM, not merely schema-valid. */
  readonly isPlanAccepted: (input: {
    readonly planHash: string;
    readonly pmCredential: string;
  }) => boolean | Promise<boolean>;
  /** Dispatch an already-created core assignment. */
  readonly dispatch: (input: SchedulerDispatch) => Promise<unknown>;
}

export interface SchedulerIdentity {
  readonly seatId: string;
  readonly actorId: string;
  readonly credential: string;
}

export interface SchedulerInitialization {
  readonly PM: SchedulerIdentity;
  readonly Developer: SchedulerIdentity;
  readonly Verifier: SchedulerIdentity;
  readonly Supervisor: SchedulerIdentity;
}

export type SchedulerStep =
  | { readonly state: "waiting_for_plan_acceptance" }
  | {
      readonly state: "waiting";
      readonly workItemId: string;
      readonly reasons: readonly string[];
    }
  | {
      readonly state: "dispatched";
      readonly sliceId: string;
      readonly assignment: AssignmentResult;
    }
  | { readonly state: "complete" }
  | { readonly state: "stopped"; readonly reason: string };

/**
 * Deterministic serial plan runner. All authoritative state and mutation receipts remain in
 * ControllerCore; the class keeps no mutable progress state between calls.
 */
export class WorkflowScheduler {
  readonly #options: SchedulerOptions;
  #initialization: SchedulerInitialization | undefined;

  constructor(options: SchedulerOptions) {
    if (!Number.isSafeInteger(options.startedAtMs) || options.startedAtMs <= 0)
      throw new TypeError("startedAtMs must be a positive safe integer");
    const validated = validateWorkflowPlan(options.plan.plan);
    if (
      validated.hash !== options.plan.hash ||
      validated.order.length !== options.plan.order.length ||
      validated.order.some((id, index) => id !== options.plan.order[index])
    )
      throw new TypeError(
        "plan, hash, and topological order do not match validated workflow content",
      );
    this.#options = { ...options, plan: validated };
  }

  get status() {
    return this.#options.core.statusSnapshot();
  }

  /** Creates stable role seats and actors, then stable plan work items and dependencies. */
  initialize(): SchedulerInitialization {
    if (this.#initialization) return this.#initialization;
    const { core, plan, seats } = this.#options;
    const pm = this.#activate("PM", seats.PM);
    const supervisor = this.#activate("Supervisor", seats.Supervisor);
    const developer = this.#activate("Developer", seats.Developer);
    const verifier = this.#activate("Verifier", seats.Verifier);
    for (const slice of plan.plan.slices) {
      const id = this.#workItemId(slice.id);
      const candidateId = `candidate-${createHash("sha256")
        .update(`${plan.plan.taskId}:${slice.id}`)
        .digest("hex")
        .slice(0, 24)}`;
      this.#mutate(`work-${slice.id}`, (context) =>
        core.createWorkItem(context, {
          workItemId: id,
          title: slice.title,
          description: `${slice.description}\n\nObjective: ${plan.plan.objective}\nSlice acceptance criteria: ${slice.acceptanceCriteria.join("; ")}\nParent acceptance criteria: ${plan.plan.acceptanceCriteria.join("; ")}\nWrite scope: ${slice.writeScope.join(", ")}\n\nDeveloper delivery contract: before editing, record \`git rev-parse HEAD\` as baseSha. Implement only the requested change in the write scope, then create a new Git commit in this isolated checkout without amending earlier commits. Return one JSON report with candidateId exactly "${candidateId}", commitSha as the full commit SHA, baseSha as the recorded starting SHA, changedScope as every changed repository-relative file and no others, and limitations as an array of strings. Do not report a candidate unless the commit is HEAD and \`git status --porcelain\` is empty.`,
          requiredRole: "Developer",
          acceptanceCriteria: slice.acceptanceCriteria,
        }),
      );
    }
    for (const slice of plan.plan.slices) {
      for (const dependency of slice.dependsOn) {
        this.#mutate(`dependency-${slice.id}-${dependency}`, (context) =>
          core.addDependency(
            context,
            this.#workItemId(slice.id),
            this.#workItemId(dependency),
          ),
        );
      }
    }
    this.#initialization = Object.freeze({
      PM: pm,
      Supervisor: supervisor,
      Developer: developer,
      Verifier: verifier,
    });
    return this.#initialization;
  }

  /** Performs at most one dispatch. Calling again resumes from observable core state. */
  async step(): Promise<SchedulerStep> {
    const identities = this.initialize();
    const { plan, core } = this.#options;
    const state = core.statusSnapshot();
    if (state.run.state !== "active")
      return { state: "stopped", reason: `run is ${state.run.state}` };
    if (Date.now() - this.#options.startedAtMs >= plan.plan.limits.maxRunMs)
      return { state: "stopped", reason: "maxRunMs exceeded" };

    const slices = plan.plan.slices;
    const workById = new Map(state.work.map((work) => [work.workItemId, work]));
    const dispatchCount = slices.reduce((count, slice) => {
      const work = workById.get(this.#workItemId(slice.id));
      return (
        count +
        (work && !["pending", "ready", "blocked"].includes(work.state) ? 1 : 0)
      );
    }, 0);
    if (dispatchCount >= plan.plan.limits.maxDispatches)
      return { state: "stopped", reason: "maxDispatches exceeded" };

    const planAccepted = await this.#options.isPlanAccepted({
      planHash: plan.hash,
      pmCredential: identities.PM.credential,
    });
    if (!planAccepted) return { state: "waiting_for_plan_acceptance" };

    for (const sliceId of plan.order) {
      const slice = slices.find((entry) => entry.id === sliceId)!;
      const workItemId = this.#workItemId(sliceId);
      const work = workById.get(workItemId);
      if (!work)
        return {
          state: "stopped",
          reason: `plan work item ${workItemId} is absent from core status`,
        };
      if (work.state === "accepted") continue;
      if (
        work.state !== "pending" &&
        work.state !== "ready" &&
        work.state !== "blocked"
      )
        return {
          state: "waiting",
          workItemId,
          reasons: [
            `work is ${work.state}; scheduler will not retry an uncertain assignment`,
          ],
        };
      if (
        slice.dependsOn.some(
          (dependency) =>
            workById.get(this.#workItemId(dependency))?.state !== "accepted",
        )
      )
        continue;

      // Readiness is deliberately checked immediately before readiness mutation/assignment.
      const readiness = core.readiness(workItemId);
      if (!readiness.ready)
        return { state: "waiting", workItemId, reasons: readiness.reasons };
      if (work.state !== "ready") {
        this.#mutate(`ready-${sliceId}`, (context) =>
          core.markReady(context, workItemId),
        );
      }
      const finalReadiness = core.readiness(workItemId);
      if (!finalReadiness.ready)
        return {
          state: "waiting",
          workItemId,
          reasons: finalReadiness.reasons,
        };
      const assignment = this.#mutate(`assign-${sliceId}`, (context) =>
        core.assignWorkItem(context, workItemId, identities.Developer.seatId),
      );
      const getMutationContext = () => this.#context(`dispatch-${sliceId}`);
      // Once assignment mutation returns, any dispatch failure is uncertain and is never retried here.
      try {
        await this.#options.dispatch({
          slice,
          assignment,
          developerCredential: identities.Developer.credential,
          getMutationContext,
        });
      } catch (error) {
        return {
          state: "stopped",
          reason: `dispatch outcome is uncertain for ${sliceId}: ${error instanceof Error ? error.message : String(error)}`,
        };
      }
      return { state: "dispatched", sliceId, assignment };
    }
    return slices.every(
      (slice) => workById.get(this.#workItemId(slice.id))?.state === "accepted",
    )
      ? { state: "complete" }
      : {
          state: "waiting",
          workItemId: this.#workItemId(slices[0]!.id),
          reasons: ["waiting for core acceptance"],
        };
  }

  /** Exposes core-derived actions instead of maintaining a parallel action model. */
  nextActions(): readonly {
    readonly workItemId: string;
    readonly actions: readonly string[];
  }[] {
    const ids = new Set(
      this.#options.plan.plan.slices.map((slice) => this.#workItemId(slice.id)),
    );
    return this.#options.core
      .statusSnapshot()
      .work.filter((work) => ids.has(work.workItemId))
      .map((work) => ({
        workItemId: work.workItemId,
        actions: work.nextLegalActions,
      }));
  }

  #activate(role: Role, seat: SchedulerSeatSpec): SchedulerIdentity {
    const { core } = this.#options;
    const key = `seat-${role}`;
    this.#mutate(key, (context) =>
      core.createSeat(context, {
        seatId: seat.seatId,
        name: seat.name,
        role: role as "PM" | "Supervisor" | "Developer" | "Verifier",
      }),
    );
    const actorKey = `actor-${role}`;
    const actor = this.#mutate(actorKey, (context) =>
      core.createActor(context, {
        displayName: seat.displayName,
        role,
        seatId: seat.seatId,
      }),
    );
    return Object.freeze({
      seatId: seat.seatId,
      actorId: actor.actorId,
      credential: actor.credential,
    });
  }

  #context(key: string): MutationContext {
    const { core, operatorCredential, mutationContexts } = this.#options;
    const stable = this.#stableKey(key);
    return mutationContexts.getOrCreate(stable, () => ({
      credential: operatorCredential,
      requestId: `scheduler-${stable}`,
      idempotencyKey: `scheduler-${stable}`,
      expectedVersion: core.stateVersion,
      inputRevision: core.inputRevision,
    }));
  }

  #mutate<T>(key: string, mutation: (context: MutationContext) => T): T {
    return mutation(this.#context(key));
  }

  #stableKey(key: string): string {
    return createHash("sha256")
      .update(`${this.#options.plan.hash}:${key}`)
      .digest("hex")
      .slice(0, 32);
  }

  #workItemId(sliceId: string): string {
    return `wf-${createHash("sha256").update(`${this.#options.plan.plan.taskId}:${sliceId}`).digest("hex").slice(0, 24)}`;
  }
}
