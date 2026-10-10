//! Host-driven task lifecycle. Provider content can never advance a task to applied.
//! Page accounting, attempts, timeouts and cancellation of in-flight work belong to P2.

use deckcraft_engine::ai_contract::{ContractError, SnapshotIdentity};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskState {
    Planning,
    AwaitingOutline,
    AwaitingTemplate,
    Generating,
    Validating,
    Ready,
    Applying,
    Applied,
    Cancelled,
    Failed,
    Conflicted,
}

impl TaskState {
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Applied | Self::Cancelled | Self::Failed | Self::Conflicted)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureReason {
    Provider,
    InvalidResponse,
    Validation,
    Deadline,
    Budget,
    Commit,
}

/// Host-only control events. A real provider adapter must only parse page/patch responses,
/// never deserialize model output as TaskEvent. ValidationPassed means *all* target pages
/// passed deterministic validation. Applied is emitted only after engine commit succeeds.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case", deny_unknown_fields)]
pub enum TaskAction {
    OutlineAvailable {},
    OutlineConfirmed {},
    TemplateConfirmed {},
    GenerationFinished {},
    ValidationPassed {},
    RetryPages {},
    ApplyRequested {},
    Applied {},
    Conflict {},
    Cancel {},
    DocumentClosed {},
    Fail { reason: FailureReason },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskEvent {
    pub snapshot: SnapshotIdentity,
    pub action: TaskAction,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EventOutcome {
    Transitioned { from: TaskState, to: TaskState },
    IgnoredTerminal,
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum TaskError {
    #[error(transparent)]
    Contract(#[from] ContractError),
    #[error("event does not match the frozen task identity")]
    SnapshotMismatch,
    #[error("event is not valid in state {0:?}")]
    InvalidTransition(TaskState),
}

/// Neither the frozen identity nor current state can be mutated by consumers.
#[derive(Clone, Debug)]
pub struct Task {
    snapshot: SnapshotIdentity,
    state: TaskState,
    failure: Option<FailureReason>,
}

impl Task {
    pub fn text_edit(snapshot: SnapshotIdentity) -> Result<Self, TaskError> {
        Self::new(snapshot, TaskState::Generating)
    }

    pub fn outline_generation(snapshot: SnapshotIdentity) -> Result<Self, TaskError> {
        Self::new(snapshot, TaskState::Planning)
    }

    fn new(snapshot: SnapshotIdentity, state: TaskState) -> Result<Self, TaskError> {
        snapshot.validate()?;
        Ok(Self { snapshot, state, failure: None })
    }

    pub fn snapshot(&self) -> &SnapshotIdentity {
        &self.snapshot
    }

    pub fn state(&self) -> TaskState {
        self.state
    }

    pub fn failure(&self) -> Option<FailureReason> {
        self.failure
    }

    /// Invalid/mismatched events do not mutate state. Late events on a terminal task are
    /// discarded. A retry after failed/conflicted requires a new task/snapshot, not resurrection.
    /// Cancel and apply must be serialized by the host: once synchronous commit completes the
    /// host must emit Applied before consuming any queued cancellation.
    pub fn accept(&mut self, event: &TaskEvent) -> Result<EventOutcome, TaskError> {
        if event.snapshot != self.snapshot {
            return Err(TaskError::SnapshotMismatch);
        }
        if self.state.is_terminal() {
            return Ok(EventOutcome::IgnoredTerminal);
        }
        let to = match (&event.action, self.state) {
            (TaskAction::OutlineAvailable {}, TaskState::Planning) => TaskState::AwaitingOutline,
            (TaskAction::OutlineConfirmed {}, TaskState::AwaitingOutline) => TaskState::AwaitingTemplate,
            (TaskAction::TemplateConfirmed {}, TaskState::AwaitingTemplate) => TaskState::Generating,
            (TaskAction::GenerationFinished {}, TaskState::Generating) => TaskState::Validating,
            (TaskAction::ValidationPassed {}, TaskState::Validating) => TaskState::Ready,
            (TaskAction::RetryPages {}, TaskState::Validating) => TaskState::Generating,
            (TaskAction::ApplyRequested {}, TaskState::Ready) => TaskState::Applying,
            (TaskAction::Applied {}, TaskState::Applying) => TaskState::Applied,
            (TaskAction::Conflict {}, TaskState::Ready | TaskState::Applying) => TaskState::Conflicted,
            (TaskAction::Cancel {} | TaskAction::DocumentClosed {}, _) => TaskState::Cancelled,
            (TaskAction::Fail { .. }, _) => TaskState::Failed,
            _ => return Err(TaskError::InvalidTransition(self.state)),
        };
        let from = self.state;
        self.state = to;
        if let TaskAction::Fail { reason } = event.action {
            self.failure = Some(reason);
        }
        Ok(EventOutcome::Transitioned { from, to })
    }
}
