export class ControllerError extends Error {
  override readonly name: string = "ControllerError";
}

export class MutationConflictError extends ControllerError {
  override readonly name: string = "MutationConflictError";
}

/** A step that waits for resume: the message says the run (or the target) is paused and gives the reason. */
export class RunPausedError extends MutationConflictError {
  override readonly name = "RunPausedError";
}

export class IdempotencyConflictError extends MutationConflictError {
  override readonly name = "IdempotencyConflictError";
}

export class StateVersionConflictError extends MutationConflictError {
  override readonly name = "StateVersionConflictError";
}

export class InputRevisionConflictError extends MutationConflictError {
  override readonly name = "InputRevisionConflictError";
}

export class TransitionAuthorizationError extends ControllerError {
  override readonly name = "TransitionAuthorizationError";
}

export class MessageTransitionError extends ControllerError {
  override readonly name = "MessageTransitionError";
  constructor(
    readonly code: string,
    message: string,
  ) {
    super(message);
  }
}

export class NoMessageTransitionDue extends Error {}

export class CandidateBindingError extends ControllerError {
  override readonly name = "CandidateBindingError";
}
