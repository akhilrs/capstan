import type { ControllerKernel } from "./kernel.js";
import { ActorsArea } from "./actors.js";
import { AgentsArea } from "./agents.js";
import { FindingsArea } from "./findings.js";
import { MessageNoticesArea } from "./message-notices.js";
import { MessagesArea } from "./messages.js";
import { PanesArea } from "./panes.js";
import { PausesArea } from "./pauses.js";
import { PromptRelayArea } from "./prompt-relay.js";
import { ReportsArea } from "./reports.js";
import { ReviewsArea } from "./reviews.js";
import { IntegrationsArea } from "./integrations.js";
import { PlansArea } from "./plans.js";
import { PlanPackagesArea } from "./plan-packages.js";
import { LinksArea } from "./links.js";
import { OperatorProposalsArea } from "./operator-proposals.js";
import { OperatorGrantsArea } from "./operator-grants.js";
import { OperatorRunsArea } from "./operator-runs.js";
import { StatusArea } from "./status.js";
import { ReconcileArea } from "./reconcile.js";

/** One field per extracted area; the facade in core.ts delegates to these. */
export interface ControllerAreas {
  readonly actors: ActorsArea;
  readonly agents: AgentsArea;
  readonly panes: PanesArea;
  readonly messages: MessagesArea;
  readonly messageNotices: MessageNoticesArea;
  readonly pauses: PausesArea;
  readonly findings: FindingsArea;
  readonly promptRelay: PromptRelayArea;
  readonly reports: ReportsArea;
  readonly reviews: ReviewsArea;
  readonly integrations: IntegrationsArea;
  readonly plans: PlansArea;
  readonly planPackages: PlanPackagesArea;
  readonly links: LinksArea;
  readonly operatorProposals: OperatorProposalsArea;
  readonly operatorGrants: OperatorGrantsArea;
  readonly operatorRuns: OperatorRunsArea;
  readonly status: StatusArea;
  readonly reconcile: ReconcileArea;
}

export function createAreas(kernel: ControllerKernel): ControllerAreas {
  const areas = {} as {
    -readonly [K in keyof ControllerAreas]: ControllerAreas[K];
  };
  areas.actors = new ActorsArea(kernel, areas);
  areas.agents = new AgentsArea(kernel, areas);
  areas.panes = new PanesArea(kernel, areas);
  areas.messages = new MessagesArea(kernel, areas);
  areas.messageNotices = new MessageNoticesArea(kernel, areas);
  areas.pauses = new PausesArea(kernel, areas);
  areas.findings = new FindingsArea(kernel, areas);
  areas.promptRelay = new PromptRelayArea(kernel);
  areas.reports = new ReportsArea(kernel, areas);
  areas.reviews = new ReviewsArea(kernel, areas);
  areas.integrations = new IntegrationsArea(kernel, areas);
  areas.plans = new PlansArea(kernel, areas);
  areas.planPackages = new PlanPackagesArea(kernel, areas);
  areas.links = new LinksArea(kernel, areas);
  areas.operatorProposals = new OperatorProposalsArea(kernel, areas);
  areas.operatorGrants = new OperatorGrantsArea(kernel, areas);
  areas.operatorRuns = new OperatorRunsArea(kernel, areas);
  areas.status = new StatusArea(kernel, areas);
  areas.reconcile = new ReconcileArea(kernel, areas);
  return areas;
}
