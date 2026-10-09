import assert from "node:assert/strict";
import { symlinkSync } from "node:fs";
import path from "node:path";
import { test } from "node:test";
import {
  AgentPaneMismatch,
  HerdrAdapter,
  InvalidArgumentError,
  PaneGone,
  PaneLost,
  PhaseError,
  UnknownPaneError,
  UnsupportedHostError,
} from "../src/herdr/adapter.js";
import { HerdrError } from "../src/herdr/runner.js";
import {
  fixture,
  SHELL_READY,
  type Harness,
  harness,
  startedWorker,
} from "./herdr-adapter-harness.js";

test("the adapter reads the Herdr version and creates worktrees and workspaces as registered fresh panes", async () => {
  const h = harness();
  try {
    assert.equal(await h.adapter.version(), "herdr 0.9.1");
    const worktree = await h.adapter.createWorktree({
      workspaceId: "w9",
      branch: "cap/task/dev-g1",
      label: "dev",
      base: "main",
    });
    assert.deepEqual(h.fake.callsTo("worktree", "create")[0], [
      "worktree",
      "create",
      "--workspace",
      "w9",
      "--branch",
      "cap/task/dev-g1",
      "--label",
      "dev",
      "--base",
      "main",
      "--no-focus",
    ]);
    assert.equal(worktree.path, h.fake.panes.get(worktree.paneId)!.checkout);
    assert.deepEqual(h.adapter.paneEntry(worktree.paneId), {
      role: "worker",
      phase: "fresh",
      kind: "shell",
      worktreePath: worktree.path,
      workspaceId: worktree.workspaceId,
    });
    const pm = await h.adapter.createWorkspace({
      cwd: h.fake.root,
      label: "pm",
      role: "PM",
    });
    assert.equal(h.adapter.paneEntry(pm.paneId)!.role, "PM");
    assert.deepEqual(await h.adapter.paneState(pm.paneId), {
      status: "unknown",
    });
    for (const bad of [
      { workspaceId: "x9", branch: "b", label: "l" },
      { workspaceId: "w9", branch: "../evil", label: "l" },
      { workspaceId: "w9", branch: "b", label: "bad\nlabel" },
      { workspaceId: "w9", branch: "b", label: "l", base: "a b" },
    ])
      await assert.rejects(h.adapter.createWorktree(bad), InvalidArgumentError);
    await assert.rejects(
      h.adapter.createWorkspace({ cwd: "relative", label: "x", role: "PM" }),
      InvalidArgumentError,
    );
    await h.adapter.removeWorktree(worktree.workspaceId, { force: true });
    assert.equal(h.adapter.paneEntry(worktree.paneId), undefined);
    assert.deepEqual(h.fake.callsTo("worktree", "remove").at(-1), [
      "worktree",
      "remove",
      "--workspace",
      worktree.workspaceId,
      "--force",
    ]);
    await h.adapter.closePane(pm.paneId);
    assert.equal(h.adapter.paneEntry(pm.paneId), undefined);
  } finally {
    h.adapter.close();
    h.fake.cleanup();
  }
});
function secondAdapter(h: Harness): HerdrAdapter {
  return new HerdrAdapter({
    run: h.fake.run,
    tempRoot: h.fake.root,
    sleep: async () => {},
    now: () => 0,
  });
}
test("a second adapter adopts a pane the first one started, after Herdr confirms it, and can then deliver to it", async () => {
  const h = harness();
  try {
    const worker = await startedWorker(h);
    const entry = h.adapter.paneEntry(worker.paneId)!;
    const next = secondAdapter(h);
    assert.equal(next.paneForAgent("dev"), undefined);
    await next.adoptPane({
      paneId: worker.paneId,
      role: "worker",
      agent: "dev",
      workspaceId: entry.workspaceId ?? null,
      worktreePath: entry.worktreePath ?? null,
    });
    assert.equal(next.paneForAgent("dev"), worker.paneId);
    assert.deepEqual(next.paneEntry(worker.paneId), {
      role: "worker",
      phase: "started",
      kind: "claude",
      agent: "dev",
      workspaceId: entry.workspaceId,
      worktreePath: entry.worktreePath,
    });
    assert.deepEqual(
      await next.guardedSend({
        paneId: worker.paneId,
        text: "after the restart",
        beforeSend: () => {},
      }),
      { sent: true },
    );
    await assert.rejects(
      next.adoptPane({
        paneId: worker.paneId,
        role: "worker",
        agent: "dev",
        workspaceId: null,
        worktreePath: null,
      }),
      PhaseError,
    );
  } finally {
    h.adapter.close();
    h.fake.cleanup();
  }
});
test("adoption registers nothing when the pane is gone, the name points elsewhere, or the name is unusable", async () => {
  const h = harness();
  try {
    const worker = await startedWorker(h);
    const next = secondAdapter(h);
    await assert.rejects(
      next.adoptPane({
        paneId: "w77:p1",
        role: "worker",
        agent: "dev",
        workspaceId: null,
        worktreePath: null,
      }),
      PaneGone,
    );
    h.fake.agentStates.set("dev", { paneId: "w78:p1", statuses: ["idle"] });
    await assert.rejects(
      next.adoptPane({
        paneId: worker.paneId,
        role: "worker",
        agent: "dev",
        workspaceId: null,
        worktreePath: null,
      }),
      AgentPaneMismatch,
    );
    await assert.rejects(
      next.adoptPane({
        paneId: worker.paneId,
        role: "worker",
        agent: "a:b",
        workspaceId: null,
        worktreePath: null,
      }),
      InvalidArgumentError,
    );
    await assert.rejects(
      next.adoptPane({
        paneId: "bad",
        role: "worker",
        agent: "dev",
        workspaceId: null,
        worktreePath: null,
      }),
      InvalidArgumentError,
    );
    assert.equal(next.paneEntry(worker.paneId), undefined);
    assert.equal(next.paneForAgent("dev"), undefined);
  } finally {
    h.adapter.close();
    h.fake.cleanup();
  }
});
test("a fallback shell pane is adopted for display only and accepts no agent input", async () => {
  const h = harness();
  try {
    const watch = h.fake.add("watch", SHELL_READY);
    const next = secondAdapter(h);
    await next.adoptShellPane(watch.paneId, watch.workspaceId);
    assert.equal(next.paneEntry(watch.paneId)!.kind, "shell");
    await assert.rejects(
      next.guardedSend({
        paneId: watch.paneId,
        text: "x",
        beforeSend: () => {},
      }),
      PhaseError,
    );
    await assert.rejects(next.adoptShellPane("w88:p1", null), PaneGone);
  } finally {
    h.adapter.close();
    h.fake.cleanup();
  }
});
async function worktreePane(h: Harness) {
  const created = await h.adapter.createWorktree({
    workspaceId: "w1",
    branch: "capstan/x-1",
    label: "x-1",
  });
  return created;
}
test("paneLayout reads the tab, the zoom flag and every pane's size, and refuses a bad id", async () => {
  const h = harness();
  try {
    h.fake.layoutRects = [
      { pane_id: "w9:p1", rect: { width: 120, height: 40 } },
      { pane_id: "w9:p2", rect: { width: 60.5, height: "40" } },
    ];
    const view = await h.adapter.paneLayout("w9:p1");
    assert.equal(view.tabId, "w9:t1");
    assert.equal(view.workspaceId, "w9");
    assert.equal(view.zoomed, false);
    assert.deepEqual(view.panes[0], {
      paneId: "w9:p1",
      width: 120,
      height: 40,
    });
    assert.equal(
      Number.isSafeInteger(view.panes[1]!.width),
      false,
      "a float is passed on as such and counts as no fit later",
    );
    assert.ok(
      Number.isNaN(view.panes[1]!.height),
      "a size sent as text is not a number",
    );
    assert.deepEqual(h.fake.callsTo("pane", "layout")[0], [
      "pane",
      "layout",
      "--pane",
      "w9:p1",
    ]);
    await assert.rejects(h.adapter.paneLayout("bad id"), InvalidArgumentError);
  } finally {
    h.adapter.close();
    h.fake.cleanup();
  }
});
test("placePane moves a worker pane, follows its new id in the registry and checks its inputs first", async () => {
  const h = harness();
  try {
    const tree = await worktreePane(h);
    const placed = await h.adapter.placePane({
      paneId: tree.paneId,
      tabId: "w9:t1",
      targetPaneId: "w9:p1",
      direction: "right",
      keep: 0.6,
      worktreePath: tree.path,
    });
    assert.equal(placed.workspaceId, "w9");
    assert.notEqual(placed.paneId, tree.paneId);
    assert.deepEqual(h.fake.callsTo("pane", "move")[0], [
      "pane",
      "move",
      tree.paneId,
      "--tab",
      "w9:t1",
      "--split",
      "right",
      "--target-pane",
      "w9:p1",
      "--ratio",
      "0.6",
      "--no-focus",
    ]);
    assert.equal(h.adapter.paneEntry(tree.paneId), undefined);
    const entry = h.adapter.paneEntry(placed.paneId)!;
    assert.equal(entry.role, "worker");
    assert.equal(entry.workspaceId, "w9");
    assert.equal(entry.worktreePath, tree.path);
    const before = h.fake.callsTo("pane", "move").length;
    for (const bad of [
      { tabId: "w9", direction: "right" as const },
      { tabId: "w9:t1; rm", direction: "right" as const },
      { tabId: "w9:t1", direction: "left" as unknown as "right" },
    ])
      await assert.rejects(
        h.adapter.placePane({
          paneId: placed.paneId,
          targetPaneId: "w9:p1",
          keep: 0.6,
          worktreePath: tree.path,
          ...bad,
        }),
        InvalidArgumentError,
      );
    await assert.rejects(
      h.adapter.placePane({
        paneId: "w5:p1",
        tabId: "w9:t1",
        targetPaneId: "w9:p1",
        direction: "down",
        keep: 0.6,
        worktreePath: tree.path,
      }),
      UnknownPaneError,
    );
    assert.equal(
      h.fake.callsTo("pane", "move").length,
      before,
      "nothing reached Herdr",
    );
  } finally {
    h.adapter.close();
    h.fake.cleanup();
  }
});
test("placePane refuses a pane that is not a worker", async () => {
  const h = harness();
  try {
    const pm = await h.adapter.createWorkspace({
      cwd: h.fake.root,
      label: "pm",
      role: "PM",
    });
    await assert.rejects(
      h.adapter.placePane({
        paneId: pm.paneId,
        tabId: "w9:t1",
        targetPaneId: "w9:p1",
        direction: "right",
        keep: 0.6,
        worktreePath: h.fake.root,
      }),
      PhaseError,
    );
  } finally {
    h.adapter.close();
    h.fake.cleanup();
  }
});
test("a move that errors is resolved by looking again: nothing moved, moved, lost or ambiguous", async () => {
  for (const [mode, twin, expect] of [
    ["error-no-move", false, "rethrow"],
    ["error-moved", false, "moved"],
    ["error-lost", false, "lost"],
    ["error-moved", true, "lost"],
    ["error-moved", false, "list-fails"],
  ] as const) {
    const h = harness();
    try {
      const tree = await worktreePane(h);
      h.fake.moveMode = mode;
      h.fake.extraPaneAtMovedPath = twin;
      if (expect === "list-fails") h.fake.failListCall = 2;
      const attempt = h.adapter.placePane({
        paneId: tree.paneId,
        tabId: "w9:t1",
        targetPaneId: "w9:p1",
        direction: "down",
        keep: 0.6,
        worktreePath: tree.path,
      });
      if (expect === "rethrow") {
        await assert.rejects(attempt, HerdrError);
        assert.notEqual(
          h.adapter.paneEntry(tree.paneId),
          undefined,
          "the registry is unchanged",
        );
      } else if (expect === "moved") {
        const placed = await attempt;
        assert.equal(placed.workspaceId, "w9");
        assert.equal(h.adapter.paneEntry(tree.paneId), undefined);
        assert.notEqual(h.adapter.paneEntry(placed.paneId), undefined);
      } else
        await assert.rejects(attempt, PaneLost, `${mode} ${twin} ${expect}`);
    } finally {
      h.adapter.close();
      h.fake.cleanup();
    }
  }
});
test("panesAtPath compares real paths, and removeWorktree never touches the PM's workspace", async () => {
  const h = harness();
  try {
    const tree = await worktreePane(h);
    const link = path.join(h.fake.root, "link");
    symlinkSync(tree.path, link);
    h.fake.panes.get(tree.paneId)!.checkout = link + "/";
    assert.deepEqual(await h.adapter.panesAtPath(tree.path), [
      { paneId: tree.paneId, workspaceId: tree.workspaceId },
    ]);
    assert.deepEqual(
      await h.adapter.panesAtPath(path.join(h.fake.root, "elsewhere")),
      [],
    );
    const pm = await h.adapter.createWorkspace({
      cwd: h.fake.root,
      label: "pm",
      role: "PM",
    });
    await assert.rejects(h.adapter.removeWorktree(pm.workspaceId), PhaseError);
    await h.adapter.removeWorktree(tree.workspaceId);
    assert.equal(h.adapter.paneEntry(tree.paneId), undefined);
    assert.notEqual(h.adapter.paneEntry(pm.paneId), undefined);
  } finally {
    h.adapter.close();
    h.fake.cleanup();
  }
});
test("an old pane that Herdr lists without a usable directory still counts as existing, so a refused move stays a tab", async () => {
  const h = harness();
  try {
    const tree = await worktreePane(h);
    h.fake.moveMode = "error-no-move";
    h.fake.panes.get(tree.paneId)!.checkout = "relative";
    await assert.rejects(
      h.adapter.placePane({
        paneId: tree.paneId,
        tabId: "w9:t1",
        targetPaneId: "w9:p1",
        direction: "right",
        keep: 0.6,
        worktreePath: tree.path,
      }),
      HerdrError,
      "not PaneLost",
    );
  } finally {
    h.adapter.close();
    h.fake.cleanup();
  }
});
test("panes the adapter cannot read are skipped, so they neither break a placement check nor match an empty or root path", async () => {
  const h = harness();
  try {
    const tree = await worktreePane(h);
    h.fake.extraListEntries = [
      {
        pane_id: "not an id",
        tab_id: "w1:t1",
        workspace_id: "w1",
        cwd: tree.path,
      },
      { pane_id: "w4:p1", workspace_id: "w4", cwd: tree.path },
      { pane_id: "w4:p2", tab_id: "w4:t1", workspace_id: "w4", cwd: "" },
      { pane_id: "w4:p3", tab_id: "w4:t1", workspace_id: "w4", cwd: "   " },
      { pane_id: "w4:p4", tab_id: "w4:t1", workspace_id: "w4", cwd: 7 },
      "junk",
      null,
    ];
    assert.deepEqual(
      (await h.adapter.panesAtPath(tree.path)).map((p) => p.paneId),
      [tree.paneId],
    );
    h.fake.extraListEntries.push({
      pane_id: "w5:p1",
      tab_id: "w5:t1",
      workspace_id: "w5",
      cwd: ".",
    });
    assert.deepEqual(await h.adapter.panesAtPath(""), []);
    assert.deepEqual(await h.adapter.panesAtPath("."), []);
    assert.deepEqual(await h.adapter.panesAtPath("relative/dir"), []);
    await assert.rejects(
      h.adapter.placePane({
        paneId: tree.paneId,
        tabId: "w9:t1",
        targetPaneId: "w9:p1",
        direction: "right",
        keep: 0.6,
        worktreePath: "relative",
      }),
      InvalidArgumentError,
    );
    assert.deepEqual(await h.adapter.panesAtPath("/"), []);
    h.fake.moveMode = "error-no-move";
    await assert.rejects(
      h.adapter.placePane({
        paneId: tree.paneId,
        tabId: "w9:t1",
        targetPaneId: "w9:p1",
        direction: "right",
        keep: 0.6,
        worktreePath: tree.path,
      }),
      HerdrError,
    );
  } finally {
    h.adapter.close();
    h.fake.cleanup();
  }
});
test("a layout without an explicit zoomed=false counts as zoomed, and a removed worktree path still matches exactly", async () => {
  const h = harness();
  try {
    h.fake.zoomed = undefined as unknown as boolean;
    h.fake.layoutRects = [
      { pane_id: "w9:p1", rect: { width: 120, height: 40 } },
    ];
    assert.equal((await h.adapter.paneLayout("w9:p1")).zoomed, true);
    h.fake.zoomed = false;
    assert.equal((await h.adapter.paneLayout("w9:p1")).zoomed, false);
    const tree = await worktreePane(h);
    h.fake.panes.get(tree.paneId)!.checkout = "/nonexistent/removed/worktree/";
    assert.deepEqual(
      (await h.adapter.panesAtPath("/nonexistent/removed/worktree")).map(
        (p) => p.paneId,
      ),
      [tree.paneId],
    );
  } finally {
    h.adapter.close();
    h.fake.cleanup();
  }
});
test("adoption takes the host kind from what Herdr reports for the agent, and refuses an agent this adapter does not drive", async () => {
  for (const kind of ["codex", "omp"] as const) {
    const h = harness();
    try {
      const worker = await startedWorker(
        h,
        fixture(`${kind}-idle-empty.ansi`),
        ["idle"],
        kind,
      );
      const entry = h.adapter.paneEntry(worker.paneId)!;
      const next = secondAdapter(h);
      await next.adoptPane({
        paneId: worker.paneId,
        role: "worker",
        agent: "dev",
        workspaceId: entry.workspaceId ?? null,
        worktreePath: entry.worktreePath ?? null,
      });
      assert.equal(next.paneEntry(worker.paneId)!.kind, kind);
      assert.equal(await next.readInput(worker.paneId), "", kind);
    } finally {
      h.adapter.close();
      h.fake.cleanup();
    }
  }
  const h = harness();
  try {
    const worker = await startedWorker(h);
    h.fake.agentStates.get("dev")!.kind = "gemini";
    const next = secondAdapter(h);
    await assert.rejects(
      next.adoptPane({
        paneId: worker.paneId,
        role: "worker",
        agent: "dev",
        workspaceId: null,
        worktreePath: null,
      }),
      UnsupportedHostError,
    );
    assert.equal(next.paneEntry(worker.paneId), undefined);
  } finally {
    h.adapter.close();
    h.fake.cleanup();
  }
});
test("adoption renames an agent that still has its bare name, and refuses one that sits on another pane", async () => {
  const h = harness();
  try {
    const worker = await startedWorker(h);
    const entry = h.adapter.paneEntry(worker.paneId)!;
    const next = new HerdrAdapter({
      run: h.fake.run,
      tempRoot: h.fake.root,
      sleep: async () => {},
      now: () => 0,
      projectSlug: "acme",
    });
    await next.adoptPane({
      paneId: worker.paneId,
      role: "worker",
      agent: "dev",
      workspaceId: entry.workspaceId ?? null,
      worktreePath: entry.worktreePath ?? null,
    });
    assert.deepEqual(h.fake.callsTo("agent", "rename")[0], [
      "agent",
      "rename",
      worker.paneId,
      "acme-dev",
    ]);
    assert.ok(h.fake.agentStates.has("acme-dev"));
    assert.ok(!h.fake.agentStates.has("dev"));
    assert.equal(next.paneForAgent("dev"), worker.paneId);

    const other = harness();
    try {
      const w = await startedWorker(other);
      other.fake.agentStates.get("dev")!.paneId = "w99:p1";
      const again = new HerdrAdapter({
        run: other.fake.run,
        tempRoot: other.fake.root,
        sleep: async () => {},
        now: () => 0,
        projectSlug: "acme",
      });
      await assert.rejects(
        again.adoptPane({
          paneId: w.paneId,
          role: "worker",
          agent: "dev",
          workspaceId: null,
          worktreePath: null,
        }),
        AgentPaneMismatch,
      );
      assert.equal(other.fake.callsTo("agent", "rename").length, 0);
    } finally {
      other.adapter.close();
      other.fake.cleanup();
    }
  } finally {
    h.adapter.close();
    h.fake.cleanup();
  }
});
test("metadata and workspace renames go to Herdr with the capstan source and checked text", async () => {
  const h = harness("acme");
  try {
    await h.adapter.reportMetadata(
      { paneId: "w9:p1" },
      { project: "Acme Shop", role: "developer", agent: "developer-1" },
    );
    assert.deepEqual(h.fake.callsTo("pane", "report-metadata")[0], [
      "pane",
      "report-metadata",
      "w9:p1",
      "--source",
      "capstan",
      "--token",
      "project=Acme Shop",
      "--token",
      "role=developer",
      "--token",
      "agent=developer-1",
    ]);
    await h.adapter.reportMetadata({ workspaceId: "w9" }, { project: "x" });
    assert.equal(h.fake.callsTo("workspace", "report-metadata").length, 1);
    await h.adapter.reportMetadata(
      { paneId: "w9:p1" },
      { project: "😀".repeat(60) },
    );
    const cut = h.fake.callsTo("pane", "report-metadata").at(-1)!.at(-1)!;
    assert.equal(
      cut,
      `project=${"😀".repeat(60)}`,
      "a value within 64 code points goes whole",
    );
    await h.adapter.renameWorkspace("w9", "Acme Shop · watch");
    assert.deepEqual(h.fake.callsTo("workspace", "rename")[0], [
      "workspace",
      "rename",
      "w9",
      "Acme Shop · watch",
    ]);
    await assert.rejects(
      h.adapter.reportMetadata({ paneId: "w9:p1" }, { Bad: "x" }),
      InvalidArgumentError,
    );
    await assert.rejects(
      h.adapter.reportMetadata({ paneId: "w9:p1" }, { project: "a\nb" }),
      InvalidArgumentError,
    );
    await assert.rejects(
      h.adapter.renameWorkspace("w9", "x".repeat(65)),
      InvalidArgumentError,
    );
  } finally {
    h.adapter.close();
    h.fake.cleanup();
  }
});
test("a tab is renamed with checked text, and a new workspace reports its first tab", async () => {
  const h = harness();
  try {
    await h.adapter.renameTab("w9:t1", "pm");
    assert.deepEqual(h.fake.callsTo("tab", "rename")[0], [
      "tab",
      "rename",
      "w9:t1",
      "pm",
    ]);
    await assert.rejects(h.adapter.renameTab("w9", "pm"), InvalidArgumentError);
    await assert.rejects(
      h.adapter.renameTab("w9:t1", "bad\nname"),
      InvalidArgumentError,
    );
  } finally {
    h.adapter.close();
    h.fake.cleanup();
  }
});

test("paneIdentity reads a pane's terminal id and its agent and project tokens, and a pane Herdr no longer has is undefined", async () => {
  const h = harness();
  try {
    const pane = h.fake.add("worker", SHELL_READY);
    pane.terminalId = "term_65d5f7a45f83b5a";
    pane.tokens = { agent: "developer-3", project: "capstan", role: "dev" };
    assert.deepEqual(await h.adapter.paneIdentity(pane.paneId), {
      terminalId: "term_65d5f7a45f83b5a",
      agent: "developer-3",
      project: "capstan",
    });
    const bare = h.fake.add("worker", SHELL_READY);
    bare.tokens = { agent: 7 };
    assert.deepEqual(await h.adapter.paneIdentity(bare.paneId), {
      terminalId: undefined,
      agent: undefined,
      project: undefined,
    });
    assert.equal(await h.adapter.paneIdentity("w99:p9"), undefined);
    await assert.rejects(
      h.adapter.paneIdentity("not a pane"),
      InvalidArgumentError,
    );
  } finally {
    h.fake.cleanup();
  }
});
