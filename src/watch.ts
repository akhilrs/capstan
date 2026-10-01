/** `cstan status --watch`: a compact status block that rings the bell when the operator has something new to see. */

export interface WatchDeps {
  fetch(): Promise<Record<string, unknown>>;
  write(text: string): void;
  sleep(ms: number): Promise<void>;
  /** Stops after this many polls; unlimited when omitted. */
  iterations?: number;
  intervalMs: number;
}

interface WatchedMessage {
  readonly messageId: string;
  readonly recipientAgentId: string;
  readonly state: string;
  readonly lastNotifiedAt: string | null;
}

interface WatchedFinding {
  readonly findingId: string;
  readonly targetAgentId: string;
  readonly severity: string;
  readonly state: string;
  readonly interventions: number;
  readonly stateReason: string | null;
}

const BELL = "\u0007";

/** Everything from the daemon is shown without control or format characters. */
function clean(value: unknown): string {
  if (value === undefined || value === null) return "";
  return String(value).replace(/[\p{Cc}\p{Cf}\p{Zl}\p{Zp}]/gu, " ");
}

function list<T>(value: unknown): T[] {
  return Array.isArray(value) ? (value as T[]) : [];
}

/** Keys of everything that should ring once: a new or changed notification, a new clear, a new stuck entry. */
export function signalsOf(status: Record<string, unknown>): Set<string> {
  const signals = new Set<string>();
  for (const message of list<WatchedMessage>(status.messages))
    if (message.lastNotifiedAt != null)
      signals.add(`notified:${message.messageId}:${message.lastNotifiedAt}`);
  for (const clear of list<{ clearId: string }>(status.inputClears))
    signals.add(`clear:${clear.clearId}`);
  for (const stuck of list<{ messageId: string; reason: string }>(status.stuck))
    signals.add(`stuck:${stuck.messageId}:${stuck.reason}`);
  for (const id of list<string>(status.lostAgentIds)) signals.add(`lost:${id}`);
  for (const finding of list<WatchedFinding>(status.agentFindings))
    if (finding.state === "escalated")
      signals.add(`finding-escalated:${finding.findingId}`);
  return signals;
}

export function renderWatch(status: Record<string, unknown>): string {
  const lines: string[] = [];
  const agents = list<{ agentId: string; kind: string; state: string }>(
    status.agents,
  );
  lines.push(
    `agents: ${agents.map((a) => `${clean(a.agentId)} (${clean(a.kind)}, ${clean(a.state)})`).join(", ") || "none"}`,
  );
  const messages = list<WatchedMessage>(status.messages);
  if (messages.length === 0) lines.push("messages: none unresolved");
  if (status.messagesTruncated === true)
    lines.push(
      `(only the first ${messages.length} unresolved messages are shown)`,
    );
  const clears = list<unknown>(status.inputClears).length;
  if (clears > 0) lines.push(`input clears recorded: ${clears}`);
  for (const m of messages)
    lines.push(
      `message ${clean(m.messageId)} -> ${clean(m.recipientAgentId)} [${clean(m.state)}] notified: ${clean(m.lastNotifiedAt ?? "no")}`,
    );
  for (const s of list<{ messageId: string; reason: string }>(status.stuck))
    lines.push(`stuck ${clean(s.messageId)}: ${clean(s.reason)}`);
  for (const f of list<WatchedFinding>(status.agentFindings))
    if (f.state === "open" || f.state === "escalated")
      lines.push(
        `finding ${clean(f.findingId)} on ${clean(f.targetAgentId)} (${clean(f.severity)}) [${clean(f.state)}, intervention ${clean(f.interventions)} of 2${f.stateReason == null ? "" : `, ${clean(f.stateReason)}`}]${f.state === "escalated" ? " needs the operator" : ""}`,
      );
  const lost = list<string>(status.lostAgentIds);
  if (lost.length > 0) lines.push(`lost: ${lost.map(clean).join(", ")}`);
  const stalled = list<string>(status.stalledAgentIds);
  if (stalled.length > 0)
    lines.push(`stalled: ${stalled.map(clean).join(", ")}`);
  return lines.join("\n");
}

/** Polls until the daemon stops answering or the iterations run out; the first poll is a baseline and never rings. */
export async function watchStatus(deps: WatchDeps): Promise<void> {
  let seen: Set<string> | undefined;
  for (
    let poll = 0;
    deps.iterations === undefined || poll < deps.iterations;
    poll += 1
  ) {
    if (poll > 0) await deps.sleep(deps.intervalMs);
    let status: Record<string, unknown>;
    try {
      status = await deps.fetch();
    } catch {
      deps.write("the controller stopped answering\n");
      return;
    }
    const current = signalsOf(status);
    const ring =
      seen !== undefined && [...current].some((key) => !seen!.has(key));
    seen = current;
    deps.write(`${ring ? BELL : ""}${renderWatch(status)}\n---\n`);
  }
}
