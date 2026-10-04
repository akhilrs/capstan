/** The adapter's error classes; each is defined once here, so `instanceof` holds across modules. */
import type { InputBlocker } from "./screen.js";

export class AdapterError extends Error {
  override readonly name: string = "AdapterError";
}
/** A pane move failed and the pane cannot be found at its old id or at a new one. */
export class PaneLost extends AdapterError {
  override readonly name = "PaneLost";
}
export class UnknownPaneError extends AdapterError {
  override readonly name = "UnknownPaneError";
}
export class PmPaneError extends AdapterError {
  override readonly name = "PmPaneError";
}
export class PhaseError extends AdapterError {
  override readonly name = "PhaseError";
}
export class PaneGone extends AdapterError {
  override readonly name = "PaneGone";
}
export class AgentPaneMismatch extends AdapterError {
  override readonly name = "AgentPaneMismatch";
}
export class DeferralNotElapsed extends AdapterError {
  override readonly name = "DeferralNotElapsed";
}
export class NotIdle extends AdapterError {
  override readonly name = "NotIdle";
}
export class NotBlocked extends AdapterError {
  override readonly name = "NotBlocked";
}
export class InputUnreadable extends AdapterError {
  override readonly name = "InputUnreadable";
  /** What keeps the input line from being read, from the screen that was read. */
  readonly blocker: InputBlocker;
  constructor(
    message: string,
    blocker: InputBlocker = "unknown",
    options?: { cause?: unknown },
  ) {
    super(message, options);
    this.blocker = blocker;
  }
}
export class ClearFailed extends AdapterError {
  override readonly name = "ClearFailed";
}
export class PromptUnrecognized extends AdapterError {
  override readonly name = "PromptUnrecognized";
}
export class ShellNotReady extends AdapterError {
  override readonly name = "ShellNotReady";
}
export class DialogStillOpen extends AdapterError {
  override readonly name = "DialogStillOpen";
}
export class UnsupportedHostError extends AdapterError {
  override readonly name = "UnsupportedHostError";
}
export class SendAfterRecordError extends AdapterError {
  override readonly name = "SendAfterRecordError";
}
export class InvalidArgumentError extends AdapterError {
  override readonly name = "InvalidArgumentError";
}
