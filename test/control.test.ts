import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import { randomUUID } from "node:crypto";
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { test } from "node:test";
import { listenControl, requestControl } from "../src/control.js";
import { ControllerCore } from "../src/controller/core.js";

test("authenticated operator control pauses dispatch without containing work and resumes only after reconciliation", async () => {
  const directory = mkdtempSync(path.join(tmpdir(), "cstan-control-"));
  const cli = path.resolve("dist/src/cli.js");
  const initialized = spawnSync(process.execPath, [cli, "init"], {
    cwd: directory,
    encoding: "utf8",
  });
  assert.equal(initialized.status, 0, initialized.stderr);
  const config = JSON.parse(
    readFileSync(path.join(directory, ".capstan/project.json"), "utf8"),
  ) as { projectId: string; name: string; stateDirectory: string };
  const credential = readFileSync(
    path.join(directory, ".capstan/operator.key"),
    "utf8",
  ).trim();
  const project = {
    projectId: config.projectId,
    name: config.name,
    ownerCredential: credential,
    initialInputs: [
      { kind: "project_config" as const, content: { name: config.name } },
      { kind: "task_brief" as const, content: { objective: "deliver" } },
      { kind: "acceptance_criteria" as const, content: ["deliver"] },
      { kind: "policy" as const, content: { maxRunMs: 60_000 } },
      { kind: "plan" as const, content: { slices: [] } },
    ],
  };
  let core: ControllerCore | undefined;
  let close: (() => Promise<void>) | undefined;
  try {
    core = await ControllerCore.open({
      stateDirectory: config.stateDirectory,
      project,
      workspaceRoot: directory,
    });
    const controller = core;
    const mutate = () => {
      const requestId = randomUUID();
      return {
        credential,
        requestId,
        idempotencyKey: requestId,
        expectedVersion: controller.stateVersion,
        inputRevision: controller.inputRevision,
      };
    };
    let reconciled = 0;
    close = await listenControl(
      path.join(config.stateDirectory, "control.sock"),
      credential,
      controller,
      async (action) => {
        if (action === "pause")
          return controller.transitionRun(mutate(), "paused");
        if (action === "resume") {
          reconciled++;
          return controller.transitionRun(mutate(), "active");
        }
        controller.transitionRun(mutate(), "canceling");
        throw new Error("cancellation containment incomplete");
      },
    );
    await assert.rejects(
      requestControl(
        path.join(config.stateDirectory, "control.sock"),
        "invalid",
        "pause",
      ),
      /unauthorized/,
    );
    const pause = await new Promise<{ status: number | null; stderr: string }>(
      (resolve, reject) => {
        const child = spawn(process.execPath, [cli, "pause"], {
          cwd: directory,
        });
        let stderr = "";
        child.stderr
          .setEncoding("utf8")
          .on("data", (chunk: string) => (stderr += chunk));
        child.once("error", reject);
        child.once("close", (status) => resolve({ status, stderr }));
      },
    );
    assert.equal(pause.status, 0, pause.stderr);
    assert.equal(controller.statusSnapshot().run.state, "paused");
    assert.equal(reconciled, 0);
    const resume = await new Promise<{ status: number | null; stderr: string }>(
      (resolve, reject) => {
        const child = spawn(process.execPath, [cli, "resume"], {
          cwd: directory,
        });
        let stderr = "";
        child.stderr
          .setEncoding("utf8")
          .on("data", (chunk: string) => (stderr += chunk));
        child.once("error", reject);
        child.once("close", (status) => resolve({ status, stderr }));
      },
    );
    assert.equal(resume.status, 0, resume.stderr);
    assert.equal(reconciled, 1);
    assert.equal(controller.statusSnapshot().run.state, "active");
    const canceled = await new Promise<{
      status: number | null;
      stderr: string;
    }>((resolve, reject) => {
      const child = spawn(process.execPath, [cli, "cancel"], {
        cwd: directory,
      });
      let stderr = "";
      child.stderr
        .setEncoding("utf8")
        .on("data", (chunk: string) => (stderr += chunk));
      child.once("error", reject);
      child.once("close", (status) => resolve({ status, stderr }));
    });
    assert.equal(canceled.status, 5);
    assert.match(canceled.stderr, /containment incomplete/);
    assert.equal(controller.statusSnapshot().run.state, "canceling");
  } finally {
    await close?.();
    core?.close();
    rmSync(directory, { recursive: true, force: true });
  }
});
