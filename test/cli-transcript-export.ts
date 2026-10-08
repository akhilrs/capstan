/**
 * Writes the transcripts the Rust front end (rust/crates/cstan) is tested against: each one runs the Node CLI
 * (dist/src/cli.js) on fixed argv, environment and directory layout against a fake daemon that replays one fixed
 * reply, and records the request the daemon saw and what the CLI printed and exited with. The Rust tests replay every
 * transcript against the same fake daemon and compare stdout, stderr and the exit code byte for byte; a transcript
 * with `fallback: true` is one the front end must hand to Node instead. Run `npm run build && node
 * dist/test/cli-transcript-export.js` after an intended change and commit the result; cli-transcript.test.ts fails
 * while the committed files differ from a fresh export.
 */
import { spawn } from "node:child_process";
import {
  mkdirSync,
  mkdtempSync,
  readFileSync,
  realpathSync,
  rmSync,
  symlinkSync,
  writeFileSync,
} from "node:fs";
import net from "node:net";
import os from "node:os";
import path from "node:path";

const root = path.resolve(import.meta.dirname, "..", "..");
export const TRANSCRIPT_DIRECTORY = path.join(
  root,
  "rust",
  "crates",
  "cstan",
  "tests",
  "transcripts",
);
const CLI = path.join(root, "dist", "src", "cli.js");

/** The fixed `Date.now()` of every transcript. */
export const NOW = Date.parse("2026-10-07T12:00:00.000Z");
const MAX_RESPONSE_BYTES = 1_048_576;

/** The front end binary the tests run: CSTAN_FRONT_BIN, else the host build of the workspace. */
export function frontEndBinary(): string {
  const configured = process.env.CSTAN_FRONT_BIN;
  if (configured !== undefined && configured !== "") return configured;
  return path.join(
    path.resolve(root, "rust", process.env.CARGO_TARGET_DIR ?? "target"),
    "release",
    "cstan",
  );
}

/** What the fake daemon sends once it has read the request line. */
export type Reply =
  | { readonly kind: "line"; readonly text: string }
  | {
      readonly kind: "expanded";
      readonly head: string;
      readonly text: string;
      readonly count: number;
      readonly tail: string;
      readonly newline: boolean;
    }
  | { readonly kind: "raw"; readonly hex: string }
  | { readonly kind: "partial"; readonly text: string }
  | { readonly kind: "close" }
  | { readonly kind: "hang" };

export interface Transcript {
  readonly name: string;
  readonly argv: readonly string[];
  /** The whole environment of the run. `$ROOT` stands for the scratch directory. */
  readonly env: Readonly<Record<string, string>>;
  /** Made under the scratch directory before the run. */
  readonly layout: {
    readonly dirs: readonly string[];
    readonly files: readonly string[];
    readonly links: readonly { readonly path: string; readonly to: string }[];
  };
  /** Relative to the scratch directory. */
  readonly cwd: string;
  readonly now: number;
  readonly daemon: {
    readonly socket: string;
    readonly reply: Reply;
  } | null;
  /** The request line the daemon read, or null when none arrived. */
  readonly request: string | null;
  /** True when the front end must not answer this itself. */
  readonly fallback: boolean;
  /** What the Node CLI did; null for a command the front end never serves (it is not run). */
  readonly node: {
    readonly stdout: string;
    readonly stderr: string;
    readonly exit: number | null;
  } | null;
}

interface Case {
  readonly name: string;
  readonly argv: readonly string[];
  readonly env?: Readonly<Record<string, string>>;
  readonly dirs?: readonly string[];
  readonly files?: readonly string[];
  readonly links?: readonly { readonly path: string; readonly to: string }[];
  readonly cwd?: string;
  readonly socket?: string;
  readonly reply?: Reply;
  readonly fallback?: boolean;
  /** Skip the run of Node: the command is not one the front end serves. */
  readonly skipNode?: boolean;
}

const TOKEN = "tok-0123456789abcdef0123456789abcdef";
const SOCKET = "$ROOT/proj/.capstan/state/control.sock";
const AGENT_ENV = {
  CAPSTAN_TOKEN: TOKEN,
  CAPSTAN_SOCKET: SOCKET,
  CAPSTAN_AGENT_ID: "developer-1",
  TZ: "UTC",
};
const BASE_ENV = { TZ: "UTC" };
const PROJECT_DIRS = ["proj/.capstan/state", "proj/sub/deeper"];

const line = (value: unknown): Reply => ({
  kind: "line",
  text: JSON.stringify(value),
});
const raw = (text: string): Reply => ({ kind: "line", text });
const okReply = (result: unknown): Reply => line({ ok: true, result });
const refused = (code: string, message: string): Reply =>
  line({ ok: false, code, message });

function agentCase(
  name: string,
  argv: readonly string[],
  reply: Reply,
  extra: Partial<Case> = {},
): Case {
  return {
    name,
    argv,
    env: AGENT_ENV,
    dirs: PROJECT_DIRS,
    cwd: "proj",
    socket: SOCKET,
    reply,
    ...extra,
  };
}

function fallbackCase(
  name: string,
  argv: readonly string[],
  extra: Partial<Case> = {},
): Case {
  return {
    name,
    argv,
    env: AGENT_ENV,
    dirs: PROJECT_DIRS,
    cwd: "proj",
    fallback: true,
    ...extra,
  };
}

const message = (
  id: string,
  from: string,
  body: string,
  extra: Record<string, unknown> = {},
) => ({
  messageId: id,
  state: "sent",
  from,
  fromAgentId: from,
  body,
  ...extra,
});

const CONTROLLER = {
  pid: 4242,
  projectRoot: "/work/proj",
  ledgerPath: "/work/proj/.capstan/state/controller.sqlite",
};

const DATES = [
  ["30s", "2026-10-07T11:59:30.000Z"],
  ["minutes", "2026-10-07T11:50:00Z"],
  ["hours", "2026-10-07T08:00:00.000Z"],
  ["days", "2026-10-04T12:00:00Z"],
  ["future", "2026-10-07T12:00:30.000Z"],
  ["no-zone", "2026-10-07T11:00:00"],
  ["no-zone-fraction", "2026-10-07T11:00:00.5"],
  ["offset", "2026-10-07T13:00:00+02:00"],
  ["compact-offset", "2026-10-07T13:00:00+0200"],
  ["lower-z", "2026-10-07t11:00:00z"],
  ["long-fraction", "2026-10-07T11:00:00.123456Z"],
  ["rollover", "2026-02-31T00:00:00Z"],
  ["date-only", "2026-10-06"],
  ["year-only", "2026"],
  ["year-month", "2026-09"],
  ["expanded-year", "+002026-10-07T00:00:00Z"],
  ["not-a-date", "yesterday"],
  ["empty", ""],
] as const;

function buildCases(): Case[] {
  const cases: Case[] = [];
  const add = (...more: Case[]): void => void cases.push(...more);

  // ping and status
  add(
    agentCase("ping-text", ["ping"], okReply(CONTROLLER)),
    agentCase("ping-json", ["ping", "--json"], okReply(CONTROLLER)),
    agentCase("ping-older-daemon", ["ping"], okReply({ pid: 4242 })),
    agentCase(
      "ping-nested-controller",
      ["ping"],
      okReply({ pid: 7, controller: CONTROLLER, projectRoot: "/other" }),
    ),
    agentCase(
      "ping-controller-null-member",
      ["ping"],
      okReply({
        controller: { projectRoot: null, ledgerPath: "/led" },
        projectRoot: "/top",
        ledgerPath: "/topled",
      }),
    ),
    agentCase(
      "ping-non-string-root",
      ["ping"],
      okReply({ projectRoot: 5, ledgerPath: "/led" }),
    ),
    agentCase("ping-string-result", ["ping"], okReply("pong")),
    agentCase("ping-no-result", ["ping"], line({ ok: true })),
    agentCase("ping-null-result", ["ping"], okReply(null)),
    agentCase("ping-truthy-ok", ["ping"], line({ ok: 1, result: { pid: 1 } })),
    agentCase(
      "status-plain",
      ["status"],
      okReply({
        ...CONTROLLER,
        run: { state: "running", stateVersion: 12 },
        roles: ["PM", "Developer"],
        work: [],
      }),
    ),
    agentCase(
      "status-json",
      ["status", "--json"],
      okReply({ controller: CONTROLLER, run: { state: "running" } }),
    ),
    agentCase(
      "status-pause-run-and-agents",
      ["status"],
      okReply({
        controller: CONTROLLER,
        pause: {
          run: {
            reason: "operator break",
            actorId: "operator",
            pausedAt: "2026-10-07T11:00:00Z",
          },
          agents: [
            {
              agentId: "developer-1",
              reason: "check",
              actorId: "pm-1",
              pausedAt: "2026-10-07T11:59:30Z",
            },
            {
              agentId: null,
              reason: "no id",
              actorId: "pm-1",
              pausedAt: "2026-10-04T11:59:30Z",
            },
            { reason: "x", actorId: "y" },
          ],
        },
      }),
    ),
    agentCase(
      "status-pause-empty",
      ["status"],
      okReply({ controller: CONTROLLER, pause: { run: null, agents: [] } }),
    ),
    agentCase(
      "status-pause-odd-entries",
      ["status"],
      okReply({
        controller: CONTROLLER,
        pause: { run: "text", agents: ["a", 5, true] },
      }),
    ),
    agentCase(
      "status-pause-null-agent",
      ["status"],
      okReply({ controller: CONTROLLER, pause: { agents: [null] } }),
    ),
    agentCase(
      "status-pause-agents-not-array",
      ["status"],
      okReply({ controller: CONTROLLER, pause: { agents: "nope" } }),
    ),
    agentCase(
      "status-pause-string",
      ["status"],
      okReply({ controller: CONTROLLER, pause: "yes" }),
    ),
    agentCase(
      "status-pause-json-unchanged",
      ["status", "--json"],
      okReply({
        controller: CONTROLLER,
        pause: { run: { reason: "r", actorId: "a", pausedAt: "bad" } },
      }),
    ),
    agentCase(
      "status-pm-mail-stale",
      ["status"],
      okReply({
        controller: CONTROLLER,
        pmMail: {
          pending: 3,
          oldestAgeSeconds: 1500,
          oldestMessageId: "m-9",
          stale: true,
        },
      }),
    ),
    agentCase(
      "status-pm-mail-stale-no-id",
      ["status"],
      okReply({
        controller: CONTROLLER,
        pmMail: { pending: 1, oldestAgeSeconds: 59, stale: true },
      }),
    ),
    agentCase(
      "status-pm-mail-pending",
      ["status"],
      okReply({
        controller: CONTROLLER,
        pmMail: { pending: 2, oldestAgeSeconds: 125 },
      }),
    ),
    agentCase(
      "status-pm-mail-none",
      ["status"],
      okReply({ controller: CONTROLLER, pmMail: { pending: 0 } }),
    ),
    agentCase(
      "status-pm-mail-odd-numbers",
      ["status"],
      okReply({
        controller: CONTROLLER,
        pmMail: { pending: "2", oldestAgeSeconds: "180" },
      }),
    ),
    agentCase(
      "status-pm-mail-nan-age",
      ["status"],
      okReply({
        controller: CONTROLLER,
        pmMail: { pending: 2, oldestAgeSeconds: "soon" },
      }),
    ),
    agentCase(
      "status-pm-mail-negative-age",
      ["status"],
      okReply({
        controller: CONTROLLER,
        pmMail: { pending: 2, oldestAgeSeconds: -90 },
      }),
    ),
    agentCase(
      "status-pm-mail-big-age",
      ["status"],
      okReply({
        controller: CONTROLLER,
        pmMail: { pending: 2, oldestAgeSeconds: 1e30 },
      }),
    ),
    agentCase(
      "status-pm-mail-stale-string-flag",
      ["status"],
      okReply({
        controller: CONTROLLER,
        pmMail: { pending: 2, stale: "true" },
      }),
    ),
    agentCase(
      "status-no-controller-fields",
      ["status"],
      okReply({ run: { state: "running" } }),
    ),
    agentCase("status-no-result", ["status"], line({ ok: true })),
    agentCase("status-array-result", ["status"], okReply([1, "a", null])),
    agentCase(
      "status-after-separator-json",
      ["status", "--", "--json"],
      okReply({ a: 1 }),
      {
        fallback: true,
      },
    ),
  );
  for (const [label, when] of DATES)
    add(
      agentCase(
        `status-pause-date-${label}`,
        ["status"],
        okReply({
          controller: CONTROLLER,
          pause: {
            run: { reason: "r", actorId: "a", pausedAt: when },
          },
        }),
      ),
      agentCase(
        `inbox-notice-date-${label}`,
        ["ack", "m-1"],
        okReply({
          acked: true,
          unread: { count: 2, oldestQueuedAt: when, actionNeeded: 0 },
        }),
      ),
    );

  // inbox and wait
  const two = [
    message("m-1", "pm-1", "do the thing"),
    {
      ...message("m-2", "controller", "Finding: line one\nline two"),
      fromAgentId: "controller-7",
      state: "delivered",
      actionNeeded: true,
    },
  ];
  add(
    agentCase(
      "inbox-messages",
      ["inbox"],
      okReply({ count: 2, messages: two }),
    ),
    agentCase(
      "inbox-messages-json",
      ["inbox", "--json"],
      okReply({ count: 2, messages: two }),
    ),
    agentCase("inbox-empty", ["inbox"], okReply({ count: 0, messages: [] })),
    agentCase("inbox-empty-no-count", ["inbox"], okReply({ messages: [] })),
    agentCase("inbox-no-messages-member", ["inbox"], okReply({ count: 0 })),
    agentCase(
      "inbox-messages-no-count",
      ["inbox"],
      okReply({ messages: [message("m-3", "a", "b")] }),
    ),
    agentCase(
      "inbox-count-string",
      ["inbox"],
      okReply({ count: "2", messages: [message("m-3", "a", "b")] }),
    ),
    agentCase(
      "wait-timed-out",
      ["wait"],
      okReply({ messages: [], timedOut: true }),
    ),
    agentCase(
      "wait-timed-out-json",
      ["wait", "--json"],
      okReply({ messages: [], timedOut: true }),
    ),
    agentCase(
      "wait-timed-out-string-flag",
      ["wait"],
      okReply({ messages: [], timedOut: "true" }),
    ),
    agentCase(
      "wait-message",
      ["wait", "30"],
      okReply({ count: 1, messages: [message("m-4", "pm-1", "hi")] }),
    ),
    agentCase(
      "inbox-with-unread-notice",
      ["inbox"],
      okReply({
        count: 1,
        messages: [message("m-4", "pm-1", "hi")],
        unread: { count: 4, oldestQueuedAt: "2026-10-07T11:00:00Z" },
      }),
    ),
    agentCase(
      "inbox-odd-message-members",
      ["inbox"],
      okReply({
        count: 4,
        messages: [
          { messageId: 5, state: null, from: ["a", null, "b"], body: { x: 1 } },
          { fromAgentId: "z" },
          "text",
          7,
        ],
      }),
    ),
    agentCase(
      "inbox-same-sender-undefined",
      ["inbox"],
      okReply({ count: 1, messages: [{ messageId: "m", body: "b" }] }),
    ),
    agentCase(
      "inbox-null-message",
      ["inbox"],
      okReply({ count: 1, messages: [null] }),
    ),
    agentCase("inbox-no-result", ["inbox"], line({ ok: true })),
    agentCase("inbox-null-result", ["inbox"], okReply(null)),
    agentCase("inbox-string-result", ["inbox"], okReply("hello")),
    agentCase("inbox-number-result", ["inbox"], okReply(3)),
    agentCase("inbox-array-result", ["inbox"], okReply([1, 2])),
    agentCase("inbox-messages-string", ["inbox"], okReply({ messages: "abc" })),
    agentCase(
      "inbox-messages-empty-string",
      ["inbox"],
      okReply({ messages: "" }),
    ),
    agentCase(
      "inbox-messages-object-length-zero",
      ["inbox"],
      okReply({ messages: { length: 0 } }),
    ),
    agentCase("inbox-messages-number", ["inbox"], okReply({ messages: 4 })),
    agentCase("inbox-messages-null", ["inbox"], okReply({ messages: null })),
    agentCase(
      "inbox-body-lone-surrogate",
      ["inbox"],
      raw(
        '{"ok":true,"result":{"count":1,"messages":[{"messageId":"m","state":"sent","from":"a","fromAgentId":"a","body":"x\\ud800y"}]}}',
      ),
    ),
    agentCase(
      "inbox-body-unicode",
      ["inbox"],
      okReply({
        count: 1,
        messages: [message("m", "pm", "héllo ✓ 😀   end")],
      }),
    ),
  );

  // notices, warnings and generic output
  add(
    agentCase(
      "ack-notice-action-needed",
      ["ack", "m-1"],
      okReply({
        acked: true,
        unread: {
          count: 3,
          oldestQueuedAt: "2026-10-07T11:55:00Z",
          actionNeeded: 2,
        },
      }),
    ),
    agentCase(
      "ack-notice-no-action",
      ["ack", "m-1"],
      okReply({
        acked: true,
        unread: {
          count: 1,
          oldestQueuedAt: "2026-10-07T11:55:00Z",
          actionNeeded: 0,
        },
      }),
    ),
    agentCase(
      "ack-notice-json-silent",
      ["ack", "--json", "m-1"],
      okReply({
        acked: true,
        unread: { count: 1, oldestQueuedAt: "2026-10-07T11:55:00Z" },
      }),
    ),
    agentCase(
      "ack-notice-no-oldest",
      ["ack", "m-1"],
      okReply({ acked: true, unread: { count: 1 } }),
    ),
    agentCase(
      "ack-notice-count-zero",
      ["ack", "m-1"],
      okReply({ acked: true, unread: { count: 0 } }),
    ),
    agentCase(
      "ack-notice-fractional",
      ["ack", "m-1"],
      okReply({
        acked: true,
        unread: { count: 1.5, actionNeeded: 0.5, oldestQueuedAt: 5 },
      }),
    ),
    agentCase(
      "ack-notice-count-string",
      ["ack", "m-1"],
      okReply({ acked: true, unread: { count: "1" } }),
    ),
    agentCase(
      "ack-unread-not-object",
      ["ack", "m-1"],
      okReply({ acked: true, unread: "many" }),
    ),
    agentCase(
      "spawn-warning",
      ["spawn", "developer", "--task", "plan-1/pkg"],
      okReply({
        state: "started",
        agentId: "developer-9",
        warning: "[env] pass names that are not set: FOO",
      }),
    ),
    agentCase(
      "spawn-warning-json",
      ["spawn", "developer", "--json"],
      okReply({ state: "started", warning: "careful" }),
    ),
    agentCase(
      "spawn-warning-not-string",
      ["spawn", "developer"],
      okReply({ state: "started", warning: 5 }),
    ),
    agentCase(
      "warning-and-notice",
      ["release", "developer-2"],
      okReply({
        state: "released",
        warning: "w",
        unread: { count: 2, oldestQueuedAt: "2026-10-07T11:00:00Z" },
      }),
    ),
    agentCase(
      "report-nested-render",
      ["report", "0123456789abcdef0123456789abcdef01234567", "done"],
      okReply({
        state: "accepted",
        nested: { list: [1, [2, 3], { deep: null }], flag: true, none: null },
        empty: {},
        emptyList: [],
        text: "multi\nline",
      }),
    ),
    agentCase(
      "report-nested-render-json",
      ["report", "--json", "abc", "done"],
      okReply({
        state: "accepted",
        nested: { list: [1, [2, 3], { deep: null }], flag: true, none: null },
        empty: {},
        emptyList: [],
        text: "multi\nline   é 😀",
      }),
    ),
    agentCase(
      "plan-numbers",
      ["plan", "show"],
      raw(
        '{"ok":true,"result":{"a":0.1,"b":-0,"c":1e21,"d":1e-7,"e":123456789012345680000,"f":0.30000000000000004,"g":9007199254740993,"h":5e-324,"i":1.7976931348623157e308,"j":-1.5e-9,"k":100,"l":1.0,"m":2e-1}}',
      ),
    ),
    agentCase(
      "plan-numbers-json",
      ["plan", "show", "--json"],
      raw(
        '{"ok":true,"result":{"a":0.1,"b":-0,"c":1e21,"d":1e-7,"e":123456789012345680000,"f":0.30000000000000004,"g":9007199254740993,"h":5e-324,"i":1.7976931348623157e308,"j":-1.5e-9,"k":100,"l":1.0,"m":2e-1}}',
      ),
    ),
    agentCase(
      "plan-key-order",
      ["plan", "show"],
      raw(
        '{"ok":true,"result":{"b":1,"2":0,"a":2,"b":3,"1":9,"__proto__":1,"":"empty key"}}',
      ),
    ),
    agentCase(
      "plan-key-order-json",
      ["plan", "show", "--json"],
      raw(
        '{"ok":true,"result":{"b":1,"2":0,"a":2,"b":3,"1":9,"": "empty key","10":1,"9":2}}',
      ),
    ),
    agentCase(
      "plan-string-result",
      ["plan", "show"],
      okReply("just text\nsecond line"),
    ),
    agentCase(
      "plan-string-result-json",
      ["plan", "show", "--json"],
      okReply("just text\nsecond line"),
    ),
    agentCase(
      "plan-lone-surrogate-key",
      ["plan", "show"],
      raw('{"ok":true,"result":{"\\ud800":"v\\udc00"}}'),
    ),
    agentCase(
      "plan-lone-surrogate-json",
      ["plan", "show", "--json"],
      raw('{"ok":true,"result":{"\\ud800":"v\\udc00"}}'),
    ),
    agentCase(
      "plan-extra-members-ignored",
      ["plan", "show"],
      raw('{"ok":true,"result":{"a":1},"extra":[1,2],"v":1}'),
    ),
    agentCase(
      "plan-no-result-json",
      ["plan", "show", "--json"],
      line({ ok: true }),
    ),
    agentCase(
      "observe-text",
      ["observe", "developer-2", "40"],
      okReply({ text: "screen\n  text" }),
    ),
    agentCase(
      "review-pass",
      ["review", "pass", "looks good"],
      okReply({ state: "passed" }),
    ),
    agentCase(
      "request-review-ok",
      ["request-review", "r-1"],
      okReply({ state: "requested" }),
    ),
    agentCase(
      "integrate-ok",
      ["integrate", "r-1", "r-2"],
      okReply({ integrationId: "i-1" }),
    ),
    agentCase(
      "pause-ok",
      ["pause", "--reason", "break"],
      okReply({ paused: true }),
    ),
    agentCase(
      "resume-ok",
      ["resume", "--reason", "back"],
      okReply({ resumed: true }),
    ),
    agentCase(
      "replace-ok",
      ["replace", "developer-2"],
      okReply({ state: "started" }),
    ),
    agentCase(
      "send-ok",
      ["send", "pm-1", "hello there"],
      okReply({ messageId: "m-7" }),
    ),
    agentCase(
      "send-unicode-arg",
      ["send", "pm-1", "héllo ✓ 😀"],
      okReply({ messageId: "m-8" }),
    ),
    agentCase(
      "report-ok",
      ["report", "0123456789abcdef0123456789abcdef01234567", "summary"],
      okReply({ reportId: "r-1" }),
    ),
    agentCase(
      "ask-not-implemented",
      ["ask", "a question"],
      refused("not_implemented", "ask is not implemented yet (Stage 2e)"),
    ),
  );

  // argument parsing
  add(
    agentCase(
      "args-json-after-positional",
      ["send", "pm-1", "text", "--json"],
      okReply({ messageId: "m" }),
    ),
    agentCase(
      "args-json-before-positional",
      ["send", "--json", "pm-1", "text"],
      okReply({ messageId: "m" }),
    ),
    agentCase(
      "args-separator",
      ["send", "pm-1", "--", "--json", "--"],
      okReply({ messageId: "m" }),
    ),
    agentCase(
      "args-separator-only",
      ["send", "--"],
      okReply({ messageId: "m" }),
    ),
    agentCase(
      "args-json-twice",
      ["send", "--json", "--json", "x"],
      okReply({ messageId: "m" }),
    ),
    agentCase(
      "args-flags-pass-through",
      ["spawn", "developer", "--type", "feat", "--title", "t", "--interrupt"],
      okReply({ state: "started" }),
    ),
    agentCase(
      "args-newline-and-quotes",
      ["send", "pm-1", 'line1\nline2 "quoted" \\ back'],
      okReply({ messageId: "m" }),
    ),
    agentCase(
      "args-control-chars",
      ["send", "pm-1", "tab\there\u0001"],
      okReply({ messageId: "m" }),
    ),
    agentCase(
      "args-long-but-allowed",
      ["send", "pm-1", "x".repeat(60000)],
      okReply({ messageId: "m" }),
    ),
    agentCase(
      "args-supplementary-plane",
      ["send", "pm-1", "\u{1f600}".repeat(100)],
      okReply({ messageId: "m" }),
    ),
    agentCase(
      "args-ping-json-after-separator",
      ["ping", "--", "--json"],
      okReply({ pid: 1 }),
      { fallback: true },
    ),
    agentCase("cwd-in-subdirectory", ["ping"], okReply(CONTROLLER), {
      cwd: "proj/sub/deeper",
    }),
    {
      ...agentCase("cwd-outside-any-project", ["ping"], okReply({ pid: 1 })),
      dirs: [...PROJECT_DIRS, "elsewhere"],
      cwd: "elsewhere",
    },
    {
      ...agentCase("socket-through-symlink", ["ping"], okReply({ pid: 1 })),
      links: [{ path: "alias", to: "proj" }],
      env: {
        ...AGENT_ENV,
        CAPSTAN_SOCKET: "$ROOT/alias/.capstan/state/control.sock",
      },
      socket: "$ROOT/proj/.capstan/state/control.sock",
    },
    {
      ...agentCase("socket-unnormalized-path", ["ping"], okReply({ pid: 1 })),
      env: {
        ...AGENT_ENV,
        CAPSTAN_SOCKET: "$ROOT/proj/sub/../.capstan/./state//control.sock",
      },
      socket: "$ROOT/proj/.capstan/state/control.sock",
    },
    {
      ...agentCase("cwd-through-symlink", ["ping"], okReply({ pid: 1 })),
      links: [{ path: "alias", to: "proj/sub" }],
      cwd: "alias",
    },
    {
      ...agentCase("extra-env-passes", ["ping"], okReply({ pid: 1 })),
      env: { ...AGENT_ENV, CAPSTAN_ALLOW_FOREIGN_SOCKET: "1", EXTRA: "x" },
    },
    {
      ...agentCase(
        "socket-outside-project-no-capstan",
        ["ping"],
        okReply({ pid: 1 }),
      ),
      dirs: ["free/dir", "sockets"],
      cwd: "free/dir",
      env: { ...AGENT_ENV, CAPSTAN_SOCKET: "$ROOT/sockets/d.sock" },
      socket: "$ROOT/sockets/d.sock",
    },
  );

  // refusals
  add(
    agentCase(
      "refused-invalid-request",
      ["send", "pm-1", "x"],
      refused("invalid_request", "args must be at most 64 non-empty strings"),
    ),
    agentCase(
      "refused-invalid-request-json",
      ["send", "--json", "pm-1", "x"],
      refused("invalid_request", "bad input"),
    ),
    agentCase(
      "refused-error",
      ["report", "abc", "s"],
      refused("error", "the command failed"),
    ),
    agentCase(
      "refused-forbidden",
      ["spawn", "developer"],
      refused("forbidden", "this command needs the operator credential"),
    ),
    agentCase(
      "refused-rejected",
      ["ack", "m-1"],
      refused("rejected", "message_not_found: no such message"),
    ),
    agentCase(
      "refused-unauthorized",
      ["ping"],
      refused("unauthorized", "credential not accepted"),
    ),
    agentCase(
      "refused-unknown-command",
      ["ping"],
      refused("unknown_command", "unknown command"),
    ),
    agentCase(
      "refused-conflict",
      ["report", "abc", "s"],
      refused("conflict", "state changed; try again"),
    ),
    agentCase("refused-empty-message", ["ping"], refused("rejected", "")),
    agentCase(
      "refused-no-message",
      ["ping"],
      line({ ok: false, code: "rejected" }),
    ),
    agentCase("refused-no-code", ["ping"], line({ ok: false, message: "m" })),
    agentCase(
      "refused-null-code",
      ["ping"],
      line({ ok: false, code: null, message: null }),
    ),
    agentCase(
      "refused-number-code",
      ["ping"],
      line({ ok: false, code: 7, message: 8 }),
    ),
    agentCase(
      "refused-invalid-request-no-message",
      ["ping"],
      line({ ok: false, code: "invalid_request" }),
    ),
    agentCase(
      "refused-error-no-message",
      ["ping"],
      line({ ok: false, code: "error" }),
    ),
    agentCase(
      "refused-falsy-ok-zero",
      ["ping"],
      line({ ok: 0, code: "x", message: "y" }),
    ),
    agentCase(
      "refused-falsy-ok-null",
      ["ping"],
      line({ ok: null, code: "x", message: "y" }),
    ),
    agentCase(
      "refused-falsy-ok-empty-string",
      ["ping"],
      line({ ok: "", code: "x", message: "y" }),
    ),
    agentCase(
      "refused-truthy-ok-string",
      ["ping"],
      line({ ok: "no", result: 1 }),
    ),
    agentCase(
      "refused-multiline-message",
      ["ping"],
      refused("rejected", "first\nsecond: third"),
    ),
    agentCase(
      "refused-message-unicode",
      ["ping"],
      refused("rejected", "héllo ✓ 😀"),
    ),
    agentCase(
      "refused-message-lone-surrogate",
      ["ping"],
      raw('{"ok":false,"code":"rejected","message":"a\\ud800b"}'),
    ),
  );

  // after the request was sent
  const big = (newline: boolean): Reply => ({
    kind: "expanded",
    head: '{"ok":true,"result":"',
    text: "x",
    count: MAX_RESPONSE_BYTES,
    tail: '"}',
    newline,
  });
  add(
    agentCase("after-send-closed", ["ack", "m-1"], { kind: "close" }),
    agentCase("after-send-partial-line", ["ack", "m-1"], {
      kind: "partial",
      text: '{"ok":true',
    }),
    agentCase("after-send-timed-out", ["ping"], { kind: "hang" }),
    agentCase("after-send-malformed-text", ["ack", "m-1"], raw("not json")),
    agentCase("after-send-malformed-array", ["ack", "m-1"], raw("[1,2]")),
    agentCase("after-send-malformed-null", ["ack", "m-1"], raw("null")),
    agentCase("after-send-malformed-string", ["ack", "m-1"], raw('"ok"')),
    agentCase("after-send-malformed-number", ["ack", "m-1"], raw("5")),
    agentCase(
      "after-send-malformed-no-ok",
      ["ack", "m-1"],
      raw('{"result":1}'),
    ),
    agentCase("after-send-malformed-empty-line", ["ack", "m-1"], raw("")),
    agentCase("after-send-malformed-bom", ["ack", "m-1"], raw('﻿{"ok":true}')),
    agentCase("after-send-malformed-bad-utf8", ["ack", "m-1"], {
      kind: "raw",
      hex: "7b226f6b223a747275652c22726573756c74223aff7d0a",
    }),
    agentCase(
      "after-send-malformed-trailing-garbage",
      ["ack", "m-1"],
      raw('{"ok":true} x'),
    ),
    agentCase("after-send-too-large-no-newline", ["ack", "m-1"], big(false)),
    agentCase("after-send-line-at-limit", ["ack", "m-1"], {
      kind: "expanded",
      head: '{"ok":true,"result":"',
      text: "y",
      count: MAX_RESPONSE_BYTES - 24,
      tail: '"}',
      newline: true,
    }),
    agentCase(
      "after-send-second-line-ignored",
      ["ack", "m-1"],
      raw('{"ok":true,"result":1}\n{"ok":false}'),
    ),
    agentCase(
      "after-send-crlf",
      ["ack", "m-1"],
      raw('{"ok":true,"result":1}\r'),
    ),
  );

  // the inbox hook
  const hook = (
    name: string,
    reply: Reply | undefined,
    extra: Partial<Case> = {},
  ): Case =>
    reply === undefined
      ? {
          name,
          argv: ["inbox", "--hook"],
          env: AGENT_ENV,
          dirs: PROJECT_DIRS,
          cwd: "proj",
          ...extra,
        }
      : agentCase(name, ["inbox", "--hook"], reply, extra);
  add(
    hook(
      "hook-count-n",
      okReply({ count: 3, oldestQueuedAt: "2026-10-07T11:45:00Z" }),
    ),
    hook("hook-count-1-no-oldest", okReply({ count: 1 })),
    hook("hook-count-zero", okReply({ count: 0 })),
    hook("hook-count-negative", okReply({ count: -1 })),
    hook("hook-count-string", okReply({ count: "2" })),
    hook(
      "hook-count-fractional",
      okReply({ count: 1.5, oldestQueuedAt: "2026-10-07T11:00:00" }),
    ),
    hook(
      "hook-count-huge",
      okReply({ count: 1e21, oldestQueuedAt: "garbage" }),
    ),
    hook(
      "hook-oldest-future",
      okReply({ count: 2, oldestQueuedAt: "2026-10-08T00:00:00Z" }),
    ),
    hook(
      "hook-oldest-no-zone",
      okReply({ count: 2, oldestQueuedAt: "2026-10-07T10:30:00" }),
    ),
    hook(
      "hook-oldest-offset",
      okReply({ count: 2, oldestQueuedAt: "2026-10-07T13:30:00+02:00" }),
    ),
    hook("hook-oldest-number", okReply({ count: 2, oldestQueuedAt: 5 })),
    hook("hook-result-string", okReply("text")),
    hook("hook-result-null", okReply(null)),
    hook("hook-no-result", line({ ok: true })),
    hook("hook-refused", refused("forbidden", "no")),
    hook("hook-malformed", raw("not json")),
    hook("hook-closed", { kind: "close" }),
    hook("hook-timed-out", { kind: "hang" }),
    hook("hook-daemon-down", undefined),
    hook("hook-no-env", undefined, { env: BASE_ENV }),
    hook("hook-no-token", undefined, {
      env: { TZ: "UTC", CAPSTAN_SOCKET: SOCKET, CAPSTAN_AGENT_ID: "a" },
    }),
    hook("hook-no-socket", undefined, {
      env: { TZ: "UTC", CAPSTAN_TOKEN: TOKEN, CAPSTAN_AGENT_ID: "a" },
    }),
    hook("hook-no-agent-id", undefined, {
      env: { TZ: "UTC", CAPSTAN_TOKEN: TOKEN, CAPSTAN_SOCKET: SOCKET },
    }),
    hook("hook-empty-agent-id", undefined, {
      env: { ...AGENT_ENV, CAPSTAN_AGENT_ID: "" },
    }),
    hook("hook-relative-socket", undefined, {
      env: { ...AGENT_ENV, CAPSTAN_SOCKET: "proj/.capstan/state/control.sock" },
    }),
    hook("hook-foreign-socket", okReply({ count: 5 }), {
      dirs: [...PROJECT_DIRS, "other/.capstan/state"],
      env: {
        ...AGENT_ENV,
        CAPSTAN_SOCKET: "$ROOT/other/.capstan/state/control.sock",
      },
      socket: "$ROOT/other/.capstan/state/control.sock",
    }),
    hook("hook-token-with-space", okReply({ count: 2 }), {
      env: { ...AGENT_ENV, CAPSTAN_TOKEN: "tok en" },
    }),
    hook("hook-socket-with-space", okReply({ count: 2 }), {
      dirs: ["proj/.capstan/state", "free dir"],
      cwd: "free dir",
      env: { ...AGENT_ENV, CAPSTAN_SOCKET: "$ROOT/free dir/s.sock" },
      socket: "$ROOT/free dir/s.sock",
    }),
    hook("hook-oversize-token", okReply({ count: 2 }), {
      env: { ...AGENT_ENV, CAPSTAN_TOKEN: "t".repeat(70000) },
    }),
  );
  // `inbox --hook` with anything else around it is a plain inbox with `--hook` as its argument
  add(
    agentCase(
      "hook-flag-is-plain-inbox-with-json",
      ["inbox", "--hook", "--json"],
      okReply({ count: 0, messages: [] }),
    ),
    agentCase(
      "hook-flag-after-separator",
      ["inbox", "--", "--hook"],
      okReply({ count: 0, messages: [] }),
    ),
    agentCase(
      "hook-flag-after-json",
      ["inbox", "--json", "--hook"],
      okReply({ count: 0, messages: [] }),
    ),
  );

  // what must fall back
  const foreign = {
    dirs: [...PROJECT_DIRS, "other/.capstan/state"],
    env: {
      ...AGENT_ENV,
      CAPSTAN_SOCKET: "$ROOT/other/.capstan/state/control.sock",
    },
  };
  add(
    fallbackCase("fallback-no-env", ["ping"], { env: BASE_ENV }),
    fallbackCase("fallback-token-only", ["ping"], {
      env: { TZ: "UTC", CAPSTAN_TOKEN: TOKEN },
    }),
    fallbackCase("fallback-socket-only", ["ping"], {
      env: { TZ: "UTC", CAPSTAN_SOCKET: SOCKET },
    }),
    fallbackCase("fallback-empty-token", ["ping"], {
      env: { ...AGENT_ENV, CAPSTAN_TOKEN: "" },
    }),
    fallbackCase("fallback-empty-socket", ["ping"], {
      env: { ...AGENT_ENV, CAPSTAN_SOCKET: "" },
    }),
    fallbackCase("fallback-both-empty", ["ping"], {
      env: { TZ: "UTC", CAPSTAN_TOKEN: "", CAPSTAN_SOCKET: "" },
    }),
    fallbackCase("fallback-relative-socket", ["ping"], {
      env: { ...AGENT_ENV, CAPSTAN_SOCKET: "proj/.capstan/state/control.sock" },
    }),
    fallbackCase("fallback-token-space", ["ping"], {
      env: { ...AGENT_ENV, CAPSTAN_TOKEN: "tok en" },
    }),
    fallbackCase("fallback-token-newline", ["ping"], {
      env: { ...AGENT_ENV, CAPSTAN_TOKEN: "tok\nen" },
    }),
    fallbackCase("fallback-token-tab", ["ping"], {
      env: { ...AGENT_ENV, CAPSTAN_TOKEN: "tok\ten" },
    }),
    fallbackCase("fallback-token-del", ["ping"], {
      env: { ...AGENT_ENV, CAPSTAN_TOKEN: "tok\u007fen" },
    }),
    fallbackCase("fallback-token-c1", ["ping"], {
      env: { ...AGENT_ENV, CAPSTAN_TOKEN: "tok\u0085en" },
    }),
    fallbackCase("fallback-token-nbsp", ["ping"], {
      env: { ...AGENT_ENV, CAPSTAN_TOKEN: "tok en" },
    }),
    fallbackCase("fallback-token-ideographic-space", ["ping"], {
      env: { ...AGENT_ENV, CAPSTAN_TOKEN: "tok　en" },
    }),
    fallbackCase("fallback-token-line-separator", ["ping"], {
      env: { ...AGENT_ENV, CAPSTAN_TOKEN: "tok en" },
    }),
    fallbackCase("fallback-token-bom", ["ping"], {
      env: { ...AGENT_ENV, CAPSTAN_TOKEN: "tok﻿en" },
    }),
    fallbackCase("fallback-socket-space", ["ping"], {
      dirs: ["free dir"],
      cwd: "free dir",
      env: { ...AGENT_ENV, CAPSTAN_SOCKET: "$ROOT/free dir/s.sock" },
    }),
    fallbackCase("fallback-socket-newline", ["ping"], {
      env: { ...AGENT_ENV, CAPSTAN_SOCKET: "$ROOT/a\nb" },
    }),
    fallbackCase("fallback-foreign-socket", ["ping"], foreign),
    fallbackCase(
      "fallback-foreign-socket-send",
      ["send", "pm-1", "x"],
      foreign,
    ),
    fallbackCase("fallback-foreign-socket-allowed", ["ping"], {
      ...foreign,
      env: { ...foreign.env, CAPSTAN_ALLOW_FOREIGN_SOCKET: "1" },
    }),
    fallbackCase("fallback-foreign-socket-allowed-status", ["status"], {
      ...foreign,
      env: { ...foreign.env, CAPSTAN_ALLOW_FOREIGN_SOCKET: "1" },
    }),
    fallbackCase("fallback-empty-arg", ["send", "pm-1", ""]),
    fallbackCase("fallback-empty-arg-first", ["ack", ""]),
    fallbackCase("fallback-replacement-character", [
      "send",
      "pm-1",
      "bad � text",
    ]),
    fallbackCase("fallback-replacement-character-only", ["send", "�"]),
    fallbackCase("fallback-empty-arg-after-separator", ["send", "--", ""]),
    fallbackCase("fallback-oversize-frame", [
      "send",
      "pm-1",
      "x".repeat(70000),
    ]),
    fallbackCase("fallback-oversize-frame-by-escapes", [
      "send",
      "pm-1",
      '"'.repeat(33000),
    ]),
    fallbackCase("fallback-oversize-frame-by-controls", [
      "send",
      "pm-1",
      "\u0001".repeat(11000),
    ]),
    fallbackCase("fallback-no-daemon-enoent", ["ping"]),
    fallbackCase("fallback-no-daemon-enoent-send", ["send", "pm-1", "x"]),
    fallbackCase("fallback-no-daemon-econnrefused", ["ping"], {
      files: ["proj/.capstan/state/control.sock"],
    }),
    fallbackCase("fallback-no-daemon-econnrefused-ack", ["ack", "m-1"], {
      files: ["proj/.capstan/state/control.sock"],
    }),
    fallbackCase("fallback-status-watch", ["status", "--watch"], {
      skipNode: true,
    }),
    fallbackCase(
      "fallback-status-watch-interval",
      ["status", "--watch", "--interval", "5"],
      { skipNode: true },
    ),
    fallbackCase("fallback-status-extra-positional", ["status", "extra"]),
    fallbackCase("fallback-ping-extra-positional", ["ping", "extra"]),
    fallbackCase("fallback-status-offline", ["status"], {
      env: BASE_ENV,
      skipNode: true,
    }),
    fallbackCase("fallback-agent-command-no-env", ["send", "pm-1", "x"], {
      env: BASE_ENV,
      skipNode: true,
    }),
    fallbackCase("fallback-no-command", [], { skipNode: true }),
    fallbackCase("fallback-unknown-command", ["frobnicate"]),
    fallbackCase("fallback-version", ["--version"]),
    fallbackCase("fallback-version-word", ["version"]),
    fallbackCase("fallback-help", ["--help"]),
    fallbackCase("fallback-front-version-extra", ["__front-version", "x"], {
      skipNode: true,
    }),
    fallbackCase("fallback-json-first", ["--json", "ping"]),
    fallbackCase("fallback-uppercase-command", ["PING"]),
    fallbackCase("fallback-cancel", ["cancel", "w-1"], { skipNode: true }),
    fallbackCase("fallback-peek", ["peek", "x"], { skipNode: true }),
    fallbackCase("fallback-assign", ["assign", "x"], { skipNode: true }),
    fallbackCase("fallback-resolve", ["resolve", "x"], { skipNode: true }),
    fallbackCase("fallback-pm-restart", ["pm", "restart"], { skipNode: true }),
    fallbackCase("fallback-launch", ["launch"], { skipNode: true }),
    fallbackCase("fallback-shutdown", ["shutdown"], { skipNode: true }),
    fallbackCase("fallback-inspect", ["inspect", "x"], { skipNode: true }),
    fallbackCase("fallback-dash", ["dash"], { skipNode: true }),
    fallbackCase("fallback-init", ["init"], { skipNode: true }),
    fallbackCase("fallback-start", ["start"], { skipNode: true }),
    fallbackCase("fallback-stop", ["stop"], { skipNode: true }),
    fallbackCase("fallback-config", ["config", "check"], { skipNode: true }),
    fallbackCase("fallback-herdr-config", ["herdr-config"], { skipNode: true }),
    fallbackCase("fallback-daemon", ["daemon"], { skipNode: true }),
    fallbackCase("fallback-restart-helper", ["__restart-helper", "x"], {
      skipNode: true,
    }),
  );
  add(
    {
      name: "front-version",
      argv: ["__front-version"],
      env: BASE_ENV,
      cwd: ".",
      skipNode: true,
    },
    {
      name: "front-version-in-agent-env",
      argv: ["__front-version"],
      env: AGENT_ENV,
      dirs: PROJECT_DIRS,
      cwd: "proj",
      skipNode: true,
    },
  );
  return cases;
}

function substitute(text: string, scratch: string): string {
  return text.split("$ROOT").join(scratch);
}

/** The version `cstan --version` prints. Transcripts hold `$VERSION` for it, so a release bump cannot make them stale. */
const PACKAGE_VERSION = (
  JSON.parse(readFileSync(path.join(root, "package.json"), "utf8")) as {
    version: string;
  }
).version;

function restore(text: string, scratch: string): string {
  return text
    .split(scratch)
    .join("$ROOT")
    .split(`cstan ${PACKAGE_VERSION}\n`)
    .join("cstan $VERSION\n");
}

function replyBytes(reply: Reply): Buffer | null {
  switch (reply.kind) {
    case "line":
      return Buffer.from(`${reply.text}\n`);
    case "expanded":
      return Buffer.from(
        `${reply.head}${reply.text.repeat(reply.count)}${reply.tail}${reply.newline ? "\n" : ""}`,
      );
    case "raw":
      return Buffer.from(reply.hex, "hex");
    case "partial":
      return Buffer.from(reply.text);
    case "close":
    case "hang":
      return null;
  }
}

interface FakeDaemon {
  readonly request: () => string | null;
  readonly close: () => Promise<void>;
}

/** Listens on `socketPath`; reads one request line and answers it as `reply` says. */
async function fakeDaemon(
  socketPath: string,
  reply: Reply,
): Promise<FakeDaemon> {
  let request: string | null = null;
  const connections = new Set<net.Socket>();
  const server = net.createServer((connection) => {
    connections.add(connection);
    connection.on("error", () => undefined);
    let bytes = Buffer.alloc(0);
    connection.on("data", (chunk: Buffer) => {
      if (request !== null) return;
      bytes = Buffer.concat([bytes, chunk]);
      const newline = bytes.indexOf(10);
      if (newline < 0) return;
      request = bytes.subarray(0, newline).toString("utf8");
      const payload = replyBytes(reply);
      if (reply.kind === "partial" || reply.kind === "close") {
        if (payload === null) connection.end();
        else connection.end(payload);
      } else if (payload !== null) connection.write(payload);
    });
  });
  await new Promise<void>((resolve, reject) => {
    server.once("error", reject);
    server.listen(socketPath, resolve);
  });
  return {
    request: () => request,
    close: async () => {
      for (const connection of connections) connection.destroy();
      await new Promise<void>((resolve) => server.close(() => resolve()));
    },
  };
}

interface NodeRun {
  readonly stdout: string;
  readonly stderr: string;
  readonly exit: number | null;
}

async function runNode(
  argv: readonly string[],
  env: Readonly<Record<string, string>>,
  cwd: string,
): Promise<NodeRun> {
  const shim = `data:text/javascript,${encodeURIComponent(`Date.now=()=>${NOW};`)}`;
  const child = spawn(process.execPath, [`--import=${shim}`, CLI, ...argv], {
    cwd,
    env: { ...env },
    stdio: ["ignore", "pipe", "pipe"],
  });
  const out: Buffer[] = [];
  const err: Buffer[] = [];
  child.stdout.on("data", (chunk: Buffer) => out.push(chunk));
  child.stderr.on("data", (chunk: Buffer) => err.push(chunk));
  const exit = await new Promise<number | null>((resolve) =>
    child.once("close", (code) => resolve(code)),
  );
  return {
    stdout: Buffer.concat(out).toString("utf8"),
    stderr: Buffer.concat(err).toString("utf8"),
    exit,
  };
}

async function record(source: Case): Promise<Transcript> {
  const scratch = realpathSync(
    mkdtempSync(path.join(os.tmpdir(), "cstan-tr-")),
  );
  try {
    const dirs = source.dirs ?? [];
    for (const dir of dirs)
      mkdirSync(path.join(scratch, dir), { recursive: true });
    for (const file of source.files ?? []) {
      mkdirSync(path.dirname(path.join(scratch, file)), { recursive: true });
      writeFileSync(path.join(scratch, file), "");
    }
    for (const link of source.links ?? [])
      symlinkSync(path.join(scratch, link.to), path.join(scratch, link.path));
    mkdirSync(path.join(scratch, source.cwd ?? "."), { recursive: true });
    const env = Object.fromEntries(
      Object.entries(source.env ?? BASE_ENV).map(([key, value]) => [
        key,
        substitute(value, scratch),
      ]),
    );
    const argv = source.argv.map((arg) => substitute(arg, scratch));
    const cwd = path.join(scratch, source.cwd ?? ".");
    const daemon =
      source.socket !== undefined && source.reply !== undefined
        ? await fakeDaemon(substitute(source.socket, scratch), source.reply)
        : undefined;
    let node: NodeRun | null = null;
    try {
      if (source.skipNode !== true) node = await runNode(argv, env, cwd);
    } finally {
      await daemon?.close();
    }
    return {
      name: source.name,
      argv: source.argv,
      env: source.env ?? BASE_ENV,
      layout: {
        dirs: source.dirs ?? [],
        files: source.files ?? [],
        links: source.links ?? [],
      },
      cwd: source.cwd ?? ".",
      now: NOW,
      daemon:
        source.socket !== undefined && source.reply !== undefined
          ? { socket: source.socket, reply: source.reply }
          : null,
      request:
        daemon === undefined
          ? null
          : restore(daemon.request() ?? "", scratch) || null,
      fallback: source.fallback === true,
      node:
        node === null
          ? null
          : {
              stdout: restore(node.stdout, scratch),
              stderr: restore(node.stderr, scratch),
              exit: node.exit,
            },
    };
  } finally {
    rmSync(scratch, { recursive: true, force: true });
  }
}

/** Every transcript by file name, as the text the exporter writes. */
export async function exportTranscripts(): Promise<Map<string, string>> {
  const cases = buildCases();
  const names = new Set<string>();
  for (const source of cases) {
    if (names.has(source.name))
      throw new Error(`duplicate case ${source.name}`);
    names.add(source.name);
  }
  const transcripts: Transcript[] = new Array<Transcript>(cases.length);
  let next = 0;
  const worker = async (): Promise<void> => {
    for (;;) {
      const at = next++;
      const source = cases[at];
      if (source === undefined) return;
      transcripts[at] = await record(source);
    }
  };
  await Promise.all(Array.from({ length: 8 }, worker));
  return new Map(
    transcripts.map((t) => [
      `${t.name}.json`,
      `${JSON.stringify(t, null, 2)}\n`,
    ]),
  );
}

if (import.meta.filename === process.argv[1]) {
  const files = await exportTranscripts();
  mkdirSync(TRANSCRIPT_DIRECTORY, { recursive: true });
  for (const [name, text] of files)
    writeFileSync(path.join(TRANSCRIPT_DIRECTORY, name), text);
  process.stdout.write(
    `wrote ${files.size} transcripts to ${TRANSCRIPT_DIRECTORY}\n`,
  );
}
