export const NOT_A_TERMINAL_MESSAGE =
  'dash needs an interactive terminal; use "cstan status" (or "cstan status --watch") for scripts and pipes';

export function hasTerminal(): boolean {
  return process.stdin.isTTY === true && process.stdout.isTTY === true;
}
