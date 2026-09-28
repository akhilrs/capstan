import assert from "node:assert/strict";
import { test } from "node:test";
import { validateWorkflowPlan } from "../src/controller/workflow.js";

function validPlan() {
  return {
    schemaVersion: 1,
    taskId: "demo-1",
    objective: "Implement a bounded sample task",
    acceptanceCriteria: ["The command prints the exact total"],
    limits: { maxSlices: 4, maxRunMs: 60_000, maxDispatches: 8 },
    slices: [
      {
        id: "parse",
        title: "Parse input",
        description: "Parse JSONL records",
        role: "Developer",
        dependsOn: [],
        writeScope: ["src/parse.ts"],
        acceptanceCriteria: ["The command prints the exact total"],
      },
      {
        id: "cli",
        title: "Add CLI",
        description: "Expose aggregation",
        role: "Developer",
        dependsOn: ["parse"],
        writeScope: ["src/cli.ts"],
        acceptanceCriteria: ["The command prints the exact total"],
      },
    ],
  };
}

test("validates a versioned two-slice plan and returns stable content identity", () => {
  const plan = validPlan();
  const first = validateWorkflowPlan(plan);
  const second = validateWorkflowPlan(plan);
  assert.deepEqual(first.order, ["parse", "cli"]);
  assert.match(first.hash, /^[a-f0-9]{64}$/);
  assert.equal(first.hash, second.hash);
});

test("rejects a dispatch limit too small for final-parent verification", () => {
  const plan = validPlan();
  plan.limits.maxDispatches = 6;
  assert.throws(() => validateWorkflowPlan(plan), /at least 7 role dispatches/);
  plan.limits.maxDispatches = 7;
  assert.equal(validateWorkflowPlan(plan).plan.limits.maxDispatches, 7);
});

test("rejects ambiguous fields, missing acceptance, scope traversal, invalid roles and cycles", () => {
  const unknownField = { ...validPlan(), accepted: true };
  assert.throws(
    () => validateWorkflowPlan(unknownField),
    /unknown or missing fields/,
  );
  const emptyCriteria = { ...validPlan(), acceptanceCriteria: [] };
  assert.throws(
    () => validateWorkflowPlan(emptyCriteria),
    /acceptanceCriteria/,
  );
  const traversal = validPlan();
  traversal.slices[0]!.writeScope[0] = "../outside";
  assert.throws(
    () => validateWorkflowPlan(traversal),
    /stay within the project/,
  );
  const wholeProject = validPlan();
  wholeProject.slices[0]!.writeScope[0] = ".";
  assert.throws(
    () => validateWorkflowPlan(wholeProject),
    /stay within the project/,
  );
  const badRole = validPlan();
  badRole.slices[1]!.role = "Verifier";
  assert.throws(() => validateWorkflowPlan(badRole), /role must be Developer/);
  const cyclic = validPlan();
  cyclic.slices[0]!.dependsOn = ["cli"];
  assert.throws(() => validateWorkflowPlan(cyclic), /acyclic/);
});

test("rejects unknown dependencies and plans that do not gate a later slice on the first", () => {
  const unknown = validPlan();
  unknown.slices[1]!.dependsOn = ["missing"];
  assert.throws(
    () => validateWorkflowPlan(unknown),
    /unknown slice dependency/,
  );
  const ungated = validPlan();
  ungated.slices[1]!.dependsOn = [];
  assert.throws(() => validateWorkflowPlan(ungated), /explicitly depend/);
});
