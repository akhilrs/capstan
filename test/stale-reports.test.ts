import assert from "node:assert/strict";
import { test } from "node:test";
import type { CommitInspection } from "../src/git.js";
import {
  call,
  close,
  ctx,
  harness,
  type Harness,
  type Member,
} from "./harness.js";

const BASE = "a".repeat(40);
const OLD_COMMIT = "b".repeat(40);
const NEW_TIP = "c".repeat(40);

function sameRole(h: Harness, name: string): Member {
  const seatId = `${name}-seat`;
  h.core.createSeat(ctx(h.core, h.owner), { seatId, name, role: "Developer" });
  const actor = h.core.createActor(ctx(h.core, h.owner), {
    displayName: name,
    role: "Developer",
    seatId,
  });
  h.core.registerAgent(ctx(h.core, h.owner), {
    agentId: name,
    roleName: "developer",
    seatId,
    actorId: actor.actorId,
  });
  return {
    agentId: name,
    credential: actor.credential,
    actorId: actor.actorId,
  };
}

function pane(h: Harness, agentId: string, branch: string): void {
  h.core.recordAgentPane(ctx(h.core, h.owner), {
    agentId,
    workspaceId: null,
    paneId: null,
    worktreePath: null,
    branch,
    baseSha: BASE,
  });
}

/** What the controller's git check says about a commit for the branch it is asked about. */
function inspect(tips: Record<string, string>, lies: Set<string>) {
  return async (input: {
    readonly branch: string;
    readonly baseSha: string | null;
    readonly sha: string;
  }): Promise<CommitInspection> => ({
    commitExists: true,
    branchTip: tips[input.branch] ?? null,
    isAncestorOfTip: lies.has(`${input.branch}:${input.sha}`),
    isAncestorOfBase: false,
  });
}

test("after a replacement the predecessor's token is refused for every command, and the replacement cannot report a commit that lies only on the predecessor's branch", async () => {
  const lies = new Set<string>();
  const tips: Record<string, string> = {};
  const h = await harness({ commands: { inspectCommit: inspect(tips, lies) } });
  try {
    const predecessorBranch = "capstan/developer-1-g1";
    const successor = sameRole(h, "developer-2");
    const successorBranch = "capstan/developer-2-g1";
    pane(h, h.developer.agentId, predecessorBranch);
    pane(h, successor.agentId, successorBranch);
    tips[predecessorBranch] = OLD_COMMIT;
    tips[successorBranch] = NEW_TIP;
    lies.add(`${predecessorBranch}:${OLD_COMMIT}`);
    const accepted = await call(h, h.developer.credential, "report", [
      OLD_COMMIT,
      "done",
    ]);
    assert.ok(accepted.ok, JSON.stringify(accepted));

    h.core.endAgent(ctx(h.core, h.owner), h.developer.agentId);
    h.core.recordAgentReplaced(ctx(h.core, h.owner), {
      predecessorId: h.developer.agentId,
      successorId: successor.agentId,
    });
    for (const command of [
      "report",
      "inbox",
      "ack",
      "status",
      "send",
      "wait",
    ]) {
      const answer = await call(
        h,
        h.developer.credential,
        command,
        command === "report"
          ? [OLD_COMMIT, "again"]
          : command === "ack"
            ? ["x"]
            : command === "send"
              ? ["@pm", "hi"]
              : [],
      );
      assert.equal(answer.ok, false, command);
      assert.equal((answer as { code: string }).code, "unauthorized", command);
    }

    const stale = await call(h, successor.credential, "report", [
      OLD_COMMIT,
      "this is the old work",
    ]);
    assert.equal(stale.ok, false);
    assert.match((stale as { message: string }).message, /report_rejected: .*/);
    const rows = h.core.agentReports(h.owner, 10);
    const refused = rows.find((r) => r.agentId === successor.agentId);
    assert.equal(refused?.state, "rejected");
    assert.equal(refused?.reason, "not_on_branch");
    assert.equal(
      rows.filter((r) => r.state === "accepted").length,
      1,
      "only the predecessor's own report is accepted, and it was before the replacement",
    );
    lies.add(`${successorBranch}:${NEW_TIP}`);
    const fresh = await call(h, successor.credential, "report", [
      NEW_TIP,
      "new work",
    ]);
    assert.ok(fresh.ok, JSON.stringify(fresh));
  } finally {
    await close(h);
  }
});

test("an ended agent cannot be sent a message, and a message queued before the end is cancelled, so no instruction reaches the dead id", async () => {
  const h = await harness();
  try {
    const queued = h.core.enqueueMessage(ctx(h.core, h.pm.credential), {
      recipientAgentId: h.developer.agentId,
      body: "before the end",
    }).messageId;
    h.core.endAgent(ctx(h.core, h.owner), h.developer.agentId);
    assert.equal(h.core.message(queued)!.state, "cancelled");
    const answer = await call(h, h.pm.credential, "send", [
      h.developer.agentId,
      "after the end",
    ]);
    assert.equal(answer.ok, false);
    assert.match(
      (answer as { message: string }).message,
      /not active|recipient|ended|unknown/i,
    );
    assert.equal(
      h.core
        .messagesFor(h.developer.agentId)
        .filter((m) => m.state !== "cancelled").length,
      0,
    );
  } finally {
    await close(h);
  }
});
