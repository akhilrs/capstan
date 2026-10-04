/** op: the Operator agent's proposals and grants. */
import { ControllerError, RESTART_COMMAND_TEXT } from "../controller/core.js";
import { OperatorError } from "../operator.js";
import { type CommandResponse } from "../daemon.js";
import {
  SAFE_AGENT_ID,
  MAX_STATUS_PROPOSALS,
  describeGrantRecord,
  describeFullAuto,
  describeProposal,
  type CommandHandler,
  ok,
  fail,
  mapError,
  type CommandEnv,
} from "./shared.js";

export function operatorHandlers(
  env: CommandEnv,
): Record<string, CommandHandler> {
  const { deps, log, agentOf } = env;
  return {
    op(call) {
      const config = deps.config;
      const service = deps.operator;
      if (
        config === undefined ||
        !config.operator.enabled ||
        service === undefined
      )
        return fail(
          "not_configured",
          "operator commands need [operator] enabled = true in capstan.toml",
        );
      const [sub, ...rest] = call.args;
      if (
        sub !== "propose" &&
        sub !== "decide" &&
        sub !== "show" &&
        sub !== "cancel" &&
        sub !== "grants" &&
        sub !== "revoke" &&
        sub !== "full-auto"
      )
        return fail(
          "invalid_request",
          "op needs propose, decide, show, cancel, grants, revoke or full-auto",
        );
      const caller = agentOf(call.identity);
      const refuse = (error: unknown): CommandResponse => {
        if (error instanceof OperatorError)
          return fail("rejected", `${error.code}: ${error.message}`);
        if (error instanceof ControllerError)
          return fail("rejected", error.message);
        return mapError(error);
      };
      const isOperatorAgent =
        caller?.state === "active" &&
        caller.kind === "Developer" &&
        caller.roleName === config.operator.role;
      const isActivePm = caller?.state === "active" && caller.kind === "PM";
      const isCli = call.identity.role === "operator";
      try {
        if (sub === "propose") {
          if (!isOperatorAgent)
            return fail(
              "forbidden",
              "only the designated operator agent proposes a command",
            );
          let kind: "command" | "restart" = "command";
          let forceRestart = false;
          let command: string;
          let reason: string;
          if (rest[0] === "--restart") {
            kind = "restart";
            const tail = rest.slice(1);
            if (tail[0] === "--force") {
              forceRestart = true;
              tail.shift();
            }
            if (tail.length !== 1)
              return fail(
                "invalid_request",
                "op propose --restart needs [--force] and one reason",
              );
            command = RESTART_COMMAND_TEXT;
            reason = tail[0]!;
          } else {
            if (rest.length !== 2)
              return fail(
                "invalid_request",
                'op propose needs "<command>" and "<reason>"',
              );
            if (rest[0]!.startsWith("-"))
              return fail(
                "invalid_request",
                "a command cannot start with -; the only options are --restart and --force",
              );
            [command, reason] = rest as [string, string];
          }
          log("operator_propose_requested", { agentId: caller.agentId, kind });
          const proposal = service.propose(call.credential, {
            kind,
            command,
            reason,
            forceRestart,
          });
          return ok(describeProposal(proposal));
        }
        if (sub === "decide") {
          const [proposalId, decision, ...tail] = rest;
          if (
            proposalId === undefined ||
            (decision !== "approve" && decision !== "deny")
          )
            return fail(
              "invalid_request",
              'op decide needs a proposal id and approve --hash <hash12> or deny ["<note>"]',
            );
          if (decision === "approve") {
            if (isCli)
              return fail(
                "rejected",
                "approve_requires_pm: only an active PM agent approves an operator proposal; the operator may deny, cancel and show",
              );
            if (!isActivePm)
              return fail("forbidden", "only an active PM approves a proposal");
            const usage =
              'op decide approve needs --hash <hash12> and may add --session exact or --session prefix="<words>"; it takes no --force, the force value is part of the proposal';
            if (tail[0] !== "--hash" || tail[1] === undefined)
              return fail("invalid_request", usage);
            let session:
              { kind: "exact" | "prefix"; text?: string } | undefined;
            if (tail.length > 2) {
              const wanted = tail[3];
              if (
                tail.length !== 4 ||
                tail[2] !== "--session" ||
                wanted === undefined
              )
                return fail("invalid_request", usage);
              if (wanted === "exact") session = { kind: "exact" };
              else if (wanted.startsWith("prefix="))
                session = { kind: "prefix", text: wanted.slice(7) };
              else return fail("invalid_request", usage);
            }
            return ok(
              describeProposal(
                service.decide(call.credential, {
                  proposalId,
                  decision,
                  hash: tail[1],
                  ...(session === undefined ? {} : { session }),
                }),
              ),
            );
          }
          if (!isCli && !isActivePm)
            return fail("forbidden", "only the PM or the operator denies");
          if (tail.length > 1)
            return fail(
              "invalid_request",
              "op decide deny takes at most one note; there is no --force at decide time",
            );
          return ok(
            describeProposal(
              service.decide(call.credential, {
                proposalId,
                decision,
                ...(tail[0] === undefined ? {} : { note: tail[0] }),
              }),
            ),
          );
        }
        if (sub === "grants") {
          if (!isCli && !isActivePm)
            return fail(
              "forbidden",
              "only the PM or the operator lists grants",
            );
          if (rest.length !== 0)
            return fail("invalid_request", "op grants takes no argument");
          return ok({ grants: service.grants().map(describeGrantRecord) });
        }
        if (sub === "revoke") {
          if (!isCli && !isActivePm)
            return fail("forbidden", "only the PM or the operator revokes");
          if (rest.length !== 1 || !SAFE_AGENT_ID.test(rest[0]!))
            return fail("invalid_request", "op revoke needs one grant id");
          return ok(
            describeGrantRecord(service.revokeGrant(call.credential, rest[0]!)),
          );
        }
        if (sub === "full-auto") {
          const [action, ...args] = rest;
          if (action === "status") {
            if (!isCli && !isActivePm && !isOperatorAgent)
              return fail("forbidden", "the caller may not read full auto");
            if (args.length !== 0)
              return fail(
                "invalid_request",
                "op full-auto status takes no argument",
              );
            return ok(describeFullAuto(service.fullAutoStatus()));
          }
          if (action === "off") {
            if (!isCli && !isActivePm)
              return fail(
                "forbidden",
                "only the PM or the operator switches full auto off",
              );
            if (args.length !== 0)
              return fail(
                "invalid_request",
                "op full-auto off takes no argument",
              );
            return ok(describeFullAuto(service.fullAutoOff(call.credential)));
          }
          if (action === "on") {
            if (isCli)
              return fail(
                "rejected",
                "full_auto_requires_pm: the user switches full auto off; only the PM switches it on, after asking the user",
              );
            if (!isActivePm)
              return fail(
                "forbidden",
                "only an active PM switches full auto on",
              );
            const minutesText = /^\d+$/.test(args[0] ?? "")
              ? args.shift()
              : undefined;
            if (
              args.length !== 2 ||
              args[0] !== "--asked-user" ||
              args[1]!.trim() === ""
            )
              return fail(
                "invalid_request",
                'op full-auto on [<minutes>] --asked-user "<what the user said>"',
              );
            return ok(
              describeFullAuto(
                service.fullAutoOn(call.credential, {
                  ...(minutesText === undefined
                    ? {}
                    : { minutes: Number(minutesText) }),
                  askedUser: args[1]!,
                }),
              ),
            );
          }
          return fail(
            "invalid_request",
            "op full-auto needs on, off or status",
          );
        }
        if (sub === "cancel") {
          if (rest.length !== 1 || !SAFE_AGENT_ID.test(rest[0]!))
            return fail("invalid_request", "op cancel needs one proposal id");
          if (!isCli && !isActivePm && !isOperatorAgent)
            return fail("forbidden", "the caller may not cancel a proposal");
          return ok(
            describeProposal(service.cancel(call.credential, rest[0]!)),
          );
        }
        if (!isCli && !isActivePm && !isOperatorAgent)
          return fail("forbidden", "the caller may not show proposals");
        if (rest.length > 1)
          return fail("invalid_request", "op show takes at most one id");
        const own = isOperatorAgent && caller !== undefined;
        if (rest[0] !== undefined) {
          if (!SAFE_AGENT_ID.test(rest[0]))
            return fail("invalid_request", "the proposal id is not valid");
          const proposal = service.show(rest[0]);
          if (
            proposal === undefined ||
            (own && proposal.proposerAgentId !== caller.agentId)
          )
            return fail("rejected", `unknown_proposal: no proposal ${rest[0]}`);
          return ok(describeProposal(proposal));
        }
        return ok({
          proposals: service
            .list({
              limit: MAX_STATUS_PROPOSALS,
              ...(own ? { proposerAgentId: caller.agentId } : {}),
            })
            .map(describeProposal),
        });
      } catch (error) {
        return refuse(error);
      }
    },
  };
}
