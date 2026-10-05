import type { ControllerKernel } from "./kernel.js";
import { ActorsArea } from "./actors.js";
import { AgentsArea } from "./agents.js";
import { FindingsArea } from "./findings.js";
import { MessageNoticesArea } from "./message-notices.js";
import { MessagesArea } from "./messages.js";
import { PanesArea } from "./panes.js";
import { PausesArea } from "./pauses.js";
import { PromptRelayArea } from "./prompt-relay.js";
import type {
  ExternalLinkRecord,
  ExternalLinkRow,
  PlanRow,
  PmRestartSummary,
  ControllerStatus,
} from "./records.js";

/**
 * Members of the facade that the extracted areas still call and that have not moved out of core.ts yet.
 * Each later extraction removes the entries its area takes over.
 */
export interface CoreHooks {
  statusSnapshot(): ControllerStatus;
  linkRecord(row: ExternalLinkRow): ExternalLinkRecord;
  linkRow(refKind: string, refId: string): ExternalLinkRow | undefined;
  planRow(planId: string): PlanRow | undefined;
  openPlansForSummary(): NonNullable<PmRestartSummary["plans"]>[number][];
  mergedIntegrationsForSummary(): NonNullable<
    PmRestartSummary["integrations"]
  >[number][];
  cancelUnstartedOperatorProposalsOf(agentId: string, now: string): void;
  endOperatorGrantsOf(agentId: string): void;
}

/** One field per extracted area; the facade in core.ts delegates to these. */
export interface ControllerAreas {
  readonly core: CoreHooks;
  readonly actors: ActorsArea;
  readonly agents: AgentsArea;
  readonly panes: PanesArea;
  readonly messages: MessagesArea;
  readonly messageNotices: MessageNoticesArea;
  readonly pauses: PausesArea;
  readonly findings: FindingsArea;
  readonly promptRelay: PromptRelayArea;
}

export function createAreas(
  kernel: ControllerKernel,
  core: CoreHooks,
): ControllerAreas {
  const areas = {} as {
    -readonly [K in keyof ControllerAreas]: ControllerAreas[K];
  };
  areas.core = core;
  areas.actors = new ActorsArea(kernel, areas);
  areas.agents = new AgentsArea(kernel, areas);
  areas.panes = new PanesArea(kernel, areas);
  areas.messages = new MessagesArea(kernel, areas);
  areas.messageNotices = new MessageNoticesArea(kernel, areas);
  areas.pauses = new PausesArea(kernel, areas);
  areas.findings = new FindingsArea(kernel, areas);
  areas.promptRelay = new PromptRelayArea(kernel);
  return areas;
}
