import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { test } from "node:test";
import {
  ControllerUnavailableError,
  daemonCommand,
  DaemonSelectionError,
  ensureDaemon,
  selectDaemon,
} from "../src/client.js";
import { loadCapstanConfig } from "../src/config/capstan-config.js";
import { ConfigError } from "../src/config/types.js";
import { RUST_DAEMON_SKIPS, RUST_DAEMON_SUITES } from "./rust-daemon-suites.js";
import { spawnSync } from "node:child_process";

const BASE = `schema_version = 1

[hosts.claude]
kind = "claude"

[roles.pm]
kind = "PM"
host = "claude"

[nexora]
track = "never"
`;

function project(extra: string): string {
  const directory = fs.realpathSync(
    fs.mkdtempSync(path.join(os.tmpdir(), "capstan-select-")),
  );
  fs.writeFileSync(path.join(directory, "capstan.toml"), `${BASE}${extra}`, {
    mode: 0o600,
  });
  return directory;
}

/** An executable that records how it was started and exits. */
function fakeBinary(directory: string): string {
  const file = path.join(directory, "fake-daemon");
  fs.writeFileSync(
    file,
    `#!/bin/sh\nprintf '%s|%s|%s\\n' "$#" "$PWD" "$CAPSTAN_TOKEN" > "${directory}/started"\nexit 3\n`,
    { mode: 0o700 },
  );
  return file;
}

test("an absent setting starts the Node daemon exactly as before", () => {
  const directory = project("");
  try {
    assert.equal(selectDaemon({}, directory), "node");
    const plain = daemonCommand({ KEEP: "1" }, "/x/cli.js", false, directory);
    const legacy = daemonCommand({ KEEP: "1" }, "/x/cli.js");
    assert.deepEqual(plain, legacy);
    assert.equal(plain.command, process.execPath);
    assert.deepEqual(plain.args.slice(-2), ["/x/cli.js", "daemon"]);
    assert.equal(loadCapstanConfig(directory).daemon, undefined);
  } finally {
    fs.rmSync(directory, { recursive: true, force: true });
  }
});

test("capstan.toml implementation = rust and CSTAN_DAEMON=rust select cstan-daemon from CSTAN_DAEMON_BIN", () => {
  const directory = project('\n[daemon]\nimplementation = "rust"\n');
  try {
    const binary = fakeBinary(directory);
    const fromFile = daemonCommand(
      { CSTAN_DAEMON_BIN: binary, CAPSTAN_TOKEN: "x", KEEP: "1" },
      "/x/cli.js",
      false,
      directory,
    );
    assert.equal(fromFile.command, binary);
    assert.deepEqual(fromFile.args, []);
    assert.equal(fromFile.env.CAPSTAN_TOKEN, undefined);
    assert.equal(fromFile.env.KEEP, "1");
    // The Rust daemon is not a CLI: its agents' cstan runs the Node CLI that started it.
    assert.equal(fromFile.env.CSTAN_NODE_CLI, "/x/cli.js");
    assert.equal(fromFile.env.CSTAN_NODE, process.execPath);
    const bare = project("");
    try {
      const fromEnv = daemonCommand(
        { CSTAN_DAEMON: "rust", CSTAN_DAEMON_BIN: binary },
        "/x/cli.js",
        false,
        bare,
      );
      assert.equal(fromEnv.command, binary);
    } finally {
      fs.rmSync(bare, { recursive: true, force: true });
    }
  } finally {
    fs.rmSync(directory, { recursive: true, force: true });
  }
});

test("CSTAN_DAEMON=node overrides the file, and an explicit node in the file is the Node daemon", () => {
  const rust = project('\n[daemon]\nimplementation = "rust"\n');
  const node = project('\n[daemon]\nimplementation = "node"\n');
  try {
    assert.equal(selectDaemon({ CSTAN_DAEMON: "node" }, rust), "node");
    assert.equal(selectDaemon({}, node), "node");
    assert.equal(selectDaemon({ CSTAN_DAEMON: "" }, rust), "rust");
    assert.equal(
      daemonCommand({ CSTAN_DAEMON: "node" }, "/x/cli.js", false, rust).command,
      process.execPath,
    );
  } finally {
    fs.rmSync(rust, { recursive: true, force: true });
    fs.rmSync(node, { recursive: true, force: true });
  }
});

test("a bad value is a config error from the file, the loader and the environment", () => {
  const directory = project('\n[daemon]\nimplementation = "go"\n');
  try {
    assert.throws(
      () => loadCapstanConfig(directory),
      (error: unknown) =>
        error instanceof ConfigError &&
        error.message === "daemon.implementation must be one of node, rust",
    );
    assert.throws(
      () => selectDaemon({}, directory),
      (error: unknown) =>
        error instanceof DaemonSelectionError &&
        error.message === "daemon.implementation must be one of node, rust",
    );
    assert.throws(
      () => selectDaemon({ CSTAN_DAEMON: "go" }, directory),
      (error: unknown) =>
        error instanceof DaemonSelectionError &&
        error.message === "CSTAN_DAEMON must be one of node, rust",
    );
  } finally {
    fs.rmSync(directory, { recursive: true, force: true });
  }
});

test("rust with no binary fails the start with start_failed and never falls back to Node", async () => {
  const directory = project('\n[daemon]\nimplementation = "rust"\n');
  try {
    for (const env of [
      {},
      { CSTAN_DAEMON_BIN: path.join(directory, "missing") },
      { CSTAN_DAEMON_BIN: "relative/cstan-daemon" },
    ]) {
      await assert.rejects(
        ensureDaemon({
          socketPath: path.join(directory, "control.sock"),
          credential: "c".repeat(40),
          projectRoot: directory,
          logPath: path.join(directory, "daemon.log"),
          cliPath: "/nonexistent/dist/src/cli.js",
          env,
          timeoutMs: 3000,
        }),
        (error: unknown) =>
          error instanceof ControllerUnavailableError &&
          error.reason === "start_failed" &&
          /Rust daemon could not be started/.test(error.message),
      );
    }
  } finally {
    fs.rmSync(directory, { recursive: true, force: true });
  }
});

test("ensureDaemon spawns the selected binary in the project directory with no arguments", async () => {
  const directory = project('\n[daemon]\nimplementation = "rust"\n');
  try {
    const binary = fakeBinary(directory);
    await assert.rejects(
      ensureDaemon({
        socketPath: path.join(directory, "control.sock"),
        credential: "c".repeat(40),
        projectRoot: directory,
        logPath: path.join(directory, "daemon.log"),
        cliPath: "/nonexistent/dist/src/cli.js",
        env: { CSTAN_DAEMON_BIN: binary, CAPSTAN_TOKEN: "secret" },
        timeoutMs: 5000,
      }),
      /exited during startup \(exit code 3\)/,
    );
    assert.equal(
      fs.readFileSync(path.join(directory, "started"), "utf8"),
      `0|${directory}|\n`,
    );
  } finally {
    fs.rmSync(directory, { recursive: true, force: true });
  }
});

const repositoryRoot = path.resolve(import.meta.dirname, "..", "..");

test("the Rust daemon gate: every listed suite exists, and check runs it after check:dash and before the tests", () => {
  const manifest = JSON.parse(
    fs.readFileSync(path.join(repositoryRoot, "package.json"), "utf8"),
  ) as { scripts: Record<string, string> };
  for (const suite of RUST_DAEMON_SUITES)
    assert.ok(
      fs.existsSync(
        path.join(
          repositoryRoot,
          "test",
          `${suite.replace(/\.test$/, "")}.test.ts`,
        ),
      ),
      `${suite} has no test file`,
    );
  for (const [suite, reason] of Object.entries(RUST_DAEMON_SKIPS)) {
    assert.ok(
      RUST_DAEMON_SUITES.includes(suite),
      `${suite} is skipped but not listed`,
    );
    assert.ok(reason.length > 0);
  }
  assert.equal(
    manifest.scripts["check:rust-daemon"],
    "node scripts/check-rust-daemon.mjs",
  );
  const steps = manifest.scripts.check?.split(" && ") ?? [];
  assert.ok(
    steps.indexOf("npm run check:rust-daemon") ===
      steps.indexOf("npm run check:dash") + 1 &&
      steps.indexOf("npm test") ===
        steps.indexOf("npm run check:rust-daemon") + 1,
    manifest.scripts.check,
  );
});

test("check:rust-daemon without a binary fails, and skips loudly when asked", () => {
  const script = path.join(repositoryRoot, "scripts", "check-rust-daemon.mjs");
  const missing = path.join(os.tmpdir(), "capstan-no-such-daemon");
  const failing = spawnSync(process.execPath, [script], {
    env: { ...process.env, CSTAN_DAEMON_BIN: missing },
    encoding: "utf8",
  });
  assert.equal(failing.status, 1);
  assert.match(
    failing.stderr,
    /is not an executable; run npm run check:dash first/,
  );
  const skipped = spawnSync(process.execPath, [script], {
    env: {
      ...process.env,
      CSTAN_DAEMON_BIN: missing,
      CSTAN_SKIP_RUST_DAEMON_CHECK: "1",
    },
    encoding: "utf8",
  });
  assert.equal(skipped.status, 0);
  assert.match(skipped.stderr, /check:rust-daemon SKIPPED/);
});

test("a bad selection exits like any other configuration error, and says why", () => {
  const cli = path.join(repositoryRoot, "dist", "src", "cli.js");
  const directory = project("");
  try {
    const environment: NodeJS.ProcessEnv = {
      ...process.env,
      CAPSTAN_LAUNCH: "off",
      CSTAN_DAEMON: "go",
    };
    delete environment.CAPSTAN_TOKEN;
    delete environment.CAPSTAN_SOCKET;
    const init = spawnSync(process.execPath, [cli, "init"], {
      cwd: directory,
      env: environment,
      encoding: "utf8",
    });
    assert.equal(init.status, 0, init.stderr);
    const bad = spawnSync(process.execPath, [cli, "ping"], {
      cwd: directory,
      env: environment,
      encoding: "utf8",
    });
    assert.equal(bad.status, 3, bad.stderr);
    assert.match(bad.stderr, /CSTAN_DAEMON must be one of node, rust/);
    fs.writeFileSync(
      path.join(directory, "capstan.toml"),
      `${BASE}\n[daemon]\nimplementation = "go"\n`,
      { mode: 0o600 },
    );
    delete environment.CSTAN_DAEMON;
    const check = spawnSync(process.execPath, [cli, "config", "check"], {
      cwd: directory,
      env: environment,
      encoding: "utf8",
    });
    assert.equal(check.status, 3, check.stderr);
    assert.match(
      check.stderr,
      /daemon.implementation must be one of node, rust/,
    );
    const fromFile = spawnSync(process.execPath, [cli, "ping"], {
      cwd: directory,
      env: environment,
      encoding: "utf8",
    });
    assert.equal(fromFile.status, 3, fromFile.stderr);
  } finally {
    fs.rmSync(directory, { recursive: true, force: true });
  }
});

test("only a bad daemon.implementation is a selection error; other config problems are left to the daemon", () => {
  const unrelated = project(
    '\n[roles.daemon]\nkind = "Nobody"\nhost = "claude"\n',
  );
  const unknownKey = project('\n[daemon]\nbinary = "x"\n');
  try {
    assert.equal(selectDaemon({}, unrelated), "node");
    assert.equal(selectDaemon({}, unknownKey), "node");
  } finally {
    fs.rmSync(unrelated, { recursive: true, force: true });
    fs.rmSync(unknownKey, { recursive: true, force: true });
  }
});
