import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";

const created = new Set<string>();

/**
 * A temp directory that is removed when the test process exits, whether or not
 * the test that made it reached its own cleanup. A test that spawns a child
 * still stops the child before removing the directory; this is the backstop for
 * a test that failed halfway.
 */
export function tempDir(prefix: string): string {
  const directory = mkdtempSync(path.join(tmpdir(), prefix));
  created.add(directory);
  return directory;
}

/** Removes a directory made by tempDir now, so it does not wait for exit. */
export function removeTempDir(directory: string): void {
  rmSync(directory, { recursive: true, force: true });
  created.delete(directory);
}

process.on("exit", () => {
  for (const directory of created) {
    rmSync(directory, { recursive: true, force: true });
  }
});
