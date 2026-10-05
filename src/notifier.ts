/**
 * Tells the operator that something needs them: a Herdr notification and a
 * passive fallback the `cstan status --watch` pane shows with a bell. Every
 * channel attempt leaves one line in notifications.jsonl, so a failed channel
 * is never silent.
 */
import fs from "node:fs";
import { openDaemonLog } from "./client.js";

export const NOTIFICATION_LOG_MAX_BYTES = 5 * 1024 * 1024;

export type NotificationKind =
  "pm_message" | "input_cleared" | "delivery_stuck" | "pm_stale";
export type NotificationChannel = "herdr" | "fallback";

export interface NotifierAdapter {
  notify(title: string, body: string): Promise<void>;
}

export interface NotificationRequest {
  readonly kind: NotificationKind;
  readonly messageId: string;
  readonly recipientAgentId: string;
  readonly repeat: boolean;
  /** Characters cleared (`input_cleared`) or the stuck reason (`delivery_stuck`). */
  readonly detail?: string;
}

export type ChannelFailureReason =
  "not_shown" | "command_failed" | "invalid_text" | "timeout";

export interface ChannelResult {
  readonly channel: NotificationChannel;
  readonly ok: boolean;
  /** Why a failed channel failed. */
  readonly reason?: ChannelFailureReason;
  /** The sanitized error message of a failed channel, at most NOTIFICATION_ERROR_MAX_CHARS characters. */
  readonly error?: string;
}

export const NOTIFICATION_ERROR_MAX_CHARS = 200;

/** Names the failure from the error's class and code; a Herdr answer of shown:false carries the code notification_not_shown. */
function failureReason(error: unknown): ChannelFailureReason {
  const code = (error as { code?: unknown } | null)?.code;
  if (code === "notification_not_shown") return "not_shown";
  if (code === "timeout") return "timeout";
  if (error instanceof Error && error.name === "InvalidArgumentError")
    return "invalid_text";
  return "command_failed";
}

function sanitizedError(error: unknown): string {
  const text = error instanceof Error ? error.message : String(error);
  return text
    .replace(/[\u0000-\u001f\u007f-\u009f]+/g, " ")
    .replace(/\s+/g, " ")
    .trim()
    .slice(0, NOTIFICATION_ERROR_MAX_CHARS);
}

export interface Notifier {
  /** Attempts every enabled channel; never throws. */
  send(request: NotificationRequest): Promise<readonly ChannelResult[]>;
  /** Appends one line per channel; never throws. `recorded` says the ledger holds a record of the event. */
  write(
    request: NotificationRequest,
    results: readonly ChannelResult[],
    recorded: boolean,
  ): void;
}

export interface NotifierOptions {
  readonly adapter: NotifierAdapter;
  readonly channels: { readonly herdr: boolean; readonly fallback: boolean };
  readonly recordPath: string;
  readonly now?: () => Date;
  readonly log?: (event: string, details: Record<string, unknown>) => void;
}

const TITLES: Readonly<Record<NotificationKind, string>> = {
  pm_message: "Capstan: PM message waiting",
  input_cleared: "Capstan: input line cleared",
  delivery_stuck: "Capstan: delivery stuck",
  pm_stale: "Capstan: PM mail is stale",
};

function bodyOf(request: NotificationRequest): string {
  switch (request.kind) {
    case "pm_message":
      return `Message ${request.messageId} for the PM is waiting${request.repeat ? " (reminder)" : ""}`;
    case "input_cleared":
      return `Cleared ${request.detail ?? "some"} characters typed in ${request.recipientAgentId}'s input line before message ${request.messageId}`;
    case "pm_stale":
      return `${request.detail ?? "A message"} for the PM has waited too long (oldest: ${request.messageId})`;
    case "delivery_stuck":
      return `Message ${request.messageId} for ${request.recipientAgentId} is stuck: ${request.detail ?? "unknown reason"}`;
  }
}

export function createNotifier(options: NotifierOptions): Notifier {
  const now = options.now ?? (() => new Date());
  const log = options.log ?? (() => undefined);
  return {
    async send(request) {
      const results: ChannelResult[] = [];
      if (options.channels.herdr) {
        try {
          await options.adapter.notify(TITLES[request.kind], bodyOf(request));
          results.push({ channel: "herdr", ok: true });
        } catch (error) {
          const reason = failureReason(error);
          const message = sanitizedError(error);
          log("notification_channel_failed", {
            channel: "herdr",
            messageId: request.messageId,
            reason,
            error: error instanceof Error ? error.name : "error",
          });
          results.push({
            channel: "herdr",
            ok: false,
            reason,
            ...(message === "" ? {} : { error: message }),
          });
        }
      }
      // Passive: the ledger record is what the watch pane reads.
      if (options.channels.fallback)
        results.push({ channel: "fallback", ok: true });
      return results;
    },

    write(request, results, recorded) {
      try {
        try {
          if (fs.statSync(options.recordPath).size > NOTIFICATION_LOG_MAX_BYTES)
            fs.renameSync(options.recordPath, `${options.recordPath}.1`);
        } catch (error) {
          if (!(
            error instanceof Error &&
            "code" in error &&
            error.code === "ENOENT"
          ))
            throw error;
        }
        const fd = openDaemonLog(options.recordPath);
        try {
          for (const result of results)
            fs.writeSync(
              fd,
              `${JSON.stringify({
                ts: now().toISOString(),
                messageId: request.messageId,
                channel: result.channel,
                ok: result.ok,
                ...(result.reason === undefined
                  ? {}
                  : { reason: result.reason }),
                ...(result.error === undefined ? {} : { error: result.error }),
                kind: request.kind,
                repeat: request.repeat,
                recorded,
              })}\n`,
            );
        } finally {
          fs.closeSync(fd);
        }
      } catch (error) {
        log("notification_record_failed", {
          messageId: request.messageId,
          error: error instanceof Error ? error.name : "error",
        });
      }
    },
  };
}
