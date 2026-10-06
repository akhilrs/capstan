/** status: the operator's view of the run. */
import { type ResolvedOperator } from "../config/capstan-config.js";
import { DEFAULT_PM_STALE_MINUTES } from "../config/types.js";
import { pmMailSummary } from "../pm-mail.js";
import {
  MAX_STATUS_MESSAGES,
  MAX_STATUS_CLEARS,
  describeGrantRecord,
  describeFullAuto,
  type CommandHandler,
  ok,
  mapError,
  MAX_STATUS_REPORTS,
  MAX_STATUS_PROPOSALS,
  MAX_STATUS_TASK_PLANS,
  MAX_STATUS_TASK_PACKAGES,
  MAX_STATUS_TASK_REQUIREMENTS,
  type CommandEnv,
} from "./shared.js";

export function statusHandlers(
  env: CommandEnv,
): Record<string, CommandHandler> {
  const { deps, core, agentOf, now } = env;
  return {
    status(call) {
      try {
        const { supervision: legacy, ...snapshot } =
          core.statusSnapshot() as unknown as Record<string, unknown>;
        const result: Record<string, unknown> = {
          ...snapshot,
          agents: core.listAgents(),
          // The live loop's state; `legacySupervision` is the old control row, which nothing enables any more.
          supervisionState: {
            enabled: deps.config?.supervision?.enabled === true,
            checkSeconds: deps.config?.supervision?.checkSeconds ?? null,
            ...core.supervisionActivity(),
          },
          legacySupervision: legacy,
        };
        if (deps.controllerLocation !== undefined)
          result.controller = {
            pid: process.pid,
            ...deps.controllerLocation,
          };
        if (call.identity.role === "operator") {
          const unresolved = core.unresolvedMessages(
            call.credential,
            MAX_STATUS_MESSAGES,
          );
          const snapshot = deps.driverSnapshot?.() ?? {
            stalledAgentIds: [],
            stuck: [],
          };
          result.messages = unresolved.messages.map((m) => ({
            messageId: m.messageId,
            recipientAgentId: m.recipientAgentId,
            state: m.state,
            sequence: m.sequence,
            queuedAt: m.queuedAt,
            deferredReason: m.deferredReason,
            stateReason: m.stateReason,
            lastNotifiedAt: m.lastNotifiedAt,
          }));
          result.legacySupervisionReason = core.supervisionReason(
            call.credential,
          );
          result.messagesTruncated = unresolved.truncated;
          const pm = core
            .listAgents()
            .find((a) => a.kind === "PM" && a.state === "active");
          result.pmMail = null;
          if (pm !== undefined) {
            const summary = pmMailSummary(
              core.messagesFor(pm.agentId),
              now(),
              (deps.config?.notifications?.pmStaleMinutes ??
                DEFAULT_PM_STALE_MINUTES) * 60,
            );
            result.pmMail = {
              agentId: pm.agentId,
              ...summary,
              notified: snapshot.pmStale?.notified ?? false,
            };
          }
          result.stalledAgentIds = snapshot.stalledAgentIds;
          result.lostAgentIds = snapshot.lostAgentIds ?? [];
          result.stuck = snapshot.stuck;
          result.inputClears = core.inputClears(
            call.credential,
            MAX_STATUS_CLEARS,
          );
          const operatorConfig: ResolvedOperator | undefined =
            deps.config?.operator;
          if (operatorConfig?.enabled === true && deps.operator !== undefined)
            result.operator = {
              fullAuto: describeFullAuto(deps.operator.fullAutoStatus()),
              grants: deps.operator.grants().map(describeGrantRecord),
            };
          result.panes = core.agentPanes(call.credential);
          result.reviews = core
            .reviews(call.credential, MAX_STATUS_REPORTS)
            .map((r) => ({
              reviewId: r.reviewId,
              reportId: r.reportId,
              integrationId: r.integrationId,
              round: r.round,
              state: r.state,
              authorAgentId: r.authorAgentId,
              reviewerAgentId: r.reviewerAgentId,
              announced: r.notifiedMessageId !== null,
              createdAt: r.createdAt,
            }));
          result.integrations = core
            .integrations(call.credential, MAX_STATUS_REPORTS)
            .map((i) => ({
              integrationId: i.integrationId,
              state: i.state,
              branch: i.state === "merged" ? i.branch : null,
              reports: i.reports.map((r) => r.reportId),
              conflictReportId: i.conflictReportId,
              createdAt: i.createdAt,
            }));
          result.pipelineCounts = core.pipelineCounts(call.credential);
          result.activeTasks = core.activeTasks(call.credential, {
            plans: MAX_STATUS_TASK_PLANS,
            packages: MAX_STATUS_TASK_PACKAGES,
            requirements: MAX_STATUS_TASK_REQUIREMENTS,
          });
          result.awaitingConfirm = core.awaitingConfirm(
            call.credential,
            MAX_STATUS_REPORTS,
          );
          if (operatorConfig?.enabled === true && deps.operator !== undefined)
            result.pendingProposals = deps.operator
              .list({ states: ["proposed"], limit: MAX_STATUS_PROPOSALS })
              .map((p) => ({
                proposalId: p.proposalId,
                kind: p.kind,
                command: p.command,
                proposer: p.proposerAgentId,
                reason: p.reason,
                createdAt: p.createdAt,
              }));
          result.agentFindings = core
            .findings(call.credential, MAX_STATUS_REPORTS)
            .map((f) => ({
              findingId: f.findingId,
              targetAgentId: f.targetAgentId,
              raisedByAgentId: f.raisedByAgentId,
              severity: f.severity,
              state: f.state,
              interventions: f.interventions,
              stateReason: f.stateReason,
              createdAt: f.createdAt,
            }));
          result.reports = core
            .agentReports(call.credential, MAX_STATUS_REPORTS)
            .map((r) => ({
              reportId: r.reportId,
              agentId: r.agentId,
              generation: r.generation,
              commitSha: r.commitSha,
              state: r.state,
              reason: r.reason,
              announced: r.notifiedMessageId !== null,
              createdAt: r.createdAt,
            }));
          if (deps.launcher !== undefined) {
            const launcherStatus = deps.launcher.status() as Record<
              string,
              unknown
            >;
            result.cleanupFailed = launcherStatus.cleanupFailed;
            result.orphanPanes = launcherStatus.orphanPanes;
          }
        }
        if (deps.config?.promptRelay?.enabled === true)
          result.promptRelay = core.promptRelayStatus();
        if (agentOf(call.identity)?.kind === "PM")
          result.nexoraDrift = core.syncDrift(call.credential).map((l) => ({
            refKind: l.refKind,
            refId: l.refId,
            externalId: l.externalId,
            syncedState: l.syncedState,
            wanted: l.wanted,
          }));
        return ok(result);
      } catch (error) {
        return mapError(error);
      }
    },
  };
}
