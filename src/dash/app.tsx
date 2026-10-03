import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Box, Text, useApp, useInput, useWindowSize } from "ink";
import { signalsOf } from "../watch.js";
import {
  ACTION_KEYS,
  availableDecisions,
  observeAction,
  toWireCall,
  type DashAction,
} from "./actions.js";
import { age, clean, pushSample, truncate } from "./format.js";
import { MIN_COLUMNS, MIN_ROWS, layoutFor, type PanelId } from "./layout.js";
import {
  buildDashModel,
  EMPTY_CHANGES,
  historySample,
  pipelineItems,
  queueRows,
  trackChanges,
  visibleAgents,
  type ChangeState,
  type DashModel,
} from "./model.js";
import {
  confirmOverlay,
  helpOverlay,
  observeOverlay,
  type Overlay,
} from "./overlays.js";
import {
  abortableSleep,
  createPoller,
  stepInterval,
  type Poller,
} from "./poller.js";
import { FloatingBox, Lines } from "./screen.js";
import { ThemeContext, type Theme } from "./theme.js";
import {
  buildFrame,
  EMPTY_RINGS,
  PANEL_ORDER,
  type Link,
  type Rings,
  type ViewState,
} from "./view.js";
import { useTick } from "./ticker.js";

export const RING_LIMIT = 300;
export const NOTICE_MS = 5000;
export const DEFAULT_CONFIRM_DELAY_MS = 300;
export const SPINNER_MS = 120;
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

type Selection = Record<PanelId, { id: string | null; index: number }>;

const EMPTY_SELECTION: Selection = {
  agents: { id: null, index: 0 },
  pipeline: { id: null, index: 0 },
  queue: { id: null, index: 0 },
  findings: { id: null, index: 0 },
  work: { id: null, index: 0 },
};

/** The row ids each panel shows, in display order. */
export function rowIds(
  model: DashModel,
  problemsOnly: boolean,
  showAllEnded = false,
): Record<PanelId, readonly string[]> {
  return {
    agents: visibleAgents(model.agents, showAllEnded).map((a) => a.id),
    pipeline: pipelineItems(model).map((i) => i.id),
    queue: queueRows(model, problemsOnly).map((m) => m.id),
    findings: model.findings.map((f) => f.id),
    work: model.work.map((w) => w.id),
  };
}

/** Where the selection is now: the remembered row if it is still shown, else the old position kept inside the list. */
export function resolveSelection(
  ids: readonly string[],
  selection: { id: string | null; index: number },
): number {
  if (ids.length === 0) return 0;
  const at = selection.id === null ? -1 : ids.indexOf(selection.id);
  return at >= 0 ? at : Math.min(selection.index, ids.length - 1);
}

export function formatClock(ms: number): string {
  const d = new Date(ms);
  const two = (n: number) => String(n).padStart(2, "0");
  return `${two(d.getHours())}:${two(d.getMinutes())}:${two(d.getSeconds())}`;
}

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
  const [rings, setRings] = useState<Rings>(EMPTY_RINGS);
  const [focus, setFocus] = useState<PanelId>("queue");
  const [selection, setSelection] = useState<Selection>(EMPTY_SELECTION);
  const [problemsOnly, setProblemsOnly] = useState(false);
  const [showAllEnded, setShowAllEnded] = useState(false);
  const [intervalSeconds, setIntervalSeconds] = useState(deps.intervalSeconds);
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
  const modelRef = useRef<DashModel | null>(null);
  const shownRef = useRef<readonly PanelId[]>(PANEL_ORDER);

  // A one-second re-render keeps the clock and the ages current.
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
        let current = modelRef.current;
        if (changed || current === null) {
          const next = buildDashModel(status, now(), deps.workerLimit);
          const tracked = trackChanges(changesRef.current, next);
          changesRef.current = tracked;
          setChanges(tracked);
          modelRef.current = next;
          setModel(next);
          current = next;
          const signals = signalsOf(status);
          const previous = seenSignals.current;
          if (
            !deps.theme.reducedMotion &&
            previous !== undefined &&
            [...signals].some((key) => !previous.has(key))
          )
            write(BELL);
          seenSignals.current = signals;
        }
        const sample = historySample(current, now());
        setRings((r) => ({
          unresolved: pushSample(r.unresolved, sample.unresolved, RING_LIMIT),
          working: pushSample(r.working, sample.working, RING_LIMIT),
          oldest: pushSample(r.oldest, sample.oldestSeconds, RING_LIMIT),
        }));
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

  const ids = useMemo(
    () => (model === null ? null : rowIds(model, problemsOnly, showAllEnded)),
    [model, problemsOnly, showAllEnded],
  );
  const indexOf = (panel: PanelId): number =>
    ids === null ? 0 : resolveSelection(ids[panel], selection[panel]);

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

  // Keys can arrive faster than React re-renders, so a move starts from the
  // last selection written, not from the one the current render saw.
  const selectionRef = useRef<Selection>(EMPTY_SELECTION);
  const move = (panel: PanelId, delta: number) => {
    const list = ids?.[panel] ?? [];
    const at = resolveSelection(list, selectionRef.current[panel]);
    const index = Math.min(
      Math.max(0, list.length - 1),
      Math.max(0, at + delta),
    );
    selectionRef.current = {
      ...selectionRef.current,
      [panel]: { id: list[index] ?? null, index },
    };
    setSelection(selectionRef.current);
  };

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
    if (input === "-" || input === "+" || input === "=") {
      const next = stepInterval(intervalSeconds, input === "-");
      if (next !== intervalSeconds) {
        setIntervalSeconds(next);
        poller.current?.setIntervalMs(next * 1000);
      }
      return;
    }
    const shown = shownRef.current;
    const jump = PANEL_KEYS[input];
    if (jump !== undefined && shown.includes(jump)) return setFocus(jump);
    if (key.tab) {
      const at = shown.indexOf(focus);
      const step = key.shift ? -1 : 1;
      return setFocus(shown[(at + step + shown.length) % shown.length]!);
    }
    if (key.upArrow || key.downArrow || input === "j" || input === "k") {
      const delta = key.upArrow || input === "k" ? -1 : 1;
      return move(focus, delta);
    }
    if (input === "f" && focus === "queue") {
      return setProblemsOnly((on) => !on);
    }
    if (input === "e" && focus === "agents") {
      return setShowAllEnded((on) => !on);
    }
    if (model === null || !canAct) return;
    if (input === "o" && focus === "agents") {
      const agent = visibleAgents(model.agents, showAllEnded)[
        indexOf("agents")
      ];
      const action = agent === undefined ? undefined : observeAction(agent);
      if (agent !== undefined && action === undefined)
        return setNotice(
          `${agent.agentId} has ended; observe needs an active agent`,
        );
      if (action !== undefined) void execute(action);
      return;
    }
    if (focus === "queue") {
      const message = queueRows(model, problemsOnly)[indexOf("queue")];
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

  const size = { columns, rows };
  if (layoutFor(columns, rows).mode === "tiny")
    return (
      <ThemeContext.Provider value={deps.theme}>
        <Text>
          terminal too small (need {MIN_COLUMNS}x{MIN_ROWS}, have {columns}x
          {rows})
        </Text>
      </ThemeContext.Provider>
    );
  if (model === null || ids === null)
    return (
      <ThemeContext.Provider value={deps.theme}>
        <Text>
          {link === "down"
            ? "controller not answering, retrying"
            : "connecting to the controller..."}
        </Text>
      </ThemeContext.Provider>
    );

  const view: ViewState = {
    size,
    focus,
    selected: {
      agents: indexOf("agents"),
      pipeline: indexOf("pipeline"),
      queue: indexOf("queue"),
      findings: indexOf("findings"),
      work: indexOf("work"),
    },
    problemsOnly,
    showAllEnded,
    paused,
    link,
    linkAge:
      lastOk.current === null
        ? "-"
        : age(new Date(lastOk.current).toISOString(), nowMs),
    clock: formatClock(nowMs),
    intervalSeconds,
    nowMs,
    tick,
    rings,
    highlight: changes.highlight,
    notice: busy && notice === null ? "working..." : notice,
  };
  const frame = buildFrame(model, view, deps.theme);
  shownRef.current = frame.shown;
  const overlay: Overlay | null =
    peek !== null
      ? observeOverlay(peek, size, deps.theme)
      : prompt !== null
        ? confirmOverlay(prompt.action, size, deps.theme)
        : help
          ? helpOverlay(size, deps.theme)
          : null;

  return (
    <ThemeContext.Provider value={deps.theme}>
      <Box flexDirection="column" width={columns} height={rows}>
        <Lines lines={frame.lines} />
        {overlay !== null && <FloatingBox overlay={overlay} />}
      </Box>
    </ThemeContext.Provider>
  );
}
