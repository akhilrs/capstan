//! The error kinds of src/controller/errors.ts, auth.ts and the ledger, each with the `name` and `message` the Node
//! implementation gives its error, so parity records can compare them.

use capstan_ledger::LedgerError;
use std::fmt;

#[derive(Debug)]
pub enum KernelError {
    /// `ControllerError`.
    Controller(String),
    /// `MutationConflictError`.
    MutationConflict(String),
    /// `RunPausedError`.
    RunPaused(String),
    /// `IdempotencyConflictError`.
    IdempotencyConflict(String),
    /// `StateVersionConflictError`.
    StateVersionConflict(String),
    /// `InputRevisionConflictError`.
    InputRevisionConflict(String),
    /// `TransitionAuthorizationError`.
    TransitionAuthorization(String),
    /// `MessageTransitionError`.
    MessageTransition {
        code: String,
        message: String,
    },
    /// `CandidateBindingError`.
    CandidateBinding(String),
    /// `AuthenticationError`.
    Authentication(String),
    /// `AuthorizationError`.
    Authorization(String),
    /// `TypeError`: a caller passed a value the controller refuses.
    Type(String),
    /// Any other `Error` thrown by the controller code.
    Other(String),
    Ledger(LedgerError),
    /// An operation whose area has not been ported yet; the text is `area.method`.
    Unported(String),
}

pub type KernelResult<T> = Result<T, KernelError>;

impl KernelError {
    pub fn controller(message: impl Into<String>) -> Self {
        Self::Controller(message.into())
    }

    pub fn type_error(message: impl Into<String>) -> Self {
        Self::Type(message.into())
    }

    pub fn conflict(message: impl Into<String>) -> Self {
        Self::MutationConflict(message.into())
    }

    /// The `name` property of the Node error.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Controller(_) => "ControllerError",
            Self::MutationConflict(_) => "MutationConflictError",
            Self::RunPaused(_) => "RunPausedError",
            Self::IdempotencyConflict(_) => "IdempotencyConflictError",
            Self::StateVersionConflict(_) => "StateVersionConflictError",
            Self::InputRevisionConflict(_) => "InputRevisionConflictError",
            Self::TransitionAuthorization(_) => "TransitionAuthorizationError",
            Self::MessageTransition { .. } => "MessageTransitionError",
            Self::CandidateBinding(_) => "CandidateBindingError",
            Self::Authentication(_) => "AuthenticationError",
            Self::Authorization(_) => "AuthorizationError",
            Self::Type(_) => "TypeError",
            Self::Other(_) => "Error",
            Self::Unported(_) => "Unported",
            Self::Ledger(e) => match e {
                LedgerError::Migration(_) => "DatabaseMigrationError",
                LedgerError::Ownership(_) => "ControllerOwnershipError",
                // `ProjectLockHeldError` extends `ControllerOwnershipError` without setting its own name.
                LedgerError::ProjectLockHeld => "ControllerOwnershipError",
                LedgerError::InvalidArgument(_) => "TypeError",
                _ => "Error",
            },
        }
    }

    /// The `message` property of the Node error.
    pub fn message(&self) -> String {
        match self {
            Self::Controller(m)
            | Self::MutationConflict(m)
            | Self::RunPaused(m)
            | Self::IdempotencyConflict(m)
            | Self::StateVersionConflict(m)
            | Self::InputRevisionConflict(m)
            | Self::TransitionAuthorization(m)
            | Self::CandidateBinding(m)
            | Self::Authentication(m)
            | Self::Authorization(m)
            | Self::Type(m)
            | Self::Other(m) => m.clone(),
            Self::MessageTransition { message, .. } => message.clone(),
            Self::Unported(m) => format!("not ported yet: {m}"),
            Self::Ledger(e) => e.to_string(),
        }
    }

    /// Whether the Node error is an instance of `MutationConflictError` (the conflict family).
    pub fn is_mutation_conflict(&self) -> bool {
        matches!(
            self,
            Self::MutationConflict(_)
                | Self::RunPaused(_)
                | Self::IdempotencyConflict(_)
                | Self::StateVersionConflict(_)
                | Self::InputRevisionConflict(_)
        )
    }

    /// Whether the Node error is an instance of `ControllerError`.
    pub fn is_controller_error(&self) -> bool {
        self.is_mutation_conflict()
            || matches!(
                self,
                Self::Controller(_)
                    | Self::TransitionAuthorization(_)
                    | Self::MessageTransition { .. }
                    | Self::CandidateBinding(_)
            )
    }

    pub fn is_unported(&self) -> bool {
        matches!(self, Self::Unported(_))
    }
}

impl fmt::Display for KernelError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.name(), self.message())
    }
}

impl std::error::Error for KernelError {}

impl From<LedgerError> for KernelError {
    fn from(e: LedgerError) -> Self {
        Self::Ledger(e)
    }
}

impl From<rusqlite::Error> for KernelError {
    fn from(e: rusqlite::Error) -> Self {
        Self::Ledger(LedgerError::Sqlite(e))
    }
}

impl From<std::io::Error> for KernelError {
    fn from(e: std::io::Error) -> Self {
        Self::Ledger(LedgerError::Io(e))
    }
}

impl From<serde_json::Error> for KernelError {
    fn from(e: serde_json::Error) -> Self {
        Self::Other(e.to_string())
    }
}
