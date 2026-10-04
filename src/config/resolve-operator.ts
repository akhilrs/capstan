/** Resolves the [operator] and [prompt_relay] sections. */
import { autoApproveRuleProblem } from "../operator-policy.js";
import {
  NAME_PATTERN,
  type Table,
  optionalBoolean,
  optionalInteger,
  optionalString,
  rejectUnknownKeys,
  stringList,
} from "./primitives.js";
import {
  ConfigError,
  DEFAULT_OPERATOR_ROLE,
  MAX_OPERATOR_OUTPUT_TAIL_BYTES,
  OPERATOR_HARD_MAX_SESSION_MINUTES,
  OPERATOR_HARD_MAX_TIMEOUT_SECONDS,
  OPERATOR_REQUIRED_DENY,
  type ResolvedArchitect,
  type ResolvedHost,
  type ResolvedOperator,
  type ResolvedPromptRelay,
  type ResolvedRole,
} from "./types.js";

export const OPERATOR_ALLOW_RULE = /^Bash\(cstan[ :][^()`$;&|<>\\\n]*\)$/;

export function operatorRules(value: unknown, at: string): string[] {
  const rules = stringList(value, at);
  rules.forEach((rule, index) => {
    const problem = autoApproveRuleProblem(rule);
    if (problem !== null) throw new ConfigError(`${at}[${index}] ${problem}`);
  });
  return rules;
}

export function resolvePromptRelay(
  table: Table,
  present: boolean,
): ResolvedPromptRelay {
  rejectUnknownKeys(table, ["enabled", "capture_ttl_seconds"], "prompt_relay");
  return {
    present,
    enabled: optionalBoolean(table.enabled, "prompt_relay.enabled", false),
    captureTtlSeconds: optionalInteger(
      table.capture_ttl_seconds,
      "prompt_relay.capture_ttl_seconds",
      60,
      3600,
      600,
    ),
  };
}

export function resolveOperator(
  table: Table,
  configured: boolean,
  roles: readonly ResolvedRole[],
  hosts: ReadonlyMap<string, ResolvedHost>,
  architect: ResolvedArchitect,
): ResolvedOperator {
  rejectUnknownKeys(
    table,
    [
      "enabled",
      "role",
      "auto_approve",
      "auto_approve_prefix",
      "timeout_seconds",
      "max_timeout_seconds",
      "output_tail_bytes",
      "proposal_ttl_minutes",
      "approval_ttl_minutes",
      "max_pending_proposals",
      "count_toward_worker_limit",
      "restart_health_timeout_seconds",
      "restart_idle_wait_seconds",
      "session_grant_max_minutes",
      "full_auto_default_minutes",
      "full_auto_max_minutes",
    ],
    "operator",
  );
  const role =
    optionalString(table.role, "operator.role", 32) ?? DEFAULT_OPERATOR_ROLE;
  if (!NAME_PATTERN.test(role))
    throw new ConfigError(`operator.role must match ${NAME_PATTERN.source}`);
  const operator: ResolvedOperator = {
    configured,
    enabled: optionalBoolean(table.enabled, "operator.enabled", false),
    role,
    autoApprove: operatorRules(table.auto_approve, "operator.auto_approve"),
    autoApprovePrefix: operatorRules(
      table.auto_approve_prefix,
      "operator.auto_approve_prefix",
    ),
    timeoutSeconds: optionalInteger(
      table.timeout_seconds,
      "operator.timeout_seconds",
      1,
      OPERATOR_HARD_MAX_TIMEOUT_SECONDS,
      300,
    ),
    maxTimeoutSeconds: optionalInteger(
      table.max_timeout_seconds,
      "operator.max_timeout_seconds",
      1,
      OPERATOR_HARD_MAX_TIMEOUT_SECONDS,
      1800,
    ),
    outputTailBytes: optionalInteger(
      table.output_tail_bytes,
      "operator.output_tail_bytes",
      1,
      MAX_OPERATOR_OUTPUT_TAIL_BYTES,
      8192,
    ),
    proposalTtlMinutes: optionalInteger(
      table.proposal_ttl_minutes,
      "operator.proposal_ttl_minutes",
      1,
      1440,
      60,
    ),
    approvalTtlMinutes: optionalInteger(
      table.approval_ttl_minutes,
      "operator.approval_ttl_minutes",
      1,
      120,
      10,
    ),
    maxPendingProposals: optionalInteger(
      table.max_pending_proposals,
      "operator.max_pending_proposals",
      1,
      50,
      5,
    ),
    countTowardWorkerLimit: optionalBoolean(
      table.count_toward_worker_limit,
      "operator.count_toward_worker_limit",
      false,
    ),
    restartHealthTimeoutSeconds: optionalInteger(
      table.restart_health_timeout_seconds,
      "operator.restart_health_timeout_seconds",
      1,
      600,
      60,
    ),
    restartIdleWaitSeconds: optionalInteger(
      table.restart_idle_wait_seconds,
      "operator.restart_idle_wait_seconds",
      0,
      3600,
      120,
    ),
    sessionGrantMaxMinutes: optionalInteger(
      table.session_grant_max_minutes,
      "operator.session_grant_max_minutes",
      1,
      OPERATOR_HARD_MAX_SESSION_MINUTES,
      60,
    ),
    fullAutoDefaultMinutes: optionalInteger(
      table.full_auto_default_minutes,
      "operator.full_auto_default_minutes",
      1,
      OPERATOR_HARD_MAX_SESSION_MINUTES,
      30,
    ),
    fullAutoMaxMinutes: optionalInteger(
      table.full_auto_max_minutes,
      "operator.full_auto_max_minutes",
      1,
      OPERATOR_HARD_MAX_SESSION_MINUTES,
      120,
    ),
  };
  if (operator.fullAutoDefaultMinutes > operator.fullAutoMaxMinutes)
    throw new ConfigError(
      `operator.full_auto_default_minutes (${operator.fullAutoDefaultMinutes}) must not exceed operator.full_auto_max_minutes (${operator.fullAutoMaxMinutes})`,
    );
  if (operator.timeoutSeconds > operator.maxTimeoutSeconds)
    throw new ConfigError(
      `operator.timeout_seconds (${operator.timeoutSeconds}) must not exceed operator.max_timeout_seconds (${operator.maxTimeoutSeconds})`,
    );
  if (!operator.enabled) return operator;

  if (role === architect.role)
    throw new ConfigError(
      `operator.role "${role}" must differ from architect.role`,
    );
  const operatorRole = roles.find((candidate) => candidate.name === role);
  if (operatorRole === undefined)
    throw new ConfigError(
      `operator.role "${role}" does not name a configured role`,
    );
  if (operatorRole.kind !== "Developer")
    throw new ConfigError(
      `operator.role "${role}" must be a Developer role; roles.${role}.kind is ${operatorRole.kind}`,
    );
  const host = hosts.get(operatorRole.host);
  if (host?.kind !== "claude")
    throw new ConfigError(
      `roles.${role}: the operator role needs a claude host so its deny rules are enforced; host ${operatorRole.host} is ${host?.kind ?? "unknown"}`,
    );
  operatorRole.allow.forEach((rule, index) => {
    if (!OPERATOR_ALLOW_RULE.test(rule))
      throw new ConfigError(
        `roles.${role}.allow[${index}] must be a Bash(cstan ...) rule; the operator role may not allow ${rule}`,
      );
  });
  for (const required of OPERATOR_REQUIRED_DENY)
    if (!operatorRole.deny.includes(required))
      throw new ConfigError(
        `roles.${role}.deny must include ${required}: the operator role may not read or edit project files or start subagents`,
      );
  return operator;
}
