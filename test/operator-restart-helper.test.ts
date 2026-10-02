import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { existsSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import net from "node:net";
import path from "node:path";
import { test } from "node:test";
import { DatabaseSync } from "node:sqlite";
import { callDaemon, pingDaemon } from "../src/client.js";
import {
  directPing,
  pingFrame,
  runHelper,
  type RestartPlan,
  type RestartResult,
} from "../src/restart-helper.js";
import { distHash, knownGoodBuild } from "../src/restart.js";
import { harness, close } from "./harness.js";
import {
  CREDENTIAL,
  alive,
  fakeProject,
  planFor,
  startFakeController,
  stopFakeController,
  waitFor,
  writeFakeBuild,
  writePlan,
  type FakeMode,
  type FakeProject,
} from "./restart-fakes.js";

const quiet = (): void => undefined;

function resultOf(project: FakeProject, id = "r1"): RestartResult {
  return JSON.parse(
    readFileSync(
      path.join(project.stateDir, "restart", id, "result.json"),
      "utf8",
    ),
  ) as RestartResult;
}

/** The project with a known-good snapshot of `good`, then `dist` replaced by the `next` build. */
function projectWithBuilds(
  good: FakeMode,
  next: FakeMode,
  options: { maxMigration?: number } = {},
): FakeProject {
  const project = fakeProject();
  writeFakeBuild(project.distDir, good, {
    startsFile: project.startsFile,
    ...options,
  });
  knownGoodBuild(project.stateDir).snapshot(project.distDir, project.root);
  rmSync(project.distDir, { recursive: true });
  writeFakeBuild(project.distDir, next, {
    startsFile: project.startsFile,
    ...options,
  });
  return project;
}

test("the helper's ping frame is the frame the client sends", async () => {
  const project = fakeProject();
  const received: Buffer[] = [];
  const server = net.createServer((connection) => {
    connection.on("data", (chunk) => {
      received.push(chunk);
      connection.end(`${JSON.stringify({ ok: true, result: { pid: 1 } })}\n`);
    });
  });
  await new Promise<void>((resolve) =>
    server.listen(project.socketPath, resolve),
  );
  try {
    await callDaemon(project.socketPath, CREDENTIAL, "ping");
    assert.equal(
      Buffer.concat(received).toString("utf8"),
      pingFrame(CREDENTIAL),
    );
  } finally {
    server.close();
    await project.cleanup();
  }
});

test("the direct ping accepts a real daemon and rejects a dead one", async () => {
  const h = await harness();
  try {
    const accepted = await directPing(h.socketPath, h.owner);
    assert.equal(accepted.kind, "ok");
    assert.equal(
      accepted.kind === "ok" ? accepted.pid : null,
      process.pid,
      "the real daemon answers with its pid",
    );
    assert.equal(
      (await directPing(h.socketPath, "wrong-credential")).kind,
      "failed",
    );
    assert.equal((await pingDaemon(h.socketPath, h.owner)).outcome, "running");
    await h.server.close();
    assert.equal((await directPing(h.socketPath, h.owner)).kind, "refused");
    const missing = path.join(h.stateDirectory, "absent.sock");
    assert.equal((await directPing(missing, h.owner)).kind, "refused");
  } finally {
    await close(h);
  }
});

test("healthy path: the new controller answers a direct ping and the result is ok", async () => {
  const project = fakeProject();
  try {
    writeFakeBuild(project.distDir, "good", { startsFile: project.startsFile });
    knownGoodBuild(project.stateDir).snapshot(project.distDir, project.root);
    const code = await runHelper(writePlan(project, planFor(project)), quiet);
    assert.equal(code, 0);
    const result = resultOf(project);
    assert.equal(result.outcome, "ok");
    const ping = await pingDaemon(project.socketPath, CREDENTIAL);
    assert.equal(ping.outcome, "running");
    assert.equal(ping.outcome === "running" ? ping.pid : 0, result.pid);
    assert.equal(existsSync(`${project.distDir}.failed-r1`), false);
  } finally {
    await stopFakeController(project);
    await project.cleanup();
  }
});

for (const failing of ["exit", "silent"] as const) {
  test(`rollback: a new build that ${failing === "exit" ? "exits at once" : "never answers ping"} is replaced by the known-good dist`, async () => {
    const project = projectWithBuilds("good", failing);
    try {
      const savedHash = distHash(path.join(project.knownGoodPath, "dist"));
      const failedHash = distHash(project.distDir);
      assert.notEqual(failedHash, savedHash);
      const code = await runHelper(
        writePlan(project, planFor(project, { healthTimeoutSeconds: 1 })),
        quiet,
      );
      assert.equal(code, 1);
      const result = resultOf(project);
      assert.equal(result.outcome, "rolled_back");
      assert.match(
        result.reason ?? "",
        failing === "exit"
          ? /exited right after it started/
          : /did not answer ping/,
      );
      if (failing === "exit")
        assert.match(
          result.failedLogTail ?? "",
          /boom: the fake controller cannot start/,
        );
      assert.equal(result.ledgerRestored, false);
      assert.equal(
        distHash(project.distDir),
        savedHash,
        "dist is byte-identical to the snapshot",
      );
      assert.equal(
        distHash(`${project.distDir}.failed-r1`),
        failedHash,
        "the failed build is kept",
      );
      assert.equal(
        (await pingDaemon(project.socketPath, CREDENTIAL)).outcome,
        "running",
        "the old build answers ping",
      );
    } finally {
      await stopFakeController(project);
      await project.cleanup();
    }
  });
}

function createLedger(project: FakeProject, versions: number): DatabaseSync {
  const db = new DatabaseSync(project.ledgerPath);
  db.exec("PRAGMA journal_mode = WAL; PRAGMA wal_autocheckpoint = 0;");
  db.exec("CREATE TABLE schema_migrations(version INTEGER PRIMARY KEY)");
  db.exec("CREATE TABLE messages(body TEXT)");
  for (let version = 1; version <= versions; version += 1)
    db.prepare("INSERT INTO schema_migrations(version) VALUES (?)").run(
      version,
    );
  return db;
}

function ledgerRows(project: FakeProject): {
  bodies: string[];
  version: number;
} {
  const db = new DatabaseSync(project.ledgerPath, { readOnly: true });
  try {
    return {
      bodies: (
        db.prepare("SELECT body FROM messages ORDER BY rowid").all() as {
          body: string;
        }[]
      ).map((row) => row.body),
      version: (
        db.prepare("SELECT MAX(version) AS v FROM schema_migrations").get() as {
          v: number;
        }
      ).v,
    };
  } finally {
    db.close();
  }
}

test("ledger: the backup is taken by the helper after the old controller exited and keeps -wal; a row of the failed new build is lost, an old row survives, and a newer schema is restored before the old build starts", async () => {
  const project = projectWithBuilds("schema", "migrate-exit", {
    maxMigration: 3,
  });
  try {
    // The "old controller" writes a message and dies without a checkpoint: the row lives only in the -wal file.
    const writer = spawn(
      process.execPath,
      [
        "-e",
        `const { DatabaseSync } = require("node:sqlite");
         const db = new DatabaseSync(${JSON.stringify(project.ledgerPath)});
         db.exec("PRAGMA journal_mode = WAL; PRAGMA wal_autocheckpoint = 0;");
         db.exec("CREATE TABLE schema_migrations(version INTEGER PRIMARY KEY)");
         db.exec("CREATE TABLE messages(body TEXT)");
         for (const v of [1, 2, 3]) db.prepare("INSERT INTO schema_migrations(version) VALUES (?)").run(v);
         db.prepare("INSERT INTO messages(body) VALUES ('old-row')").run();
         console.log("written");
         setInterval(() => {}, 1000);`,
      ],
      { stdio: ["ignore", "pipe", "inherit"] },
    );
    await new Promise<void>((resolve) =>
      writer.stdout.once("data", () => resolve()),
    );
    writer.kill("SIGKILL");
    await new Promise((resolve) => writer.once("exit", resolve));
    assert.ok(
      existsSync(`${project.ledgerPath}-wal`),
      "the old row is only in the -wal file",
    );

    const code = await runHelper(
      writePlan(project, planFor(project, { healthTimeoutSeconds: 1 })),
      quiet,
    );
    assert.equal(code, 1);
    const result = resultOf(project);
    assert.equal(result.outcome, "rolled_back");
    assert.equal(result.ledgerRestored, true);
    assert.ok(
      existsSync(
        path.join(
          project.stateDir,
          "restart",
          "r1",
          "ledger.bak",
          "controller.sqlite-wal",
        ),
      ),
      "the backup includes the -wal file",
    );
    const rows = ledgerRows(project);
    assert.deepEqual(rows.bodies, ["old-row"]);
    assert.equal(rows.version, 3);
    assert.equal(
      (await pingDaemon(project.socketPath, CREDENTIAL)).outcome,
      "running",
    );
  } finally {
    await stopFakeController(project);
    await project.cleanup();
  }
});

test("ledger: without a newer schema the ledger is not restored", async () => {
  const project = projectWithBuilds("schema", "exit", { maxMigration: 3 });
  try {
    const db = createLedger(project, 3);
    db.prepare("INSERT INTO messages(body) VALUES ('kept')").run();
    db.close();
    await runHelper(
      writePlan(project, planFor(project, { healthTimeoutSeconds: 1 })),
      quiet,
    );
    const result = resultOf(project);
    assert.equal(result.outcome, "rolled_back");
    assert.equal(result.ledgerRestored, false);
  } finally {
    await stopFakeController(project);
    await project.cleanup();
  }
});

test("down: a total failure writes outcome down with the manual recovery path and the dependency change, and tries the restored build at most twice", async () => {
  const project = projectWithBuilds("exit", "exit");
  try {
    const code = await runHelper(
      writePlan(
        project,
        planFor(project, { depsChanged: ["package-lock.json"] }),
      ),
      quiet,
    );
    assert.equal(code, 2);
    const result = resultOf(project);
    assert.equal(result.outcome, "down");
    assert.match(result.reason ?? "", /package-lock\.json/);
    assert.deepEqual(result.depsChanged, ["package-lock.json"]);
    assert.match(result.manualRecovery ?? "", /cstan start/);
    assert.match(result.manualRecovery ?? "", /known-good/);
    const starts = readFileSync(project.startsFile, "utf8").trim().split("\n");
    assert.equal(
      starts.length,
      3,
      "one start of the new build and two of the restored one",
    );
  } finally {
    await project.cleanup();
  }
});

test("down: a known-good dist that cannot be restored leaves the failed build in place and names it", async () => {
  const project = projectWithBuilds("good", "exit");
  try {
    rmSync(path.join(project.knownGoodPath, "dist"), { recursive: true });
    const code = await runHelper(writePlan(project, planFor(project)), quiet);
    assert.equal(code, 2);
    assert.equal(resultOf(project).outcome, "down");
    assert.ok(existsSync(path.join(project.distDir, "src", "cli.js")));
    assert.equal(existsSync(`${project.distDir}.restoring-r1`), false);
  } finally {
    await project.cleanup();
  }
});

test("handoff: the helper waits for the socket and the pid file to go before it starts the new build", async () => {
  const project = fakeProject();
  try {
    writeFakeBuild(project.distDir, "good", { startsFile: project.startsFile });
    knownGoodBuild(project.stateDir).snapshot(project.distDir, project.root);
    const old = await startFakeController(project);
    const oldPid = old.pid;
    setTimeout(() => void stopFakeController(project), 700);
    const code = await runHelper(
      writePlan(
        project,
        planFor(project, {
          pid: oldPid,
          timing: {
            handoffWaitMs: 5000,
            termWaitMs: 400,
            killWaitMs: 2000,
            pollMs: 100,
          },
        }),
      ),
      quiet,
    );
    assert.equal(code, 0);
    assert.equal(alive(oldPid), false);
    const starts = readFileSync(project.startsFile, "utf8").trim().split("\n");
    assert.equal(
      starts.length,
      2,
      "the old controller and exactly one new start by the helper",
    );
    assert.equal(resultOf(project).pid === oldPid, false);
  } finally {
    await stopFakeController(project);
    await project.cleanup();
  }
});

test("handoff: a stuck old pid gets SIGTERM, then SIGKILL, and only its own stale pid file is removed", async () => {
  const project = fakeProject();
  try {
    writeFakeBuild(project.distDir, "stubborn", {
      startsFile: project.startsFile,
    });
    const old = await startFakeController(project);
    writeFakeBuild(project.distDir, "good", { startsFile: project.startsFile });
    knownGoodBuild(project.stateDir).snapshot(project.distDir, project.root);
    assert.ok(existsSync(project.pidPath));
    const code = await runHelper(
      writePlan(
        project,
        planFor(project, {
          pid: old.pid,
          timing: {
            handoffWaitMs: 300,
            termWaitMs: 400,
            killWaitMs: 2000,
            pollMs: 100,
          },
        }),
      ),
      quiet,
    );
    assert.equal(code, 0, JSON.stringify(resultOf(project)));
    assert.equal(
      alive(old.pid),
      false,
      "the stubborn old controller was killed",
    );
    assert.equal(resultOf(project).outcome, "ok");
  } finally {
    await stopFakeController(project);
    await project.cleanup();
  }
});

test("handoff: the pid file of another live process is never removed and no new controller is started", async () => {
  const project = fakeProject();
  const other = spawn(process.execPath, ["-e", "setInterval(() => {}, 1000)"], {
    stdio: "ignore",
  });
  try {
    writeFakeBuild(project.distDir, "good", { startsFile: project.startsFile });
    knownGoodBuild(project.stateDir).snapshot(project.distDir, project.root);
    writeFileSync(project.pidPath, `${other.pid}\n`);
    const code = await runHelper(
      writePlan(
        project,
        planFor(project, {
          timing: {
            handoffWaitMs: 300,
            termWaitMs: 200,
            killWaitMs: 500,
            pollMs: 100,
          },
        }),
      ),
      quiet,
    );
    assert.equal(code, 2);
    const result = resultOf(project);
    assert.equal(result.outcome, "down");
    assert.match(result.reason ?? "", /NOT running/);
    assert.equal(
      readFileSync(project.pidPath, "utf8").trim(),
      String(other.pid),
    );
    assert.equal(
      existsSync(project.startsFile),
      false,
      "no controller was started",
    );
    assert.equal(alive(other.pid!), true, "the other process was left alone");
  } finally {
    other.kill("SIGKILL");
    await project.cleanup();
  }
});

test("the helper is a standalone file: it imports only node builtins", () => {
  const source = readFileSync(
    path.resolve(import.meta.dirname, "..", "src", "restart-helper.js"),
    "utf8",
  );
  const specifiers = [
    ...source.matchAll(/from\s+"([^"]+)"|import\("([^"]+)"\)/g),
  ].map((match) => match[1] ?? match[2]!);
  assert.ok(specifiers.length > 0);
  for (const specifier of specifiers)
    assert.match(specifier, /^node:/, `unexpected import ${specifier}`);
});

test("a handoff failure after the old controller was killed but its socket is held writes down, not rolled_back", async () => {
  const project = fakeProject();
  const holder = spawn(
    process.execPath,
    [
      "-e",
      `require("net").createServer(c => c.end("x\\n")).listen(${JSON.stringify(project.socketPath)}); setInterval(() => {}, 1000)`,
    ],
    { stdio: "ignore" },
  );
  try {
    writeFakeBuild(project.distDir, "good");
    knownGoodBuild(project.stateDir).snapshot(project.distDir, project.root);
    await waitFor(() => existsSync(project.socketPath));
    const code = await runHelper(
      writePlan(
        project,
        planFor(project, {
          timing: {
            handoffWaitMs: 300,
            termWaitMs: 200,
            killWaitMs: 500,
            pollMs: 100,
          },
        }),
      ),
      quiet,
    );
    assert.equal(code, 2);
    const result = resultOf(project);
    assert.equal(result.outcome, "down");
    assert.match(result.reason ?? "", /NOT running/);
    assert.match(result.manualRecovery ?? "", /cstan start/);
  } finally {
    holder.kill("SIGKILL");
    await project.cleanup();
  }
});

test("an unreadable key file or a corrupt plan still writes a down result", async () => {
  const project = fakeProject();
  try {
    rmSync(project.keyPath);
    const code = await runHelper(writePlan(project, planFor(project)), quiet);
    assert.equal(code, 2);
    const unreadable = resultOf(project);
    assert.equal(unreadable.outcome, "down");
    assert.match(unreadable.reason ?? "", /plan or the operator key/);
    assert.match(unreadable.manualRecovery ?? "", /cstan start/);

    const planPath = writePlan(project, planFor(project, { id: "r2" }));
    writeFileSync(planPath, "{ not json");
    assert.equal(await runHelper(planPath, quiet), 2);
    assert.equal(resultOf(project, "r2").outcome, "down");
  } finally {
    await project.cleanup();
  }
});

async function handoffFailureWithPingHolder(
  answeredPid: number,
  withoutDepsChanged = false,
): Promise<RestartResult> {
  const project = fakeProject();
  const holder = spawn(
    process.execPath,
    [
      "-e",
      `require("net").createServer(c => c.end(JSON.stringify({ ok: true, result: { pid: ${answeredPid} } }) + "\\n")).listen(${JSON.stringify(project.socketPath)}); setInterval(() => {}, 1000)`,
    ],
    { stdio: "ignore" },
  );
  try {
    writeFakeBuild(project.distDir, "good");
    knownGoodBuild(project.stateDir).snapshot(project.distDir, project.root);
    await waitFor(() => existsSync(project.socketPath));
    const plan: Record<string, unknown> = {
      ...planFor(project, {
        timing: {
          handoffWaitMs: 300,
          termWaitMs: 200,
          killWaitMs: 500,
          pollMs: 100,
        },
      }),
    };
    if (withoutDepsChanged) delete plan.depsChanged;
    await runHelper(writePlan(project, plan as unknown as RestartPlan), quiet);
    return resultOf(project);
  } finally {
    holder.kill("SIGKILL");
    await project.cleanup();
  }
}

test("a handoff failure while the recorded old pid still answers ping is rolled_back", async () => {
  const result = await handoffFailureWithPingHolder(2_000_000_000);
  assert.equal(result.outcome, "rolled_back");
  assert.match(result.reason ?? "", /old controller \(pid \d+\) still answers/);
});

test("a handoff failure while a different process answers ping is down and says so", async () => {
  const result = await handoffFailureWithPingHolder(12345);
  assert.equal(result.outcome, "down");
  assert.match(result.reason ?? "", /other than the old one/);
  assert.match(result.manualRecovery ?? "", /cstan start/);
});

test("a plan without depsChanged still writes a down result", async () => {
  const result = await handoffFailureWithPingHolder(12345, true);
  assert.equal(result.outcome, "down");
  assert.match(result.reason ?? "", /failed unexpectedly/);
});
