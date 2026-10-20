//! Data-only boundary between the application and infrastructure.
use std::{fmt, num::NonZeroU64, str::FromStr};

use ku_core::{Board, SearchResult, TraceEvent, TraceSnapshot};

/// An invalid identity, activation, or completion envelope.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ProtocolError {
    /// A numeric identity was zero.
    #[error("identities must be nonzero")]
    ZeroIdentity,
    /// A solver identity was empty or contained whitespace.
    #[error("solver identity must be nonempty and contain no whitespace")]
    SolverIdentity,
    /// An activation contained no jobs.
    #[error("an activation needs at least one job")]
    EmptyActivation,
    /// Command identities were repeated or not strictly increasing.
    #[error("commands must have strictly increasing identities")]
    CommandOrder,
    /// Activation identity or logical time violated session ordering.
    #[error("activation identity or logical time is out of order")]
    ActivationOrder,
    /// An identified timer fired before its deadline.
    #[error("timer fired before its scheduled deadline")]
    EarlyTimer,
    /// Failure feedback referenced a command the model had not emitted.
    #[error("feedback references a command that has not been emitted")]
    CommandReference,
    /// An identity or logical time increment overflowed.
    #[error("identity space exhausted")]
    Exhausted,
}
macro_rules! identity {
    ($name:ident, $doc:literal) => {
        #[doc = $doc]
        #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(NonZeroU64);
        impl $name {
            /// The first identity, with numeric value one.
            pub const FIRST: Self = Self(NonZeroU64::MIN);
            /// Returns the nonzero numeric identity.
            pub const fn get(self) -> u64 {
                self.0.get()
            }
            /// Returns the next identity.
            ///
            /// # Errors
            /// Returns [`ProtocolError::Exhausted`] at the largest representable identity.
            pub fn checked_next(self) -> Result<Self, ProtocolError> {
                self.get()
                    .checked_add(1)
                    .ok_or(ProtocolError::Exhausted)?
                    .try_into()
            }
        }
        impl TryFrom<u64> for $name {
            type Error = ProtocolError;
            fn try_from(value: u64) -> Result<Self, Self::Error> {
                NonZeroU64::new(value)
                    .map(Self)
                    .ok_or(ProtocolError::ZeroIdentity)
            }
        }
        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.get().fmt(f)
            }
        }
    };
}
identity!(
    ActivationId,
    "Monotonic activation identity within a session."
);
identity!(CommandId, "Monotonic command identity within a session.");
identity!(
    RunId,
    "Solver run identity, independent of command identity."
);

/// Milliseconds from a session's logical origin; never wall clock time.
///
/// # Examples
///
/// ```
/// use ku_session::protocol::{ActivationId, LogicalTime, ProtocolError};
///
/// let second = ActivationId::FIRST.checked_next()?;
/// assert_eq!(second.get(), 2);
/// assert_eq!(ActivationId::try_from(0), Err(ProtocolError::ZeroIdentity));
/// let deadline = LogicalTime::ZERO.checked_add(50)?;
/// assert_eq!(deadline.as_millis(), 50);
/// assert_eq!(LogicalTime::from_millis(u64::MAX).checked_add(1),
///     Err(ProtocolError::Exhausted));
/// # Ok::<(), ProtocolError>(())
/// ```
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LogicalTime(u64);
impl LogicalTime {
    /// The logical origin of a session.
    pub const ZERO: Self = Self(0);
    /// Constructs an absolute logical time in milliseconds.
    pub const fn from_millis(value: u64) -> Self {
        Self(value)
    }
    /// Returns milliseconds since the logical origin.
    pub const fn as_millis(self) -> u64 {
        self.0
    }
    /// Adds a duration in milliseconds.
    ///
    /// # Errors
    /// Returns [`ProtocolError::Exhausted`] on overflow.
    pub fn checked_add(self, milliseconds: u64) -> Result<Self, ProtocolError> {
        self.0
            .checked_add(milliseconds)
            .map(Self)
            .ok_or(ProtocolError::Exhausted)
    }
}

/// Stable strategy identity, never a function pointer or process address.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SolverId(String);
impl SolverId {
    /// Borrows the validated solver identity.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
impl TryFrom<String> for SolverId {
    type Error = ProtocolError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        if value.is_empty() || value.chars().any(char::is_whitespace) {
            Err(ProtocolError::SolverIdentity)
        } else {
            Ok(Self(value))
        }
    }
}
impl FromStr for SolverId {
    type Err = ProtocolError;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        value.to_owned().try_into()
    }
}
impl fmt::Display for SolverId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// Portable failure data delivered to the deterministic model.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FailureKind {
    /// The solver could not be started.
    Start,
    /// The solver or command failed during execution.
    Execution,
    /// The event producer closed without a valid completion.
    ProducerClosed,
    /// Producer feedback violated the session or trace protocol.
    Protocol,
}
/// A categorized infrastructure failure with a human-readable diagnostic.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Failure {
    /// The stage or protocol condition that failed.
    pub kind: FailureKind,
    /// A diagnostic suitable for display and portable history.
    pub message: String,
}
/// Why the runtime closed the session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionEndReason {
    /// The user requested exit.
    Quit,
    /// The input source reached its end.
    InputEnded,
    /// A runtime operation or hook failed.
    Error,
    /// The session driver was dropped before explicit closure.
    Dropped,
}

/// Semantic feedback. Pending transport polls never become jobs.
#[derive(Clone, Debug, PartialEq, Eq)]
#[expect(
    clippy::large_enum_variant,
    reason = "inline trace data avoids an allocation per activation"
)]
pub enum Job {
    /// A semantic user action or transport compatibility signal.
    Action(crate::action::Action),
    /// Infrastructure could not execute a semantic command, including cleanup.
    CommandFailed {
        /// The emitted command that failed.
        command: CommandId,
        /// The categorized failure and diagnostic.
        failure: Failure,
    },
    /// An identified timer reached its deadline.
    TimerFired {
        /// The timer command being acknowledged.
        command: CommandId,
    },
    /// Infrastructure accepted a solver start command.
    SolverStarted {
        /// The solver run that started.
        run: RunId,
        /// The start command being acknowledged.
        command: CommandId,
    },
    /// An ordered batch of actual trace events from a solver run.
    SolverEvents {
        /// The producing solver run.
        run: RunId,
        /// The start command that owns this run.
        command: CommandId,
        /// Events in producer order, without transport polling artifacts.
        events: Vec<TraceEvent>,
    },
    /// A solver finished with a result and its final observable state.
    SolverCompleted {
        /// The completed solver run.
        run: RunId,
        /// The start command that owns this run.
        command: CommandId,
        /// The completed search result.
        result: SearchResult,
        /// The final search snapshot associated with the result.
        snapshot: TraceSnapshot,
    },
    /// A solver run stopped with a categorized failure.
    SolverFailed {
        /// The failed solver run.
        run: RunId,
        /// The start command that owns this run.
        command: CommandId,
        /// The categorized failure and diagnostic.
        failure: Failure,
    },
    /// Infrastructure acknowledged cancellation of a solver run.
    SolverCancelled {
        /// The cancelled solver run.
        run: RunId,
        /// The cancellation command being acknowledged.
        command: CommandId,
    },
    /// The runtime closed the session and requested mandatory cleanup.
    SessionEnded {
        /// The runtime reason for closure.
        reason: SessionEndReason,
    },
}
impl From<crate::action::Action> for Job {
    fn from(action: crate::action::Action) -> Self {
        Self::Action(action)
    }
}

/// Ordered semantic work emitted by the model.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CommandKind {
    /// Starts a trace-producing search from immutable givens.
    StartSolver {
        /// The new run identity, distinct from the command identity.
        run: RunId,
        /// The strategy identity resolved by infrastructure.
        solver: SolverId,
        /// The validated puzzle passed to the solver.
        puzzle: Board,
    },
    /// Cancels an outstanding solver run.
    CancelSolver {
        /// The run whose infrastructure must be stopped.
        run: RunId,
    },
    /// Requests a timer notification at an absolute logical deadline.
    ScheduleTimer {
        /// The earliest logical time at which the timer may fire.
        deadline: LogicalTime,
    },
}
/// An identified infrastructure request emitted by one activation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Command {
    /// The session-wide, strictly increasing command identity.
    pub id: CommandId,
    /// The semantic work to execute.
    pub kind: CommandKind,
}

/// One deterministic batch of jobs at an explicit logical time.
///
/// # Examples
///
/// ```
/// use ku_session::{action::Action, protocol::{Activation, ActivationId, LogicalTime, ProtocolError}};
///
/// assert_eq!(Activation::new(ActivationId::FIRST, LogicalTime::ZERO, vec![]),
///     Err(ProtocolError::EmptyActivation));
/// let activation = Activation::new(ActivationId::FIRST, LogicalTime::ZERO,
///     vec![Action::ToggleMode.into(), Action::StepForward.into()])?;
/// assert_eq!(activation.jobs().len(), 2); // The model processes jobs in this order.
/// # Ok::<(), ProtocolError>(())
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Activation {
    id: ActivationId,
    time: LogicalTime,
    jobs: Vec<Job>,
}
impl Activation {
    /// Constructs a batch while preserving job order.
    ///
    /// # Errors
    /// Returns [`ProtocolError::EmptyActivation`] if `jobs` is empty.
    /// Session ordering is checked separately by [`crate::app::App::activate`].
    pub fn new(id: ActivationId, time: LogicalTime, jobs: Vec<Job>) -> Result<Self, ProtocolError> {
        if jobs.is_empty() {
            return Err(ProtocolError::EmptyActivation);
        }
        Ok(Self { id, time, jobs })
    }
    /// Returns the activation identity.
    pub fn id(&self) -> ActivationId {
        self.id
    }
    /// Returns the absolute logical time for this batch.
    pub fn time(&self) -> LogicalTime {
        self.time
    }
    /// Borrows the jobs in their reduction order.
    pub fn jobs(&self) -> &[Job] {
        &self.jobs
    }
}
/// Acknowledges an activation with its ordered command sequence.
///
/// # Examples
///
/// ```
/// use ku_session::protocol::{ActivationId, Command, CommandId, CommandKind,
///     Completion, LogicalTime, ProtocolError};
///
/// let command = Command { id: CommandId::FIRST,
///     kind: CommandKind::ScheduleTimer { deadline: LogicalTime::from_millis(50) } };
/// assert!(Completion::new(ActivationId::FIRST, vec![command.clone()]).is_ok());
/// assert_eq!(Completion::new(ActivationId::FIRST, vec![command.clone(), command]),
///     Err(ProtocolError::CommandOrder));
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Completion {
    activation: ActivationId,
    commands: Vec<Command>,
}
impl Completion {
    /// Constructs a completion with an ordered command sequence.
    ///
    /// # Errors
    /// Returns [`ProtocolError::CommandOrder`] for repeated or decreasing command identities.
    /// An empty sequence is valid when the activation emits no work.
    pub fn new(activation: ActivationId, commands: Vec<Command>) -> Result<Self, ProtocolError> {
        if commands.windows(2).any(|pair| pair[0].id >= pair[1].id) {
            return Err(ProtocolError::CommandOrder);
        }
        Ok(Self {
            activation,
            commands,
        })
    }
    /// Returns the activation acknowledged by this completion.
    pub fn activation(&self) -> ActivationId {
        self.activation
    }
    /// Borrows the emitted commands in execution order.
    pub fn commands(&self) -> &[Command] {
        &self.commands
    }
}
