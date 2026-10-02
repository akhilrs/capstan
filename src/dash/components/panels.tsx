import { Box, Text } from "ink";
import { age, fit, sparkline, truncate } from "../format.js";
import type {
  AgentRow,
  DashModel,
  FindingRow,
  MessageRow,
  PipelineItem,
  PipelineStage,
  WorkRow,
} from "../model.js";
import { cellsFor, windowOf, type Layout, type PanelId } from "../layout.js";
import { colorOfState, useTheme, type ColorName } from "../theme.js";
import { Spinner } from "./widgets.js";

export interface PanelProps {
  readonly capacity: number;
  readonly width: number;
  readonly focused: boolean;
  readonly cursor: number;
  readonly highlight: ReadonlyMap<string, number>;
  readonly nowMs: number;
  readonly layout: Layout;
  readonly tick: number;
}

const TITLES: Record<PanelId, string> = {
  agents: "Agents",
  pipeline: "Report pipeline",
  queue: "Message queue",
  findings: "Supervisor findings",
  work: "Work items (v1)",
};
const NUMBERS: Record<PanelId, number> = {
  agents: 1,
  pipeline: 2,
  queue: 3,
  findings: 4,
  work: 5,
};

function Frame({
  id,
  props,
  hidden,
  note,
  children,
}: {
  id: PanelId;
  props: PanelProps;
  hidden: number;
  note?: string;
  children: React.ReactNode;
}) {
  const theme = useTheme();
  const title = `${NUMBERS[id]} ${TITLES[id]}${note ? ` ${note}` : ""}${hidden > 0 ? `  +${hidden} more` : ""}`;
  return (
    <Box flexDirection="column" width={props.width}>
      <Text
        bold={props.focused}
        underline={props.focused}
        {...theme.paint(props.focused ? "cyan" : "gray")}
      >
        {truncate(`${props.focused ? ">" : " "}${title}`, props.width)}
      </Text>
      {children}
    </Box>
  );
}

function Row({
  selected,
  changed,
  color,
  width,
  children,
}: {
  selected: boolean;
  changed: boolean;
  color?: ColorName | undefined;
  width: number;
  children: string;
}) {
  const theme = useTheme();
  const marker = selected ? ">" : changed ? "+" : " ";
  return (
    <Text
      wrap="truncate-end"
      inverse={changed && !theme.reducedMotion}
      bold={selected}
      {...(color === undefined ? {} : theme.paint(color))}
    >
      {truncate(`${marker}${children}`, width)}
    </Text>
  );
}

function dim(text: string, width: number) {
  return <DimLine text={text} width={width} />;
}
function DimLine({ text, width }: { text: string; width: number }) {
  const theme = useTheme();
  return (
    <Text {...theme.paint("gray")} wrap="truncate-end">
      {truncate(text, width)}
    </Text>
  );
}

export function AgentsPanel(
  props: PanelProps & { agents: readonly AgentRow[]; ended: number },
) {
  const { agents } = props;
  const win = windowOf(agents.length, props.cursor, props.capacity);
  const w = props.width;
  return (
    <Frame
      id="agents"
      props={props}
      hidden={win.hidden}
      note="(working is inferred)"
    >
      {agents.length === 0 && dim("no agents", w)}
      {agents.slice(win.start, win.end).map((a, i) => {
        const flags = [
          a.stalled ? "STALLED" : "",
          a.lost ? "LOST" : "",
          a.blocked ? "BLOCKED" : "",
        ]
          .filter(Boolean)
          .join(" ");
        const cells = cellsFor(w);
        const text = `${fit(a.agentId, 15)} ${cells.showRole ? `${fit(a.roleName, 10)} ` : ""}${fit(a.kind, 10)} g${fit(String(a.generation), 2)} ${fit(a.state, 6)} ${fit(flags, 8)}${cells.showAge ? ` ${fit(age(a.lastActivityAt, props.nowMs), 4)}` : ""}${cells.showPane ? ` ${fit(a.paneId ?? "-", 8)}` : ""} q${a.queueDepth}`;
        const selected = props.focused && win.start + i === props.cursor;
        return (
          <Box key={a.id}>
            <Row
              selected={selected}
              changed={props.highlight.has(`a:${a.id}`)}
              color={
                a.stalled || a.lost || a.blocked
                  ? "red"
                  : a.state === "active"
                    ? undefined
                    : "gray"
              }
              width={w - 2}
            >
              {text}
            </Row>
            {a.working ? <Spinner tick={props.tick} /> : <Text> </Text>}
          </Box>
        );
      })}
      {props.ended > 0 && dim(`+${props.ended} more ended agents`, w)}
    </Frame>
  );
}

function StageLine({
  name,
  stage,
  width,
}: {
  name: string;
  stage: PipelineStage;
  width: number;
}) {
  const parts = Object.entries(stage.counts)
    .map(([state, n]) => `${state} ${n}`)
    .join(", ");
  return (
    <DimLine
      text={`${name}: ${stage.total}${parts ? ` (${parts})` : ""}`}
      width={width}
    />
  );
}

export function PipelinePanel(
  props: PanelProps & { pipeline: DashModel["pipeline"] },
) {
  const { pipeline } = props;
  const items: PipelineItem[] = [
    ...pipeline.reports.items,
    ...pipeline.reviews.items,
    ...pipeline.integrations.items,
  ].sort((a, b) => b.createdAt.localeCompare(a.createdAt));
  const body = Math.max(0, props.capacity - (props.capacity > 3 ? 4 : 1));
  const win = windowOf(items.length, props.cursor, body);
  const w = props.width;
  return (
    <Frame id="pipeline" props={props} hidden={win.hidden}>
      <DimLine
        width={w}
        text={`reported ${pipeline.reports.total} > review ${pipeline.reviews.total} > integrated ${pipeline.integrations.total}`}
      />
      {props.capacity > 3 && (
        <>
          <StageLine name="reports" stage={pipeline.reports} width={w} />
          <StageLine name="reviews" stage={pipeline.reviews} width={w} />
          <StageLine
            name="integrations"
            stage={pipeline.integrations}
            width={w}
          />
        </>
      )}
      {items.slice(win.start, win.end).map((item, i) => (
        <Row
          key={item.id}
          selected={props.focused && win.start + i === props.cursor}
          changed={props.highlight.has(`p:${item.id}`)}
          color={colorOfState(item.state)}
          width={w}
        >
          {`${fit(item.stage, 11)} ${fit(item.state, 10)} ${item.label}${cellsFor(props.width).showAge ? ` ${age(item.createdAt, props.nowMs)}` : ""}`}
        </Row>
      ))}
    </Frame>
  );
}

export function QueuePanel(
  props: PanelProps & { queue: DashModel["queue"]; samples: readonly number[] },
) {
  const { queue } = props;
  const win = windowOf(queue.messages.length, props.cursor, props.capacity);
  const w = props.width;
  const note =
    queue.inputClearCount > 0 ? `(input clears: ${queue.inputClearCount})` : "";
  return (
    <Frame id="queue" props={props} hidden={win.hidden} note={note}>
      {queue.messages.length === 0 && dim("no unresolved messages", w)}
      {queue.messages.slice(win.start, win.end).map((m: MessageRow, i) => (
        <Row
          key={m.id}
          selected={props.focused && win.start + i === props.cursor}
          changed={props.highlight.has(`m:${m.id}`)}
          color={m.problem !== null ? "red" : colorOfState(m.state)}
          width={w}
        >
          {`#${fit(String(m.sequence), 4)} ${fit(m.recipientAgentId, 12)} ${fit(m.state, 9)}${cellsFor(props.width).showAge ? ` ${fit(age(m.queuedAt, props.nowMs), 5)}` : ""}${cellsFor(props.width).showNotified ? ` ${m.notified ? "notified" : "         "}` : ""} ${m.problem !== null ? `PROBLEM ${m.problem}` : (m.deferredReason ?? m.stateReason ?? "")}`}
        </Row>
      ))}
      {queue.truncated && dim("+more messages not shown", w)}
      {props.layout.sparklines && (
        <DimLine
          width={w}
          text={`unresolved ${sparkline(props.samples, 30)} since dash start`}
        />
      )}
      {queue.cleanupFailed.map((c) => (
        <DimLine key={c} width={w} text={`cleanup failed: ${c}`} />
      ))}
      {queue.orphanPanes.map((o) => (
        <DimLine key={o} width={w} text={`orphan pane: ${o}`} />
      ))}
    </Frame>
  );
}

export function FindingsPanel(
  props: PanelProps & { findings: readonly FindingRow[] },
) {
  const win = windowOf(props.findings.length, props.cursor, props.capacity);
  const w = props.width;
  return (
    <Frame id="findings" props={props} hidden={win.hidden}>
      {props.findings.length === 0 && dim("no open findings", w)}
      {props.findings.slice(win.start, win.end).map((f, i) => (
        <Row
          key={f.id}
          selected={props.focused && win.start + i === props.cursor}
          changed={props.highlight.has(`f:${f.id}`)}
          color={f.needsOperator ? "red" : "yellow"}
          width={w}
        >
          {`${fit(f.findingId.slice(0, 6), 6)} ${fit(f.targetAgentId, 11)} ${fit(f.severity, 8)} ${fit(f.state, 9)} ${f.interventions}/2${f.needsOperator ? " NEEDS OPERATOR" : ""}${f.stateReason ? ` ${f.stateReason}` : ""}`}
        </Row>
      ))}
    </Frame>
  );
}

export function WorkPanel(props: PanelProps & { work: readonly WorkRow[] }) {
  const win = windowOf(props.work.length, props.cursor, props.capacity);
  return (
    <Frame id="work" props={props} hidden={win.hidden}>
      {props.work.slice(win.start, win.end).map((r, i) => (
        <Row
          key={r.id}
          selected={props.focused && win.start + i === props.cursor}
          changed={props.highlight.has(`w:${r.id}`)}
          width={props.width}
        >
          {`${fit(r.workItemId, 14)} ${fit(r.state, 20)} ${r.title}`}
        </Row>
      ))}
    </Frame>
  );
}

export function Header({
  model,
  link,
  ageText,
  intervalSeconds,
  paused,
  width,
}: {
  model: DashModel;
  link: "ok" | "down" | "toolarge" | "starting";
  ageText: string;
  intervalSeconds: number;
  paused: boolean;
  width: number;
}) {
  const theme = useTheme();
  const h = model.header;
  const healthColor: ColorName =
    h.health === "degraded"
      ? "red"
      : h.health === "evaluating"
        ? "yellow"
        : h.health === "healthy"
          ? "green"
          : "gray";
  const linkText =
    link === "ok"
      ? "answering"
      : link === "starting"
        ? "connecting"
        : link === "toolarge"
          ? "status too large"
          : "controller not answering";
  return (
    <Box flexDirection="column" width={width}>
      <Text wrap="truncate-end">
        <Text bold>cstan dash</Text> {h.projectId} run:{h.runState} supervision:
        {h.supervisionEnabled ? "on" : "off"}{" "}
        <Text {...theme.paint(healthColor)} bold>
          {h.health.toUpperCase()}
        </Text>
        {h.healthReason ? ` (${h.healthReason})` : ""} workers {h.workers}
        {h.workerLimit === null ? "" : `/${h.workerLimit}`}
      </Text>
      <Text
        wrap="truncate-end"
        {...theme.paint(link === "ok" ? "gray" : "red")}
      >
        {link === "ok" ? "●" : "○"} {linkText}
        {paused ? " PAUSED" : ""} updated {ageText} ago every {intervalSeconds}s
        {h.checkpointEpoch !== null || h.targetEpoch !== null
          ? ` epoch ${h.checkpointEpoch ?? "-"}/${h.targetEpoch ?? "-"}`
          : ""}
        {h.replacementAttempts > 0
          ? ` replacements ${h.replacementAttempts}`
          : ""}
      </Text>
    </Box>
  );
}
