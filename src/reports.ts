/**
 * Helpers for `cstan report`: the one-line summary a worker may attach, the
 * per-agent rate limit, and the relay that makes sure every accepted report
 * reaches the PM even when none was active at the time.
 */
import type { ControllerCore } from "./controller/core.js";
import { newContext } from "./context.js";

/** One printable line (control and format characters become spaces), at most `maxBytes` UTF-8 bytes, cut where a user-perceived character ends. */
export function oneLineSummary(text: string, maxBytes: number): string {
  const clean = text
    .replace(
      /[\p{Cc}\p{Cf}\p{Cs}\p{Zl}\p{Zp}\p{Noncharacter_Code_Point}\s]+/gu,
      " ",
    )
    .trim();
  let out = "";
  let bytes = 0;
  for (const { segment } of new Intl.Segmenter(undefined, {
    granularity: "grapheme",
  }).segment(clean)) {
    const size = Buffer.byteLength(segment, "utf8");
    if (bytes + size > maxBytes) break;
    out += segment;
    bytes += size;
  }
  out = out.trim();
  if (out === "" && clean !== "") {
    // A first character longer than the limit (a letter with a flood of combining marks): keep whole code points up to the limit.
    for (const point of clean) {
      const size = Buffer.byteLength(point, "utf8");
      if (bytes + size > maxBytes) break;
      out += point;
      bytes += size;
    }
  }
  return out.trim();
}

export const REPORT_RATE_LIMIT = 10;
export const REPORT_RATE_WINDOW_MS = 60_000;

/** At most `limit` attempts per key in a sliding window (reports by default). */
export class ReportRateLimiter {
  readonly #windows = new Map<string, number[]>();
  readonly #limit: number;

  constructor(limit: number = REPORT_RATE_LIMIT) {
    this.#limit = limit;
  }

  /** Returns true and counts the attempt when the key is under the limit. */
  allow(key: string, now: number): boolean {
    if (this.#windows.size > 256) this.#sweep(now);
    const recent = (this.#windows.get(key) ?? []).filter(
      (at) => now - at < REPORT_RATE_WINDOW_MS,
    );
    if (recent.length >= this.#limit) {
      this.#windows.set(key, recent);
      return false;
    }
    recent.push(now);
    this.#windows.set(key, recent);
    return true;
  }

  /** Drops keys with no attempt left in the window, so ended agents do not accumulate. */
  #sweep(now: number): void {
    for (const [key, attempts] of this.#windows)
      if (attempts.every((at) => now - at >= REPORT_RATE_WINDOW_MS))
        this.#windows.delete(key);
  }
}

const RELAY_BACKOFF_MS = 30_000;

export interface ReportRelay {
  stop(): void;
}

/** Queues the PM notice for accepted reports and finished reviews that have none, on every tick. Safe to run more than once: the notice is queued once per report. */
export function startReportRelay(options: {
  readonly core: ControllerCore;
  readonly credential: string;
  readonly intervalMs: number;
  /** When set, open findings with no check by their Supervisor this long after the latest delivery are escalated on each tick. */
  readonly findingCheckSeconds?: number;
  readonly log: (event: string, details: Record<string, unknown>) => void;
}): ReportRelay {
  let running = false;
  let backoffUntil = 0;
  const tick = (): void => {
    if (running || Date.now() < backoffUntil) return;
    running = true;
    try {
      if (options.findingCheckSeconds !== undefined)
        for (const findingId of options.core.sweepFindings(
          () => newContext(options.core, options.credential),
          options.findingCheckSeconds,
        ))
          options.log("finding_escalated_by_deadline", { findingId });
      // Nothing to do, and nothing written, until a PM is active.
      // The core announces only when exactly one PM is active; use the same condition.
      if (
        options.core
          .listAgents()
          .filter((agent) => agent.kind === "PM" && agent.state === "active")
          .length !== 1
      )
        return;
      for (const report of options.core.unannouncedReports(
        options.credential,
      )) {
        const result = options.core.announceReport(
          newContext(options.core, options.credential),
          report.reportId,
        );
        if (!result.announced) {
          // It could not be announced just now (the PM changed, no controller actor): try again later, not every tick.
          backoffUntil = Date.now() + RELAY_BACKOFF_MS;
          return;
        }
        options.log("report_announced", { reportId: report.reportId });
      }
      for (const review of options.core.unannouncedReviews(
        options.credential,
      )) {
        const result = options.core.announceReview(
          newContext(options.core, options.credential),
          review.reviewId,
        );
        if (!result.announced) {
          backoffUntil = Date.now() + RELAY_BACKOFF_MS;
          return;
        }
        options.log("review_announced", { reviewId: review.reviewId });
      }
      for (const notice of options.core.unannouncedFindingNotices(
        options.credential,
      )) {
        const result = options.core.announceFindingNotice(
          newContext(options.core, options.credential),
          notice.noticeId,
        );
        if (!result.announced) {
          backoffUntil = Date.now() + RELAY_BACKOFF_MS;
          return;
        }
        options.log("finding_notice_announced", {
          findingId: notice.findingId,
          event: notice.event,
        });
      }
    } catch (error) {
      options.log("report_relay_failed", { error: String(error) });
    } finally {
      running = false;
    }
  };
  const timer = setInterval(tick, options.intervalMs);
  timer.unref();
  return { stop: () => clearInterval(timer) };
}
