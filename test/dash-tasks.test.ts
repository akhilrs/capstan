import assert from "node:assert/strict";
import { test } from "node:test";
import { cellWidth, taskSummary } from "../src/dash/format.js";
import { plainLines } from "../src/dash/lines.js";
import { buildDashModel, taskIndexOf } from "../src/dash/model.js";
import { makeTheme } from "../src/dash/theme.js";
import { buildFrame } from "../src/dash/view.js";
import { NOW, healthy, showcase, tasksShowcase } from "./dash-fixtures.js";
import { modelOf, viewOf } from "./dash-view-helpers.js";

const theme = makeTheme({ noColor: true, reducedMotion: true });
const taskOf = (status: Record<string, unknown>, agentId: string) =>
  modelOf(status).agents.find((a) => a.agentId === agentId)!.task;
const entry = (id: string, label = id) => ({ id, label });

test("the summary reads idle with no tasks and nothing when there is no data", () => {
  assert.equal(taskSummary([], 60), "idle");
  assert.equal(taskSummary(null, 60), "");
});

test("one task reads 'working on:' with its label, cut with an ellipsis when it does not fit", () => {
  const one = [entry("plan-19", "plan-19 Dashboard cleanup (1/2)")];
  assert.equal(
    taskSummary(one, 80),
    "working on: plan-19 Dashboard cleanup (1/2)",
  );
  assert.equal(taskSummary(one, 30), "working on: plan-19 Dashboard…");
});

test("several tasks that fit are listed with a middle dot", () => {
  const tasks = [
    entry("plan-19", "plan-19 Cleanup (1/2)"),
    entry("PM-120", "PM-120 Poller"),
  ];
  assert.equal(
    taskSummary(tasks, 100),
    "working on: plan-19 Cleanup (1/2) · PM-120 Poller",
  );
});

test("tasks that do not fit fall back to a count, as many names as fit and +k", () => {
  const tasks = ["plan-19", "plan-20", "PM-120", "PM-121"].map((id) =>
    entry(id, `${id} some longer title (0/3)`),
  );
  for (const [width, text] of [
    [60, "4 tasks: plan-19, plan-20, PM-120, PM-121"],
    [37, "4 tasks: plan-19, plan-20, PM-120, +1"],
    [32, "4 tasks: plan-19, plan-20, +2"],
    [22, "4 tasks: plan-19, +3"],
  ] as const) {
    const got = taskSummary(tasks, width);
    assert.equal(got, text, `width ${width}`);
    assert.ok(cellWidth(got) <= width);
  }
  const cut = taskSummary(tasks, 14);
  assert.equal(cut, "4 tasks: plan…");
  assert.equal(cellWidth(cut), 14);
});

test("the summary never exceeds its width", () => {
  const tasks = ["plan-19", "PM-120", "x".repeat(40)].map((id) =>
    entry(id, `${id} title`),
  );
  for (let width = 1; width <= 90; width++)
    assert.ok(cellWidth(taskSummary(tasks, width)) <= width, `width ${width}`);
});

test("the model derives the header entries and every agent's TASK from activeTasks", () => {
  const model = modelOf(tasksShowcase());
  assert.deepEqual(model.header.tasks, [
    { id: "plan-19", label: "plan-19 Dashboard cleanup (1/2)" },
    { id: "PM-120", label: "PM-120 Fix the poller" },
  ]);
  const task = (id: string) => model.agents.find((a) => a.agentId === id)!.task;
  assert.equal(task("developer-1"), "plan-19/dash-ui PM-114 Dash UI");
  assert.equal(task("architect-1"), "plan-19 · plan");
  assert.equal(task("developer-2"), "PM-120 Fix the poller");
  assert.equal(task("reviewer-1"), "review developer-2 r1");
  assert.equal(task("designer-1"), "-");
  assert.equal(task("pm-1"), "-");
});

test("an integration review names the integration as its subject, and only a started review counts", () => {
  const status = tasksShowcase({
    reviews: [
      {
        reviewerAgentId: "reviewer-1",
        integrationId: "7c1e0f2a99",
        round: 2,
        state: "started",
      },
      {
        reviewerAgentId: "designer-1",
        authorAgentId: "developer-1",
        state: "passed",
      },
    ],
  });
  assert.equal(taskOf(status, "reviewer-1"), "review 7c1e0f2a r2");
  assert.equal(taskOf(status, "designer-1"), "-");
});

test("a task change is part of the row fingerprint", () => {
  const a = modelOf(
    tasksShowcase({
      activeTasks: { plans: [], requirements: [], truncated: false },
    }),
  ).agents.find((r) => r.agentId === "developer-2")!;
  const b = modelOf(
    tasksShowcase({
      activeTasks: { plans: [], requirements: [], truncated: false },
      panes: [
        {
          agentId: "developer-2",
          paneId: "p4",
          taskRef: "req-9",
          taskTitle: "Other",
        },
      ],
    }),
  ).agents.find((r) => r.agentId === "developer-2")!;
  assert.notEqual(a.fingerprint, b.fingerprint);
});

test("activeTasks with no plans and no requirements is idle", () => {
  const model = modelOf(
    healthy({ activeTasks: { plans: [], requirements: [], truncated: false } }),
  );
  assert.deepEqual(model.header.tasks, []);
});

test("an older daemon without activeTasks takes the header from plans and the TASK from the pane fields", () => {
  const status = healthy({
    plans: [
      {
        planId: "plan-3",
        title: "Old plan",
        state: "approved",
        cancelled: false,
        architectAgentId: "pm-agent",
        packages: { reviewed: 1, assigned: 2, cancelled: 4 },
        signoffs: [],
      },
      {
        planId: "plan-2",
        title: "Done",
        state: "approved",
        cancelled: false,
        architectAgentId: null,
        packages: { integrated: 1 },
        signoffs: [{ integrationId: "i" }],
      },
      {
        planId: "plan-1",
        title: "Gone",
        state: "draft",
        cancelled: true,
        architectAgentId: null,
        packages: {},
        signoffs: [],
      },
    ],
    panes: [
      {
        agentId: "developer-agent",
        paneId: "p2",
        taskRef: "req-1",
        taskTitle: "Fix it",
      },
    ],
  });
  const model = modelOf(status);
  assert.deepEqual(model.header.tasks, [
    { id: "plan-3", label: "plan-3 Old plan (1/3)" },
  ]);
  assert.equal(taskOf(status, "pm-agent"), "plan-3 · plan");
  assert.equal(taskOf(status, "developer-agent"), "req-1 Fix it");
});

test("a status with neither activeTasks nor plans nor pane task fields hides the header line and shows '-'", () => {
  const model = modelOf(showcase());
  assert.equal(model.header.tasks, null);
  assert.ok(model.agents.every((a) => a.task === "-"));
  const lines = plainLines(buildFrame(model, viewOf(120, 36), theme).lines);
  assert.ok(!lines.some((l) => /working on:|idle\s*│$/.test(l)));
});

test("malformed task data falls back instead of throwing", () => {
  for (const activeTasks of [
    null,
    7,
    "x",
    [],
    { plans: 3, requirements: "x" },
    {
      plans: [null, 1, { planId: 5, packages: "x" }],
      requirements: [{ agentIds: "x" }, { refId: "r", agentIds: [null, 4] }],
    },
  ]) {
    const model = buildDashModel({ ...tasksShowcase(), activeTasks }, NOW, 4);
    assert.equal(model.agents.length, 6);
  }
  assert.deepEqual(
    taskIndexOf({
      activeTasks: { plans: [{ planId: "p", done: "x", total: null }] },
    }).entries,
    [{ id: "p", label: "p" }],
  );
});

test("labels from plan and spawn titles lose their control characters", () => {
  const status = tasksShowcase({
    panes: [
      {
        agentId: "developer-2",
        paneId: "p4",
        taskRef: "req-7",
        taskTitle: "bad\u001b[31m\u0007title",
      },
    ],
  });
  const model = modelOf(status);
  const row = model.agents.find((a) => a.agentId === "developer-2")!;
  assert.doesNotMatch(row.task, /[\u0000-\u001f\u007f]/);
  const plan = tasksShowcase();
  (plan.activeTasks as { plans: { title: string }[] }).plans[0]!.title =
    "x\u001b]0;pwn\u0007y";
  const labels = modelOf(plan)
    .header.tasks!.map((t) => t.label)
    .join();
  assert.doesNotMatch(labels, /[\u0000-\u001f\u007f]/);
});

test("the header line shows in the frame at 80 and 160 columns, and fits", () => {
  for (const [columns, rows] of [
    [80, 24],
    [160, 45],
  ] as const) {
    const lines = plainLines(
      buildFrame(modelOf(tasksShowcase()), viewOf(columns, rows), theme).lines,
    );
    assert.equal(lines.length, rows);
    assert.match(
      lines.slice(0, 5).join("\n"),
      /working on: plan-19 Dashboard cleanup \(1\/2\)/,
    );
    for (const line of lines) assert.equal(Array.from(line).length, columns);
  }
  const idle = plainLines(
    buildFrame(
      modelOf(
        healthy({
          activeTasks: { plans: [], requirements: [], truncated: false },
        }),
      ),
      viewOf(80, 24),
      theme,
    ).lines,
  );
  assert.ok(idle.slice(0, 5).some((l) => /^│ idle +│$/.test(l)));
});

test("at 80 columns TASK is at least 10 cells wide or absent, and the other columns keep their widths", () => {
  const withTask = plainLines(
    buildFrame(modelOf(tasksShowcase()), viewOf(80, 24), theme).lines,
  );
  const header = withTask.find((l) => l.includes("AGENT"))!;
  const at = header.indexOf("TASK");
  assert.ok(at > 0);
  const after = header.slice(at).search(/ROLE/);
  assert.ok(after - 1 >= 10, `TASK is ${after - 1} wide`);
  const plain = plainLines(
    buildFrame(modelOf(showcase()), viewOf(80, 24), theme).lines,
  );
  for (const word of ["GEN", "STATE", "AGE", "Q"] as const) {
    const w = (lines: string[]) =>
      lines
        .find((l) => l.includes("AGENT"))!
        .match(new RegExp(`${word}\\s+`))![0].length;
    assert.equal(w(withTask), w(plain), word);
  }
});

test("TASK is the first column to go when the panel narrows", () => {
  for (const columns of [60, 64, 70]) {
    const lines = plainLines(
      buildFrame(modelOf(tasksShowcase()), viewOf(columns, 24), theme).lines,
    );
    const header = lines.find((l) => l.includes("AGENT"))!;
    assert.match(header, /AGENT/);
    assert.match(header, /GEN/);
    assert.match(header, /STATE/);
    if (header.includes("TASK")) assert.match(header, /ROLE/);
  }
});

test("long task values end with an ellipsis", () => {
  const status = tasksShowcase({
    panes: [
      {
        agentId: "developer-2",
        paneId: "p4",
        taskRef: "req-7",
        taskTitle: "t".repeat(80),
      },
    ],
    activeTasks: { plans: [], requirements: [], truncated: false },
  });
  const lines = plainLines(
    buildFrame(modelOf(status), viewOf(160, 45), theme).lines,
  );
  assert.ok(lines.some((l) => /req-7 t+…/.test(l)));
});

test("the Architect of several plans shows +k, and a pane's package ref resolves without an assignee entry", () => {
  const status = tasksShowcase({
    activeTasks: {
      plans: [
        {
          planId: "plan-19",
          title: "A",
          architectAgentId: "architect-1",
          done: 0,
          total: 1,
          packages: [
            {
              packageId: "dash-ui",
              title: "Dash UI",
              nexoraId: "PM-114",
              assigneeAgentId: null,
            },
          ],
        },
        {
          planId: "plan-20",
          title: "B",
          architectAgentId: "architect-1",
          done: 0,
          total: 0,
          packages: [],
        },
      ],
      requirements: [],
      truncated: false,
    },
  });
  assert.equal(taskOf(status, "architect-1"), "plan-19 · plan +1");
  assert.equal(taskOf(status, "developer-1"), "plan-19/dash-ui PM-114 Dash UI");
});

test("without activeTasks the header is hidden when no plan is open", () => {
  const status = healthy({
    plans: [
      {
        planId: "plan-1",
        title: "Gone",
        state: "draft",
        cancelled: true,
        packages: {},
        signoffs: [],
      },
    ],
  });
  assert.equal(modelOf(status).header.tasks, null);
  assert.equal(modelOf(healthy({ plans: [] })).header.tasks, null);
});
