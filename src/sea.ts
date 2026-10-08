import path from "node:path";
import sea from "node:sea";
import { fileURLToPath } from "node:url";

export { restoreNodeOptions } from "./node-options.js";

/** True when running from the standalone single-executable binary. */
export function isSea(): boolean {
  return seaOverride ?? sea.isSea();
}

/** True only inside a real binary with embedded assets; never affected by setSeaForTests. */
export function hasEmbeddedAssets(): boolean {
  return sea.isSea();
}

let seaOverride: boolean | undefined;

/** Tests only: pretend to be (or not be) the standalone binary; undefined restores detection. */
export function setSeaForTests(value: boolean | undefined): void {
  seaOverride = value;
}

/**
 * The file that runs the CLI: the binary itself under SEA, otherwise dist/src/cli.js beside this
 * module.
 */
export function entryPath(): string {
  if (isSea()) return process.execPath;
  return path.join(path.dirname(fileURLToPath(import.meta.url)), "cli.js");
}

/** The command and arguments that re-run this CLI with `args`. */
export function selfCommand(
  args: readonly string[],
  cliPath: string = entryPath(),
): {
  command: string;
  args: string[];
} {
  if (isSea()) return { command: process.execPath, args: [...args] };
  return {
    command: process.execPath,
    args: [...process.execArgv, cliPath, ...args],
  };
}

/** Reads an asset embedded in the binary (for example `migrations/0001_initial.sql`). */
export function readAsset(name: string): Buffer {
  return Buffer.from(sea.getAsset(name));
}
