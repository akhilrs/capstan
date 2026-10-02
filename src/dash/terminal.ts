export const NOT_A_TERMINAL_MESSAGE =
  'dash needs an interactive terminal; use "cstan status" (or "cstan status --watch") for scripts and pipes';

export function hasTerminal(): boolean {
  return process.stdin.isTTY === true && process.stdout.isTTY === true;
}

/**
 * Whether the locale names a UTF-8 charset. The first of LC_ALL, LC_CTYPE and
 * LANG that is set decides; with none set the terminal is assumed to be UTF-8.
 */
export function isUtf8Locale(env: NodeJS.ProcessEnv): boolean {
  const value = [env.LC_ALL, env.LC_CTYPE, env.LANG].find(
    (v) => v !== undefined && v !== "",
  );
  return value === undefined || /utf-?8/i.test(value);
}

/** ASCII glyphs are used on a dumb terminal and under a non-UTF-8 locale. */
export function wantsAscii(env: NodeJS.ProcessEnv): boolean {
  return env.TERM === "dumb" || !isUtf8Locale(env);
}
