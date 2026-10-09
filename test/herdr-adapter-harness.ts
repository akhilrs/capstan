import {
  mkdirSync,
  mkdtempSync,
  readFileSync,
  realpathSync,
  rmSync,
} from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { HerdrAdapter } from "../src/herdr/adapter.js";
import { type HerdrResult } from "../src/herdr/runner.js";
import { type PromptAnswer } from "../src/herdr/prompt-relay.js";

export const RULE = "─".repeat(40);
export const NBSP = " ";
export const DIM = "\u001b[2m";
export const RESET = "\u001b[0m";
export const fixture = (name: string): string =>
  readFileSync(path.resolve("test/fixtures", name), "utf8");
export function idleScreen(...typed: string[]): string {
  const first =
    typed.length === 0
      ? `❯${NBSP}${RESET}${DIM}Try "x"${RESET}`
      : `❯${NBSP}${typed[0]}`;
  return [
    "",
    RULE,
    first,
    ...typed.slice(1).map((line) => `  ${line}`),
    RULE,
    "  footer",
  ].join("\r\n");
}
export const SHELL_READY = "user in dir\r\n❯ ";
export const CLEAN_ENV = { HOME: "/h", PATH: "/p", TERM: "t" };
export interface FakePane {
  paneId: string;
  workspaceId: string;
  agent?: string;
  status: string;
  screen: string;
  checkout: string;
  terminalId?: string;
  tokens?: Record<string, unknown>;
}
export class FakeHerdr {
  readonly calls: string[][] = [];
  readonly events: string[] = [];
  readonly panes = new Map<string, FakePane>();
  readonly agentStates = new Map<
    string,
    { paneId: string; statuses: string[]; kind?: string }
  >();
  readonly root = realpathSync(
    mkdtempSync(path.join(tmpdir(), "capstan-fake-herdr-")),
  );
  startError: { code: string; message: string } | undefined;
  notification: Record<string, unknown> = { shown: true };
  /** How `pane move` behaves: it works, errors without moving, errors after moving, or errors and loses the pane. */
  moveMode: "ok" | "error-no-move" | "error-moved" | "error-lost" = "ok";
  moveTargetWorkspace = "w9";
  extraPaneAtMovedPath = false;
  /** The 1-based `pane list` call that fails, if any. */
  failListCall: number | undefined;
  /** Raw entries added to the `pane list` answer, to test how unreadable panes are handled. */
  extraListEntries: unknown[] = [];
  private listCalls = 0;
  layoutRects: Array<{
    pane_id: string;
    rect: { width: unknown; height: unknown };
  }> = [];
  zoomed = false;
  private moved = 20;
  onKey: ((pane: FakePane, key: string) => void) | undefined;
  onText: ((pane: FakePane, text: string) => void) | undefined;
  /** Screens `pane read` returns once each, in order, before the pane's own screen. */
  readQueue: string[] = [];
  onRun: ((pane: FakePane, command: string) => void) | undefined = (
    pane,
    command,
  ) => {
    const rc = /--rcfile '([^']+)'/.exec(command);
    if (rc) {
      rmSync(path.dirname(rc[1]!), { recursive: true, force: true });
      pane.screen = "❯ ";
    }
  };
  private counter = 0;

  cleanup(): void {
    rmSync(this.root, { recursive: true, force: true });
  }

  callsTo(command: string, sub?: string): string[][] {
    return this.calls.filter(
      (call) => call[0] === command && (sub === undefined || call[1] === sub),
    );
  }

  add(role: string, screen: string, status = "unknown"): FakePane {
    this.counter += 1;
    const pane: FakePane = {
      paneId: `w${this.counter}:p1`,
      workspaceId: `w${this.counter}`,
      status,
      screen,
      checkout: path.join(this.root, `${role}-${this.counter}`),
    };
    mkdirSync(pane.checkout, { recursive: true });
    this.panes.set(pane.paneId, pane);
    return pane;
  }

  private json(result: unknown): HerdrResult {
    return { code: 0, stdout: JSON.stringify({ id: "x", result }), stderr: "" };
  }

  private failure(code: string, message: string): HerdrResult {
    return {
      code: 1,
      stdout: JSON.stringify({ id: "x", error: { code, message } }),
      stderr: "",
    };
  }

  readonly run = async (args: readonly string[]): Promise<HerdrResult> => {
    this.calls.push([...args]);
    const [command, sub] = args;
    if (command === "--version")
      return { code: 0, stdout: "herdr 0.9.1\n", stderr: "" };
    const flag = (name: string): string | undefined => {
      const at = args.indexOf(name);
      return at < 0 ? undefined : args[at + 1];
    };
    if (command === "worktree" && sub === "create") {
      const pane = this.add("worker", SHELL_READY);
      return this.json({
        root_pane: { pane_id: pane.paneId },
        workspace: {
          workspace_id: pane.workspaceId,
          worktree: { checkout_path: pane.checkout },
        },
      });
    }
    if (command === "workspace" && sub === "create") {
      const pane = this.add("workspace", SHELL_READY);
      return this.json({
        root_pane: { pane_id: pane.paneId },
        workspace: { workspace_id: pane.workspaceId },
        tab: { tab_id: `${pane.workspaceId}:t1` },
      });
    }
    if (command === "worktree" && sub === "remove")
      return this.json({ forced: true });
    if (command === "pane" && sub === "get") {
      const pane = this.panes.get(args[2]!);
      if (!pane) return this.failure("pane_not_found", "no such pane");
      return this.json({
        pane: {
          pane_id: pane.paneId,
          agent_status: pane.status,
          ...(pane.agent ? { agent: "claude" } : {}),
          ...(pane.terminalId === undefined
            ? {}
            : { terminal_id: pane.terminalId }),
          ...(pane.tokens === undefined ? {} : { tokens: pane.tokens }),
        },
      });
    }
    if (command === "agent" && sub === "get") {
      const state = this.agentStates.get(args[2]!);
      if (!state) return this.failure("agent_not_found", "no such agent");
      const status =
        state.statuses.length > 1
          ? state.statuses.shift()!
          : state.statuses[0]!;
      return this.json({
        agent: {
          name: args[2],
          pane_id: state.paneId,
          agent_status: status,
          agent: state.kind ?? "claude",
        },
      });
    }
    if (command === "pane" && sub === "read") {
      const pane = this.panes.get(args[2]!);
      if (!pane) return this.failure("pane_not_found", "no such pane");
      return {
        code: 0,
        stdout: this.readQueue.shift() ?? pane.screen,
        stderr: "",
      };
    }
    if (command === "pane" && sub === "send-text") {
      const pane = this.panes.get(args[2]!)!;
      this.events.push(`text:${args[3]}`);
      this.onText?.(pane, args[3]!);
      return { code: 0, stdout: "", stderr: "" };
    }
    if (command === "pane" && sub === "send-keys") {
      const pane = this.panes.get(args[2]!)!;
      this.events.push(`key:${args[3]}`);
      this.onKey?.(pane, args[3]!);
      return { code: 0, stdout: "", stderr: "" };
    }
    if (command === "pane" && sub === "run") {
      const pane = this.panes.get(args[2]!)!;
      this.events.push("run");
      this.onRun?.(pane, args[3]!);
      return { code: 0, stdout: "", stderr: "" };
    }
    if (command === "agent" && sub === "prompt") {
      this.events.push(`prompt:${args[3]}`);
      return this.json({});
    }
    if (command === "agent" && sub === "start") {
      const name = args[2]!;
      const pane = this.panes.get(flag("--pane")!)!;
      if (this.startError)
        return this.failure(this.startError.code, this.startError.message);
      pane.agent = name;
      pane.status = "idle";
      this.agentStates.set(name, {
        paneId: pane.paneId,
        statuses: ["idle"],
        kind: flag("--kind") ?? "claude",
      });
      return this.json({ agent: { name } });
    }
    if (command === "pane" && sub === "layout")
      return this.json({
        layout: {
          tab_id: "w9:t1",
          workspace_id: "w9",
          zoomed: this.zoomed,
          panes: this.layoutRects,
        },
      });
    if (command === "pane" && sub === "list") {
      this.listCalls += 1;
      if (this.listCalls === this.failListCall)
        return this.failure("timeout", "no answer");
      return this.json({
        panes: [
          ...[...this.panes.values()].map((pane) => ({
            pane_id: pane.paneId,
            tab_id: `${pane.workspaceId}:t1`,
            workspace_id: pane.workspaceId,
            cwd: pane.checkout,
          })),
          ...this.extraListEntries,
        ],
      });
    }
    if (command === "pane" && sub === "move") {
      const old = this.panes.get(args[2]!);
      if (!old) return this.failure("pane_not_found", "no such pane");
      if (this.moveMode === "error-no-move")
        return this.failure("move_refused", "the move was refused");
      this.panes.delete(old.paneId);
      if (this.moveMode === "error-lost")
        return this.failure("timeout", "no answer");
      this.moved += 1;
      const placed: FakePane = {
        ...old,
        paneId: `${this.moveTargetWorkspace}:p${this.moved}`,
        workspaceId: this.moveTargetWorkspace,
      };
      this.panes.set(placed.paneId, placed);
      if (this.extraPaneAtMovedPath) {
        this.moved += 1;
        const twin = {
          ...placed,
          paneId: `${this.moveTargetWorkspace}:p${this.moved}`,
        };
        this.panes.set(twin.paneId, twin);
      }
      if (this.moveMode === "error-moved")
        return this.failure("timeout", "no answer");
      return this.json({
        move_result: {
          changed: true,
          pane: {
            pane_id: placed.paneId,
            tab_id: `${placed.workspaceId}:t1`,
            workspace_id: placed.workspaceId,
            cwd: placed.checkout,
          },
        },
      });
    }
    if (command === "pane" && sub === "close") return this.json({});
    if (command === "notification" && sub === "show")
      return this.json(this.notification);
    if (command === "tab" && sub === "create") {
      const pane = this.add("worker", SHELL_READY);
      return this.json({
        tab: { tab_id: `${pane.workspaceId}:t9` },
        root_pane: { pane_id: pane.paneId },
      });
    }
    if (command === "agent" && sub === "rename") {
      const state = [...this.agentStates.entries()].find(
        ([, value]) => value.paneId === args[2],
      );
      if (!state) return this.failure("agent_not_found", "no agent there");
      this.agentStates.delete(state[0]);
      this.agentStates.set(args[3]!, state[1]);
      return this.json({});
    }
    if (command === "workspace" && sub === "rename") return this.json({});
    if (command === "tab" && sub === "rename") return this.json({});
    if (
      (command === "pane" || command === "workspace") &&
      sub === "report-metadata"
    )
      return this.json({});
    return this.failure("unknown", `unhandled ${args.join(" ")}`);
  };
}
export interface Harness {
  readonly fake: FakeHerdr;
  readonly adapter: HerdrAdapter;
}
export function harness(projectSlug?: string): Harness {
  const fake = new FakeHerdr();
  let clock = 0;
  const adapter = new HerdrAdapter({
    ...(projectSlug === undefined ? {} : { projectSlug }),
    run: fake.run,
    tempRoot: fake.root,
    sleep: async () => {
      clock += 100;
    },
    now: () => clock,
    pollMs: 100,
  });
  return { fake, adapter };
}
export async function startedWorker(
  h: Harness,
  screen = idleScreen(),
  statuses = ["idle"],
  kind: "claude" | "codex" | "omp" = "claude",
): Promise<{ paneId: string; agent: string; pane: FakePane }> {
  const { paneId } = await h.adapter.createWorktree({
    workspaceId: "w9",
    branch: "cap/task/dev-g1",
    label: "dev",
  });
  await h.adapter.startAgent({
    name: "dev",
    kind,
    paneId,
    args: [],
    environment: CLEAN_ENV,
  });
  const pane = h.fake.panes.get(paneId)!;
  pane.screen = screen;
  h.fake.agentStates.set("dev", { paneId, statuses, kind });
  h.fake.calls.length = 0;
  h.fake.events.length = 0;
  return { paneId, agent: "dev", pane };
}
export interface Answered {
  readonly outcome: Awaited<ReturnType<HerdrAdapter["answerPrompt"]>>;
  readonly before: number;
  readonly logged: string[];
}
export async function answer(
  h: Harness,
  paneId: string,
  promptSha: string,
  reply: PromptAnswer,
): Promise<Answered> {
  const logged: string[] = [];
  let before = 0;
  const outcome = await h.adapter.answerPrompt({
    paneId,
    promptSha,
    answer: reply,
    beforeType: () => {
      before += 1;
      logged.push("before");
    },
    log: (entry) => {
      logged.push(entry.key);
    },
  });
  return { outcome, before, logged };
}
