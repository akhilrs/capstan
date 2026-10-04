/** Resolves the [nexora], [architect] and [researcher] sections. */
import { researcherRuleProblems } from "../researcher-policy.js";
import {
  NAME_PATTERN,
  PRINTABLE_LINE,
  type Table,
  enumValue,
  guardCredentialShape,
  optionalBoolean,
  optionalInteger,
  optionalString,
  rejectUnknownKeys,
  stringList,
} from "./primitives.js";
import {
  ConfigError,
  DEFAULT_ARCHITECT_MAX_PACKAGES,
  DEFAULT_ARCHITECT_ROLE,
  DEFAULT_HIGH_RISK_TRIGGERS,
  DEFAULT_RESEARCHER_OUTPUT_DIR,
  DEFAULT_RESEARCHER_ROLE,
  DEFAULT_RESEARCHER_USER_AGENT,
  MAX_ARCHITECT_PACKAGES,
  NEXORA_DEFAULT_ACTIONS,
  NEXORA_TRACK_MODES,
  PLAN_REVIEW_MODES,
  type ResolvedArchitect,
  type ResolvedHost,
  type ResolvedNexora,
  type ResolvedOperator,
  type ResolvedResearcher,
  type ResolvedRole,
} from "./types.js";

export function resolveNexora(table: Table): ResolvedNexora {
  rejectUnknownKeys(table, ["track", "default_action"], "nexora");
  const nexora: ResolvedNexora = {
    track:
      table.track === undefined
        ? "ask"
        : enumValue(table.track, "nexora.track", NEXORA_TRACK_MODES),
    defaultAction:
      table.default_action === undefined
        ? "create"
        : enumValue(
            table.default_action,
            "nexora.default_action",
            NEXORA_DEFAULT_ACTIONS,
          ),
  };
  if (nexora.track === "always" && nexora.defaultAction === "none")
    throw new ConfigError(
      'nexora.track = "always" contradicts nexora.default_action = "none"; use track = "never" to turn tracking off',
    );
  return nexora;
}

export function resolveArchitect(
  table: Table,
  roles: readonly ResolvedRole[],
  hosts: ReadonlyMap<string, ResolvedHost>,
): ResolvedArchitect {
  rejectUnknownKeys(
    table,
    [
      "enabled",
      "role",
      "plan_review",
      "reviewer_role",
      "max_packages",
      "count_toward_worker_limit",
      "high_risk_triggers",
    ],
    "architect",
  );
  const role =
    optionalString(table.role, "architect.role", 32) ?? DEFAULT_ARCHITECT_ROLE;
  if (!NAME_PATTERN.test(role))
    throw new ConfigError(`architect.role must match ${NAME_PATTERN.source}`);
  const reviewerRole = optionalString(
    table.reviewer_role,
    "architect.reviewer_role",
    32,
  );
  if (reviewerRole !== null && !NAME_PATTERN.test(reviewerRole))
    throw new ConfigError(
      `architect.reviewer_role must match ${NAME_PATTERN.source}`,
    );
  const architect: ResolvedArchitect = {
    enabled: optionalBoolean(table.enabled, "architect.enabled", false),
    role,
    planReview:
      table.plan_review === undefined
        ? "high_risk"
        : enumValue(
            table.plan_review,
            "architect.plan_review",
            PLAN_REVIEW_MODES,
          ),
    reviewerRole,
    maxPackages: optionalInteger(
      table.max_packages,
      "architect.max_packages",
      1,
      MAX_ARCHITECT_PACKAGES,
      DEFAULT_ARCHITECT_MAX_PACKAGES,
    ),
    countTowardWorkerLimit: optionalBoolean(
      table.count_toward_worker_limit,
      "architect.count_toward_worker_limit",
      false,
    ),
    highRiskTriggers:
      table.high_risk_triggers === undefined
        ? DEFAULT_HIGH_RISK_TRIGGERS
        : stringList(table.high_risk_triggers, "architect.high_risk_triggers"),
  };
  if (!architect.enabled) return architect;

  const architectRole = roles.find((candidate) => candidate.name === role);
  if (architectRole === undefined)
    throw new ConfigError(
      `architect.role "${role}" does not name a configured role`,
    );
  if (architectRole.kind !== "Developer")
    throw new ConfigError(
      `architect.role "${role}" must be a Developer role; roles.${role}.kind is ${architectRole.kind}`,
    );
  const host = hosts.get(architectRole.host);
  if (host?.kind !== "claude")
    throw new ConfigError(
      `roles.${role}: the architect role needs a claude host so its deny rules are enforced; host ${architectRole.host} is ${host?.kind ?? "unknown"}`,
    );
  if (reviewerRole !== null) {
    const reviewer = roles.find((candidate) => candidate.name === reviewerRole);
    if (reviewer === undefined)
      throw new ConfigError(
        `architect.reviewer_role "${reviewerRole}" does not name a configured role`,
      );
    if (reviewer.kind !== "Verifier")
      throw new ConfigError(
        `architect.reviewer_role "${reviewerRole}" must be a Verifier role; roles.${reviewerRole}.kind is ${reviewer.kind}`,
      );
  }
  return architect;
}

export const OUTPUT_DIR_PATTERN = /^[A-Za-z0-9._-]+(?:\/[A-Za-z0-9._-]+)*$/;

export function resolveResearcher(
  table: Table,
  configured: boolean,
  roles: readonly ResolvedRole[],
  hosts: ReadonlyMap<string, ResolvedHost>,
  architect: ResolvedArchitect,
  operator: ResolvedOperator,
): ResolvedResearcher {
  rejectUnknownKeys(
    table,
    ["enabled", "role", "output_dir", "user_agent"],
    "researcher",
  );
  const role =
    optionalString(table.role, "researcher.role", 32) ??
    DEFAULT_RESEARCHER_ROLE;
  if (!NAME_PATTERN.test(role))
    throw new ConfigError(`researcher.role must match ${NAME_PATTERN.source}`);
  const outputDir =
    optionalString(table.output_dir, "researcher.output_dir", 200) ??
    DEFAULT_RESEARCHER_OUTPUT_DIR;
  if (
    !OUTPUT_DIR_PATTERN.test(outputDir) ||
    outputDir.split("/").some((part) => part === "." || part === "..")
  )
    throw new ConfigError(
      "researcher.output_dir must be a repo-relative directory with no '..', no leading '/' and only letters, digits, '.', '_', '-' and '/'",
    );
  const userAgent =
    optionalString(table.user_agent, "researcher.user_agent", 200) ??
    DEFAULT_RESEARCHER_USER_AGENT;
  if (!PRINTABLE_LINE.test(userAgent) || /["'`]/.test(userAgent))
    throw new ConfigError(
      "researcher.user_agent must be one printable line with no quote characters",
    );
  guardCredentialShape(userAgent, "researcher.user_agent");
  const researcher: ResolvedResearcher = {
    configured,
    enabled: optionalBoolean(table.enabled, "researcher.enabled", false),
    role,
    outputDir,
    userAgent,
  };
  if (!researcher.enabled) return researcher;

  if (architect.enabled && role === architect.role)
    throw new ConfigError(
      `researcher.role "${role}" must differ from architect.role`,
    );
  if (operator.configured && role === operator.role)
    throw new ConfigError(
      `researcher.role "${role}" must differ from operator.role`,
    );
  const researcherRole = roles.find((candidate) => candidate.name === role);
  if (researcherRole === undefined)
    throw new ConfigError(
      `researcher.role "${role}" does not name a configured role`,
    );
  const host = hosts.get(researcherRole.host);
  if (host?.kind !== "claude")
    throw new ConfigError(
      `roles.${role}: the researcher role needs a claude host so its allow and deny rules are enforced; host ${researcherRole.host} is ${host?.kind ?? "unknown"}`,
    );
  const [problem] = researcherRuleProblems(researcherRole, researcher);
  if (problem !== undefined) throw new ConfigError(problem);
  return researcher;
}
