import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Box, Text, useApp, useInput, useWindowSize } from "ink";
import { signalsOf } from "../watch.js";
import {
  ACTION_KEYS,
  availableDecisions,
  confirmText,
  observeAction,
  toWireCall,
  type DashAction,
} from "./actions.js";
import { age, clean, pushSample, truncate } from "./format.js";
import {
  allocate,
  columnsOf,
  FOOTER_ROWS,
  HEADER_ROWS,
  layoutFor,
  MIN_COLUMNS,
  MIN_ROWS,
  type PanelId,
} from "./layout.js";
import {
  buildDashModel,
  EMPTY_CHANGES,
  trackChanges,
  type ChangeState,
  type DashModel,
} from "./model.js";
import { abortableSleep, createPoller, type Poller } from "./poller.js";
import { ThemeContext, type Theme } from "./theme.js";
import {
  AgentsPanel,
  FindingsPanel,
  Header,
  PipelinePanel,
  QueuePanel,
  WorkPanel,
  type PanelProps,
} from "./components/panels.js";
import { SPINNER_MS, useTick } from "./components/widgets.js";

export const SAMPLE_LIMIT = 30;
export const NOTICE_MS = 5000;
export const DEFAULT_CONFIRM_DELAY_MS = 300;
const BELL = "\u0007";

export type CallResult =
  | { readonly ok: true; readonly result: unknown }
  | { readonly ok: false; readonly message: string };

export interface AppDeps {
  readonly fetch: () => Promise<Record<string, unknown>>;
  readonly call: (
    command: string,
    args: readonly string[],
  ) => Promise<CallResult>;
  readonly workerLimit: number | null;
  readonly intervalSeconds: number;
  readonly theme: Theme;
  readonly now?: () => number;
  /** A second `y` sooner than this after the prompt opened is ignored, so a held key cannot confirm. */
  readonly confirmDelayMs?: number;
  readonly write?: (text: string) => void;
  readonly size?: { readonly columns: number; readonly rows: number };
}

type Link = "starting" | "ok" | "down" | "toolarge";
interface Prompt {
  readonly action: DashAction;
  readonly openedAt: number;
}
interface Peek {
  readonly agentId: string;
  readonly agentStatus: string;
  readonly text: string;
}

const PANEL_KEYS: Record<string, PanelId> = {
  "1": "agents",
  "2": "pipeline",
  "3": "queue",
  "4": "findings",
  "5": "work",
};

const HELP = [
  "Tab / Shift+Tab or 1-5  focus a panel      up/down or j/k  move",
  "o  observe the selected agent (read-only)",
  "y  retry   s  skip   c  cancel   the selected message; then press y to confirm",
  "p  pause polling   r  poll now   ?  close help   q  quit",
  "Working is inferred: activity within 30 s or a message in flight.",
];

export function App({ deps }: { deps: AppDeps }) {
  const { exit } = useApp();
  const window = useWindowSize();
  const columns = deps.size?.columns ?? window.columns;
  const rows = deps.size?.rows ?? window.rows;
  const now = deps.now ?? Date.now;
  const write =
    deps.write ?? ((text: string) => void process.stdout.write(text));
  const confirmDelay = deps.confirmDelayMs ?? DEFAULT_CONFIRM_DELAY_MS;

  const [model, setModel] = useState<DashModel | null>(null);
  const [link, setLink] = useState<Link>("starting");
  const [changes, setChanges] = useState<ChangeState>(EMPTY_CHANGES);
  const [unresolvedSamples, setUnresolvedSamples] = useState<number[]>([]);
  const [focus, setFocus] = useState<PanelId>("queue");
  const [cursors, setCursors] = useState<Record<PanelId, number>>({
    agents: 0,
    pipeline: 0,
    queue: 0,
    findings: 0,
    work: 0,
  });
  const [paused, setPaused] = useState(false);
  const [help, setHelp] = useState(false);
  const [prompt, setPrompt] = useState<Prompt | null>(null);
  const [peek, setPeek] = useState<Peek | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const lastOk = useRef<number | null>(null);
  const seenSignals = useRef<Set<string> | undefined>(undefined);
  const poller = useRef<Poller | undefined>(undefined);
  const changesRef = useRef<ChangeState>(EMPTY_CHANGES);

  // A one-second re-render keeps the ages in the header and rows current.
  useTick(1000, true);
  const nowMs = now();
  const working = model?.counts.working ?? 0;
  const tick = useTick(SPINNER_MS, !deps.theme.reducedMotion && working > 0);

  useEffect(() => {
    const instance = createPoller({
      intervalMs: deps.intervalSeconds * 1000,
      fetch: deps.fetch,
      sleep: abortableSleep,
      onStatus: (status, changed) => {
        lastOk.current = now();
        setLink("ok");
        const unresolved = Array.isArray(status.messages)
          ? status.messages.length
          : 0;
        setUnresolvedSamples((samples) =>
          pushSample(samples, unresolved, SAMPLE_LIMIT),
        );
        if (!changed) return;
        const next = buildDashModel(status, now(), deps.workerLimit);
        const tracked = trackChanges(changesRef.current, next);
        changesRef.current = tracked;
        setChanges(tracked);
        setModel(next);
        const signals = signalsOf(status);
        const previous = seenSignals.current;
        if (
          !deps.theme.reducedMotion &&
          previous !== undefined &&
          [...signals].some((key) => !previous.has(key))
        )
          write(BELL);
        seenSignals.current = signals;
      },
      onError: (error) => {
        const code =
          error instanceof Error && "code" in error ? error.code : undefined;
        setLink(code === "EMSGSIZE" ? "toolarge" : "down");
      },
    });
    poller.current = instance;
    void instance.run();
    return () => instance.stop();
  }, []);

  useEffect(() => {
    if (notice === null) return;
    const timer = setTimeout(() => setNotice(null), NOTICE_MS);
    return () => clearTimeout(timer);
  }, [notice]);

  useEffect(() => {
    if (prompt === null || prompt.action.kind !== "resolve" || model === null)
      return;
    const target = prompt.action.message;
    const current = model.queue.messages.find(
      (m) => m.messageId === target.messageId,
    );
    if (current === undefined || current.state !== target.state) {
      setPrompt(null);
      setNotice("cancelled: the message changed");
    }
  }, [model, prompt]);

  const panels = useMemo<PanelId[]>(
    () =>
      model !== null && model.work.length > 0
        ? ["agents", "pipeline", "queue", "findings", "work"]
        : ["agents", "pipeline", "queue", "findings"],
    [model],
  );

  const rowCount = (panel: PanelId): number => {
    if (model === null) return 0;
    switch (panel) {
      case "agents":
        return model.agents.length;
      case "queue":
        return model.queue.messages.length;
      case "findings":
        return model.findings.length;
      case "work":
        return model.work.length;
      case "pipeline":
        return (
          model.pipeline.reports.items.length +
          model.pipeline.reviews.items.length +
          model.pipeline.integrations.items.length
        );
    }
  };

  const canAct = link === "ok" && !paused && !busy && model !== null;

  const execute = useCallback(
    async (action: DashAction) => {
      const wire = toWireCall(action);
      setBusy(true);
      const outcome = await deps.call(wire.command, wire.args);
      setBusy(false);
      if (action.kind === "observe") {
        if (!outcome.ok)
          return setNotice(truncate(clean(outcome.message), 200));
        const r = (outcome.result ?? {}) as Record<string, unknown>;
        return setPeek({
          agentId: action.agentId,
          agentStatus: clean(r.agentStatus ?? "unknown"),
          text: typeof r.text === "string" ? r.text : "",
        });
      }
      setNotice(
        outcome.ok
          ? `${action.decision} ${action.message.messageId}: ${clean(
              ((outcome.result ?? {}) as Record<string, unknown>).state ??
                "done",
            )}`
          : truncate(clean(outcome.message), 200),
      );
      poller.current?.pollNow();
    },
    [deps],
  );

  useInput((input, key) => {
    if (peek !== null) {
      if (key.escape || input === "q") setPeek(null);
      return;
    }
    if (prompt !== null) {
      if (input === "y" && now() - prompt.openedAt >= confirmDelay) {
        const action = prompt.action;
        setPrompt(null);
        void execute(action);
      } else if (!(input === "y")) {
        setPrompt(null);
        setNotice("cancelled");
      }
      return;
    }
    if (input === "q" || (key.ctrl && input === "c")) return exit();
    if (input === "?") return setHelp((h) => !h);
    if (help) return setHelp(false);
    if (input === "p") {
      poller.current?.setPaused(!paused);
      return setPaused(!paused);
    }
    if (input === "r") return poller.current?.pollNow();
    const jump = PANEL_KEYS[input];
    if (jump !== undefined && panels.includes(jump)) return setFocus(jump);
    if (key.tab) {
      const at = panels.indexOf(focus);
      const step = key.shift ? -1 : 1;
      return setFocus(panels[(at + step + panels.length) % panels.length]!);
    }
    if (key.upArrow || key.downArrow || input === "j" || input === "k") {
      const delta = key.upArrow || input === "k" ? -1 : 1;
      const max = Math.max(0, rowCount(focus) - 1);
      return setCursors((c) => ({
        ...c,
        [focus]: Math.min(max, Math.max(0, c[focus] + delta)),
      }));
    }
    if (model === null || !canAct) return;
    if (input === "o" && focus === "agents") {
      const agent = model.agents[cursors.agents];
      const action = agent === undefined ? undefined : observeAction(agent);
      if (action !== undefined) void execute(action);
      return;
    }
    if (focus === "queue") {
      const message = model.queue.messages[cursors.queue];
      if (message === undefined) return;
      const decision = (
        Object.keys(ACTION_KEYS) as (keyof typeof ACTION_KEYS)[]
      ).find((d) => ACTION_KEYS[d] === input);
      if (decision === undefined) return;
      if (!availableDecisions(message).includes(decision))
        return setNotice(
          `${decision} does not apply to a message in state ${message.state}`,
        );
      setPrompt({
        action: { kind: "resolve", decision, message },
        openedAt: now(),
      });
    }
  });

  const layout = layoutFor(columns, rows);
  if (layout.mode === "tiny")
    return (
      <ThemeContext.Provider value={deps.theme}>
        <Text>
          terminal too small (need {MIN_COLUMNS}x{MIN_ROWS}, have {columns}x
          {rows})
        </Text>
      </ThemeContext.Provider>
    );

  const ageText =
    lastOk.current === null
      ? "-"
      : age(new Date(lastOk.current).toISOString(), nowMs);
  const bodyRows = rows - HEADER_ROWS - FOOTER_ROWS;
  const colWidth =
    layout.mode === "wide" ? Math.floor(columns / 2) - 1 : columns;

  const renderPanel = (id: PanelId, capacity: number) => {
    if (model === null) return null;
    const props: PanelProps = {
      capacity,
      width: colWidth,
      focused: focus === id,
      cursor: cursors[id],
      highlight: changes.highlight,
      nowMs,
      layout,
      tick,
    };
    switch (id) {
      case "agents":
        return (
          <AgentsPanel
            key={id}
            {...props}
            agents={model.agents}
            ended={model.endedAgentsHidden}
          />
        );
      case "pipeline":
        return <PipelinePanel key={id} {...props} pipeline={model.pipeline} />;
      case "queue":
        return (
          <QueuePanel
            key={id}
            {...props}
            queue={model.queue}
            samples={unresolvedSamples}
          />
        );
      case "findings":
        return <FindingsPanel key={id} {...props} findings={model.findings} />;
      case "work":
        return <WorkPanel key={id} {...props} work={model.work} />;
    }
  };

  const footerTop =
    prompt !== null
      ? confirmText(prompt.action)
      : notice !== null
        ? notice
        : busy
          ? "working..."
          : "";

  return (
    <ThemeContext.Provider value={deps.theme}>
      <Box flexDirection="column" width={columns} height={rows}>
        {model !== null && (
          <Header
            model={model}
            link={link}
            ageText={ageText}
            intervalSeconds={deps.intervalSeconds}
            paused={paused}
            width={columns}
          />
        )}
        {model === null && (
          <Text>
            {link === "down"
              ? "controller not answering, retrying"
              : "connecting to the controller..."}
          </Text>
        )}
        {peek !== null ? (
          <Box flexDirection="column" height={bodyRows}>
            <Text bold>
              {truncate(
                `screen of ${peek.agentId} (${peek.agentStatus}); unverified text, Esc closes`,
                columns,
              )}
            </Text>
            {peek.text
              .split("\n")
              .slice(-(bodyRows - 1))
              .map((line, i) => (
                <Text key={i} wrap="truncate-end">
                  {clean(line)}
                </Text>
              ))}
          </Box>
        ) : help ? (
          <Box flexDirection="column" height={bodyRows}>
            {HELP.map((line) => (
              <Text key={line}>{line}</Text>
            ))}
          </Box>
        ) : (
          <Box height={bodyRows}>
            {model !== null &&
              columnsOf(layout.mode, panels).map((column, c) => {
                const budget = allocate(bodyRows, column, focus);
                return (
                  <Box
                    key={c}
                    flexDirection="column"
                    width={colWidth}
                    marginRight={layout.mode === "wide" && c === 0 ? 2 : 0}
                  >
                    {column.map((id) => renderPanel(id, budget.get(id) ?? 1))}
                  </Box>
                );
              })}
          </Box>
        )}
        <Text wrap="truncate-end" bold>
          {footerTop}
        </Text>
        <Text wrap="truncate-end" dimColor>
          {truncate(
            "Tab focus  j/k move  o observe  y retry  s skip  c cancel  p pause  r poll  ? help  q quit",
            columns,
          )}
        </Text>
      </Box>
    </ThemeContext.Provider>
  );
}
