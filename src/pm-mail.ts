/**
 * What the PM's mail looks like to the operator: how many messages wait, which
 * is the oldest and whether it has waited too long. Pure, and shared by the
 * driver, status and dash. The input type is local so that it depends on
 * nothing in the controller; a MessageRecord satisfies it.
 */

export interface PmMailMessage {
  readonly messageId: string;
  readonly state: string;
  readonly queuedAt: string;
  readonly sentAt: string | null;
  readonly ackedAt: string | null;
}

export interface PmMailSummary {
  readonly pending: number;
  readonly oldestMessageId: string | null;
  readonly oldestQueuedAt: string | null;
  readonly oldestAgeSeconds: number;
  readonly stale: boolean;
}

const PENDING_STATES: ReadonlySet<string> = new Set([
  "queued",
  "deferred",
  "sent",
  "unacked",
]);

/** Pending means queued, deferred, sent or unacked: the PM has not acknowledged it yet. The oldest is the one queued first. */
export function pmMailSummary(
  messages: readonly PmMailMessage[],
  nowMs: number,
  staleSeconds: number,
): PmMailSummary {
  let pending = 0;
  let oldest: { message: PmMailMessage; queuedMs: number } | undefined;
  for (const message of messages) {
    if (!PENDING_STATES.has(message.state)) continue;
    pending += 1;
    const queuedMs = Date.parse(message.queuedAt);
    if (
      oldest === undefined ||
      (Number.isFinite(queuedMs) && queuedMs < oldest.queuedMs) ||
      !Number.isFinite(oldest.queuedMs)
    )
      oldest = { message, queuedMs };
  }
  if (oldest === undefined)
    return {
      pending: 0,
      oldestMessageId: null,
      oldestQueuedAt: null,
      oldestAgeSeconds: 0,
      stale: false,
    };
  const ageSeconds = Number.isFinite(oldest.queuedMs)
    ? Math.max(0, Math.floor((nowMs - oldest.queuedMs) / 1000))
    : 0;
  return {
    pending,
    oldestMessageId: oldest.message.messageId,
    oldestQueuedAt: oldest.message.queuedAt,
    oldestAgeSeconds: ageSeconds,
    stale: ageSeconds >= staleSeconds,
  };
}
