/** pause, resume, launch, spawn, replace, release and pm-restart: the commands that manage agents. */
import { NAME_PATTERN } from "../config/capstan-config.js";
import { BRANCH_TYPES } from "../conventions.js";
import {
  SAFE_AGENT_ID,
  type CommandHandler,
  ok,
  fail,
  mapError,
  type CommandEnv,
} from "./shared.js";

export function agentHandlers(env: CommandEnv): Record<string, CommandHandler> {
  const { deps, core, log, workerManager, changePause } = env;
  return {
    async pause(call) {
      return await changePause(call, true);
    },

    async resume(call) {
      return await changePause(call, false);
    },

    async launch(call) {
      try {
        core.assertRunNotPaused("launch");
      } catch (error) {
        return mapError(error);
      }
      if (deps.launcher === undefined)
        return fail(
          "not_configured",
          "launching agents needs capstan.toml and Herdr",
        );
      if (call.args.length !== 0)
        return fail("invalid_request", "launch takes no arguments");
      try {
        return ok(await deps.launcher.launchPm());
      } catch (error) {
        return mapError(error);
      }
    },

    async spawn(call) {
      const requestedBy = workerManager(call.identity);
      if (requestedBy === undefined)
        return fail(
          "forbidden",
          "only the PM or the operator may spawn workers",
        );
      const [roleName, ...flags] = call.args;
      const usage =
        "spawn needs a role name and takes --task <plan-id>/<package-id> or <requirement-ref-id>, --type <type> and --title <text>";
      if (roleName === undefined) return fail("invalid_request", usage);
      if (!NAME_PATTERN.test(roleName))
        return fail("invalid_request", "the role name is not valid");
      const named: { task?: string; type?: string; title?: string } = {};
      for (let i = 0; i < flags.length; i += 2) {
        const flag = flags[i];
        const value = flags[i + 1];
        if (
          (flag !== "--task" && flag !== "--type" && flag !== "--title") ||
          value === undefined ||
          named[flag.slice(2) as "task" | "type" | "title"] !== undefined
        )
          return fail("invalid_request", usage);
        named[flag.slice(2) as "task" | "type" | "title"] = value;
      }
      if (
        named.task !== undefined &&
        !/^[A-Za-z0-9][A-Za-z0-9._:-]{0,127}(\/[A-Za-z0-9][A-Za-z0-9._:-]{0,127})?$/.test(
          named.task,
        )
      )
        return fail(
          "invalid_request",
          "--task must be <plan-id>/<package-id> or a requirement ref id",
        );
      if (
        named.type !== undefined &&
        !(BRANCH_TYPES as readonly string[]).includes(named.type)
      )
        return fail(
          "invalid_request",
          `--type must be one of ${BRANCH_TYPES.join(", ")}`,
        );
      if (named.title !== undefined && !/\S/.test(named.title))
        return fail("invalid_request", "--title must not be empty");
      try {
        core.assertRunNotPaused("spawn");
      } catch (error) {
        return mapError(error);
      }
      if (deps.launcher === undefined)
        return fail(
          "not_configured",
          "spawning agents needs capstan.toml and Herdr",
        );
      log("spawn_requested", { requestedBy, role: roleName, ...named });
      try {
        return ok(await deps.launcher.spawn(roleName, named));
      } catch (error) {
        return mapError(error);
      }
    },

    async replace(call) {
      const requestedBy = workerManager(call.identity);
      if (requestedBy === undefined)
        return fail(
          "forbidden",
          "only the PM or the operator may replace workers",
        );
      if (call.args.length !== 1)
        return fail("invalid_request", "replace needs one agent id");
      const agentId = call.args[0]!;
      if (!SAFE_AGENT_ID.test(agentId))
        return fail("invalid_request", "the agent id is not valid");
      if (deps.launcher === undefined)
        return fail(
          "not_configured",
          "replacing agents needs capstan.toml and Herdr",
        );
      log("replace_requested", { requestedBy, agentId });
      try {
        return ok(await deps.launcher.replace(agentId));
      } catch (error) {
        return mapError(error);
      }
    },

    async release(call) {
      const requestedBy = workerManager(call.identity);
      if (requestedBy === undefined)
        return fail(
          "forbidden",
          "only the PM or the operator may release workers",
        );
      if (call.args.length !== 1)
        return fail("invalid_request", "release needs one agent id");
      const agentId = call.args[0]!;
      if (!SAFE_AGENT_ID.test(agentId))
        return fail("invalid_request", "the agent id is not valid");
      if (deps.launcher === undefined)
        return fail(
          "not_configured",
          "releasing agents needs capstan.toml and Herdr",
        );
      log("release_requested", { requestedBy, agentId });
      try {
        return ok(await deps.launcher.release(agentId));
      } catch (error) {
        return mapError(error);
      }
    },

    async "pm-restart"(call) {
      if (deps.launcher === undefined)
        return fail(
          "not_configured",
          "restarting the PM needs capstan.toml and Herdr",
        );
      if (call.args.length !== 0)
        return fail("invalid_request", "pm restart takes no arguments");
      try {
        return ok(await deps.launcher.restartPm());
      } catch (error) {
        return mapError(error);
      }
    },
  };
}
