/** Node options the daemon is started with, and the one place that takes them back out of its environment. */

/**
 * V8 lets the young generation grow to two 64 MB semi-spaces when a tick holds a few MB of rows while it allocates,
 * and never gives it back; a daemon's ticks are small, so it starts with a 2 MB cap (about 100 MB less resident).
 */
export const DAEMON_SEMI_SPACE_FLAG = "--max-semi-space-size=2";

/** Names the flag the daemon's spawn put into NODE_OPTIONS (standalone binary only), so the daemon can take exactly that back out. */
export const ADDED_NODE_OPTION_VARIABLE = "CAPSTAN_ADDED_NODE_OPTION";

/**
 * Called by the daemon right after it starts: takes the flag its spawn added to NODE_OPTIONS (and the variable that names
 * it) out of the daemon's own environment, keeping any NODE_OPTIONS the user set, so no child process of the daemon (herdr,
 * git, a launched agent, the restart helper) inherits the cap. V8 has already read the flag.
 */
export function restoreNodeOptions(env: NodeJS.ProcessEnv = process.env): void {
  const added = env[ADDED_NODE_OPTION_VARIABLE];
  if (added === undefined) return;
  delete env[ADDED_NODE_OPTION_VARIABLE];
  const options = env.NODE_OPTIONS ?? "";
  const rest =
    options === added
      ? ""
      : options.endsWith(` ${added}`)
        ? options.slice(0, -added.length - 1)
        : undefined;
  if (rest === undefined) return;
  if (rest === "") delete env.NODE_OPTIONS;
  else env.NODE_OPTIONS = rest;
}
