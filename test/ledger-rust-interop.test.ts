/**
 * Interop between the Node ledger (src/controller/database.ts, ownership.ts) and capstan-ledger (rust/crates/ledger),
 * through the dev-only binary `ledger-probe`.
 *
 * The suite needs ledger-probe: LEDGER_PROBE_BIN, else ${CARGO_TARGET_DIR:-rust/target}/release/ledger-probe. It is built by
 * `npm run check:dash` (which `npm run check` runs before `npm test`) or by
 * `cargo build --release -p capstan-ledger` in rust/. A bare `npm test` without one of those FAILS here (it never skips).
 */
import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import {
  copyFileSync,
  existsSync,
  mkdirSync,
  readdirSync,
  readFileSync,
  symlinkSync,
  chmodSync,
  writeFileSync,
} from "node:fs";
import path from "node:path";
import { test } from "node:test";
import { openDatabase } from "../src/controller/database.js";
import { ControllerCore } from "../src/controller/core.js";
import {
  ProjectLock,
  ProjectLockHeldError,
} from "../src/controller/ownership.js";
import { openSqlite } from "../src/controller/sqlite.js";
import { project } from "./controller-harness.js";
import {
  PARITY_DIRECTORY,
  REFUSAL_SETUPS,
  buildNodeLedger,
  exportFixtures,
  migrationFiles,
} from "./ledger-parity-export.js";
import { removeTempDir, tempDir } from "./tmp.js";

const root = path.resolve(import.meta.dirname, "..", "..");
const NODE_CLI = path.join(root, "dist", "src", "cli.js");
const REGENERATE =
  "run `npm run build && node dist/test/ledger-parity-export.js` and commit rust/crates/ledger/tests/parity";
const BACKUP = /^controller\.sqlite\.pre-v(\d+)-(\d+)\.sqlite$/;

/** The probe binary the tests run: LEDGER_PROBE_BIN, else the host release build of the Rust workspace. */
function probeBinary(): string {
  const configured = process.env.LEDGER_PROBE_BIN;
  const file =
    configured !== undefined && configured !== ""
      ? configured
      : path.join(
          path.resolve(root, "rust", process.env.CARGO_TARGET_DIR ?? "target"),
          "release",
          "ledger-probe",
        );
  assert.ok(
    existsSync(file),
    `ledger-probe is missing (${file}): build it with \`npm run check:dash\` or \`cargo build --release -p capstan-ledger\` in rust/, or set LEDGER_PROBE_BIN; a bare \`npm test\` needs it built first`,
  );
  return file;
}

interface Probe {
  ok: boolean;
  kind?: string;
  error?: string;
  [key: string]: unknown;
}

function probe(command: string, file: string): Probe {
  const result = spawnSync(probeBinary(), [command, file], {
    encoding: "utf8",
    timeout: 60_000,
  });
  assert.ok(
    result.status === 0 || result.status === 1,
    `ledger-probe ${command}: status ${String(result.status)} ${result.stderr}`,
  );
  return JSON.parse(result.stdout) as Probe;
}

interface Dump {
  master: unknown[];
  migrations: unknown[];
  userVersion: unknown;
  journalMode: unknown;
}

function dump(file: string): Dump {
  const db = openSqlite(file, { readOnly: true });
  try {
    return {
      master: db
        .prepare(
          "SELECT type, name, tbl_name, sql FROM sqlite_master ORDER BY type, name",
        )
        .all(),
      migrations: db
        .prepare(
          "SELECT version, name, checksum FROM schema_migrations ORDER BY version",
        )
        .all(),
      userVersion: db.pragma("user_version"),
      journalMode: db.pragma("journal_mode"),
    };
  } finally {
    db.close();
  }
}

function backupsIn(directory: string): Array<[number, number]> {
  return readdirSync(directory)
    .flatMap((name) => {
      const m = BACKUP.exec(name);
      return m === null
        ? []
        : [[Number(m[1]), Number(m[2])] as [number, number]];
    })
    .sort((a, b) => b[0] - a[0] || b[1] - a[1]);
}

function scratch(): string {
  return tempDir("capstan-ledger-interop-");
}

test("ledger-probe is built", () => {
  probeBinary();
});

test("the committed ledger parity fixtures equal a fresh export", async () => {
  const fresh = await exportFixtures();
  for (const [name, text] of fresh) {
    const file = path.join(PARITY_DIRECTORY, name);
    assert.ok(existsSync(file), `${name} is missing; ${REGENERATE}`);
    assert.ok(
      readFileSync(file, "utf8") === text,
      `${name} is stale; ${REGENERATE}`,
    );
  }
  const extra = readdirSync(PARITY_DIRECTORY).filter(
    (name) => !fresh.has(name),
  );
  assert.deepEqual(extra, [], `unexpected fixtures; ${REGENERATE}`);
});

/** The same ledger migrated by Node and by Rust must come out identical. */
async function compareMigration(start: (file: string) => void, label: string) {
  const nodeDir = scratch();
  const rustDir = scratch();
  const nodeFile = path.join(nodeDir, "controller.sqlite");
  const rustFile = path.join(rustDir, "controller.sqlite");
  try {
    start(nodeFile);
    if (existsSync(nodeFile)) copyFileSync(nodeFile, rustFile);
    (await openDatabase(nodeFile)).close();
    const result = probe("open", rustFile);
    assert.equal(result.ok, true, `${label}: ${result.error}`);
    assert.deepEqual(dump(rustFile), dump(nodeFile), label);
    const rustBackups = backupsIn(rustDir);
    const nodeBackups = backupsIn(nodeDir);
    assert.deepEqual(
      rustBackups.map(([version]) => version),
      nodeBackups.map(([version]) => version),
      `${label}: kept backups`,
    );
    for (const [, at] of rustBackups)
      assert.ok(at > 1_700_000_000_000 && at <= Date.now(), `${label}: ${at}`);
    return rustBackups.length;
  } finally {
    removeTempDir(nodeDir);
    removeTempDir(rustDir);
  }
}

const LATEST = migrationFiles().length;

for (let version = 0; version <= LATEST; version++) {
  test(`Node ledger at v${version} migrated by Rust equals Node's result`, async () => {
    const kept = await compareMigration(
      (file) => buildNodeLedger(file, version),
      `v${version}`,
    );
    // One backup before every step that has a ledger before it (not the first), and the newest three are kept.
    assert.equal(
      kept,
      Math.min(3, version === 0 ? LATEST - 1 : LATEST - version),
    );
  });
}

test("the better-sqlite3 fixture migrates in Rust to Node's schema", async () => {
  await compareMigration(
    (file) =>
      copyFileSync(
        path.join(root, "test", "fixtures", "ledger-better-sqlite3.sqlite"),
        file,
      ),
    "better-sqlite3 fixture",
  );
});

test("a ledger created by Rust opens in Node with no migration and no backup", async () => {
  const directory = scratch();
  try {
    const file = path.join(directory, "controller.sqlite");
    assert.equal(probe("open", file).ok, true);
    const before = dump(file);
    const backupsBefore = backupsIn(directory);
    const applied = (): unknown => {
      const db = openSqlite(file, { readOnly: true });
      try {
        return db.prepare("SELECT * FROM schema_migrations").all();
      } finally {
        db.close();
      }
    };
    const rows = applied();
    (await openDatabase(file)).close();
    assert.deepEqual(dump(file), before);
    assert.deepEqual(applied(), rows, "Node applied nothing");
    assert.deepEqual(backupsIn(directory), backupsBefore, "no new backup");
    for (const row of rows as Array<{ applied_at: string }>)
      assert.match(row.applied_at, /^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d\.\d{3}Z$/);

    // ControllerCore opens the state directory a Rust-made ledger sits in.
    const state = scratch();
    copyFileSync(file, path.join(state, "controller.sqlite"));
    const stateBackups = backupsIn(state);
    const core = await ControllerCore.open({
      stateDirectory: state,
      project: project(),
    });
    core.close();
    assert.deepEqual(backupsIn(state), stateBackups);
  } finally {
    removeTempDir(directory);
  }
});

for (const setup of REFUSAL_SETUPS.filter((s) => s.invalidUtf8 !== true)) {
  test(`Rust refuses a ${setup.name} ledger with Node's text`, () => {
    const directory = scratch();
    try {
      const file = path.join(directory, "controller.sqlite");
      setup.prepare(file);
      const fixture = (
        JSON.parse(
          readFileSync(path.join(PARITY_DIRECTORY, "refusals.json"), "utf8"),
        ) as Array<{ case: string; message: string }>
      ).find((c) => c.case === setup.name);
      const result = probe("open", file);
      assert.equal(result.ok, false);
      assert.equal(result.kind, "migration");
      assert.equal(result.error, fixture?.message);
    } finally {
      removeTempDir(directory);
    }
  });
}

test("open-ro never creates a missing file", () => {
  const directory = scratch();
  try {
    const file = path.join(directory, "controller.sqlite");
    assert.equal(probe("open-ro", file).ok, false);
    assert.equal(existsSync(file), false);
    buildNodeLedger(file, 2);
    assert.equal(probe("open-ro", file).ok, true);
  } finally {
    removeTempDir(directory);
  }
});

test("integers and BLOBs written by Rust read back in Node; Rust refuses what Node cannot read", () => {
  const directory = scratch();
  try {
    const file = path.join(directory, "controller.sqlite");
    const result = probe("write-values", file);
    assert.equal(result.ok, true);
    assert.equal(result.unsafe_rejected, true);
    const db = openSqlite(file);
    try {
      const row = db.prepare("SELECT n, b FROM probe_values").get() as {
        n: number;
        b: Uint8Array;
      };
      assert.equal(row.n, Number.MAX_SAFE_INTEGER);
      assert.deepEqual([...row.b], [0, 1, 255]);
      // The Node adapter throws on an integer beyond 2^53 (documented in sqlite.ts), which is why Rust never writes one.
      db.exec("UPDATE probe_values SET n = 9007199254740993");
      assert.throws(() => db.prepare("SELECT n FROM probe_values").get());
    } finally {
      db.close();
    }
  } finally {
    removeTempDir(directory);
  }
});

function holdInRust(file: string): Promise<{
  release: () => Promise<void>;
}> {
  const child = spawn(probeBinary(), ["lock-hold", file], {
    stdio: ["pipe", "pipe", "inherit"],
  });
  let output = "";
  return new Promise((resolve, reject) => {
    child.once("error", reject);
    child.stdout.on("data", (chunk: Buffer) => {
      output += chunk.toString();
      if (output.includes('"held":true')) {
        resolve({
          release: () =>
            new Promise<void>((done) => {
              child.once("close", () => done());
              child.stdin.end("\n");
            }),
        });
      }
    });
    child.once("close", () => reject(new Error(`lock-hold ended: ${output}`)));
  });
}

test("lock interop: Node holds, Rust is refused, and release is seen", () => {
  const directory = scratch();
  try {
    const lockFile = path.join(directory, "controller.lock");
    const lock = ProjectLock.acquire(lockFile);
    const refused = probe("lock-try", lockFile);
    assert.equal(refused.ok, false);
    assert.equal(refused.kind, "held");
    assert.equal(refused.error, new ProjectLockHeldError().message);
    lock.close();
    assert.equal(probe("lock-try", lockFile).ok, true);
  } finally {
    removeTempDir(directory);
  }
});

test("lock interop: Rust holds, Node is refused, and release is seen", async () => {
  const directory = scratch();
  try {
    const lockFile = path.join(directory, "controller.lock");
    const held = await holdInRust(lockFile);
    assert.throws(() => ProjectLock.acquire(lockFile), ProjectLockHeldError);
    await held.release();
    ProjectLock.acquire(lockFile).close();
  } finally {
    removeTempDir(directory);
  }
});

test("lock refusals match: symlink, foreign mode and a file that is not a lock database", () => {
  const directory = scratch();
  try {
    const target = path.join(directory, "elsewhere");
    writeFileSync(target, "");
    const link = path.join(directory, "link.lock");
    symlinkSync(target, link);
    const open = path.join(directory, "open.lock");
    writeFileSync(open, "");
    chmodSync(open, 0o644);
    const junk = path.join(directory, "junk.lock");
    writeFileSync(junk, "x".repeat(200), { mode: 0o600 });
    for (const file of [link, open, junk]) {
      let nodeMessage = "";
      try {
        ProjectLock.acquire(file).close();
      } catch (error) {
        nodeMessage = (error as Error).message;
      }
      assert.notEqual(nodeMessage, "", file);
      const result = probe("lock-try", file);
      assert.equal(result.ok, false);
      assert.equal(result.error, nodeMessage, path.basename(file));
    }
  } finally {
    removeTempDir(directory);
  }
});

test("lock interop: a running Node scratch daemon makes Rust refuse", () => {
  const directory = scratch();
  const env = { ...process.env, CAPSTAN_LAUNCH: "off" } as NodeJS.ProcessEnv;
  for (const name of [
    "CAPSTAN_TOKEN",
    "CAPSTAN_SOCKET",
    "CAPSTAN_AGENT_ID",
    "CSTAN_NODE_CLI",
    "CSTAN_NODE",
    "CSTAN_FRONT_END",
  ])
    delete env[name];
  const node = (...argv: string[]): void => {
    const result = spawnSync(process.execPath, [NODE_CLI, ...argv], {
      cwd: directory,
      encoding: "utf8",
      env,
      timeout: 60_000,
    });
    assert.equal(result.status, 0, `cstan ${argv.join(" ")}: ${result.stderr}`);
  };
  try {
    const git = (...args: string[]): void => {
      const result = spawnSync("git", ["-C", directory, ...args], {
        encoding: "utf8",
      });
      assert.equal(result.status, 0, result.stderr);
    };
    git("init", "--quiet");
    git(
      "-c",
      "user.name=t",
      "-c",
      "user.email=t@example.com",
      "commit",
      "--quiet",
      "--allow-empty",
      "-m",
      "chore: initial commit",
    );
    mkdirSync(directory, { recursive: true });
    node("init");
    node("start");
    const lockFile = path.join(
      directory,
      ".capstan",
      "state",
      "controller.lock",
    );
    assert.ok(existsSync(lockFile), "the daemon created the lock file");
    const result = probe("lock-try", lockFile);
    assert.equal(result.ok, false);
    assert.equal(result.kind, "held");
    node("stop");
    assert.equal(probe("lock-try", lockFile).ok, true);
  } finally {
    spawnSync(process.execPath, [NODE_CLI, "stop"], {
      cwd: directory,
      env,
      timeout: 30_000,
    });
    removeTempDir(directory);
  }
});
