import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdtempSync, readdirSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { test } from "node:test";

const root = path.resolve(import.meta.dirname, "..", "..");
const dashDirectory = path.join(root, "src", "dash");

function sources(directory: string): string[] {
  return readdirSync(directory, { withFileTypes: true }).flatMap((entry) => {
    const full = path.join(directory, entry.name);
    if (entry.isDirectory()) return sources(full);
    return /\.tsx?$/.test(entry.name) ? [full] : [];
  });
}

test("the dashboard never touches the database or imports core values", () => {
  const files = sources(dashDirectory);
  assert.ok(files.length > 5);
  for (const file of files) {
    const text = readFileSync(file, "utf8");
    for (const match of text.matchAll(
      /^import\s+(type\s+)?[^;]*?from\s+"([^"]+)"/gms,
    )) {
      const [, typeOnly, specifier] = match;
      assert.doesNotMatch(
        specifier!,
        /better-sqlite3|node:sqlite|controller\/sqlite/,
        file,
      );
      assert.doesNotMatch(specifier!, /controller\/database/, file);
      if (/controller\/core/.test(specifier!))
        assert.ok(typeOnly, `${file} imports a value from controller/core`);
    }
    assert.doesNotMatch(
      text,
      /openDatabase|openReadOnly|ControllerCore\.open/,
      file,
    );
  }
});

function cstan(args: string[], cwd: string) {
  const env = { ...process.env };
  delete env.CAPSTAN_TOKEN;
  delete env.CAPSTAN_SOCKET;
  return spawnSync(
    process.execPath,
    [path.join(root, "dist", "src", "cli.js"), ...args],
    {
      cwd,
      env,
      encoding: "utf8",
      stdio: ["pipe", "pipe", "pipe"],
    },
  );
}

test("dash outside a terminal exits 2 and points to cstan status, before it needs a credential or a daemon", () => {
  const cwd = mkdtempSync(path.join(tmpdir(), "cstan-dash-"));
  try {
    const result = cstan(["dash"], cwd);
    assert.equal(result.status, 2);
    assert.match(result.stderr, /dash needs an interactive terminal/);
    assert.match(result.stderr, /cstan status/);
    assert.equal(result.stdout, "");
  } finally {
    rmSync(cwd, { recursive: true, force: true });
  }
});

test("dash takes only its own flags and refuses a bad interval", () => {
  const cwd = mkdtempSync(path.join(tmpdir(), "cstan-dash-"));
  try {
    assert.equal(cstan(["dash", "--json"], cwd).status, 2);
    assert.equal(cstan(["dash", "extra"], cwd).status, 2);
    const bad = cstan(["dash", "--interval", "0"], cwd);
    assert.equal(bad.status, 3);
    assert.match(bad.stderr, /--interval must be an integer from 1 to 60/);
  } finally {
    rmSync(cwd, { recursive: true, force: true });
  }
});

test("peek is not a cstan command", () => {
  const cwd = mkdtempSync(path.join(tmpdir(), "cstan-dash-"));
  try {
    assert.equal(cstan(["peek", "x"], cwd).status, 2);
  } finally {
    rmSync(cwd, { recursive: true, force: true });
  }
});
