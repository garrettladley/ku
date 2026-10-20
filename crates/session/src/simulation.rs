//! Deterministic time skipping and callback-authored infrastructure responses.
//!
//! Commands and jobs use the same protocol as the live session. No thread,
//! clock, terminal, or solver participates in scheduling. Equal deadlines are
//! delivered in insertion order as one activation.

use std::collections::BTreeMap;

use crate::protocol::{
    Activation, ActivationId, Command, CommandId, CommandKind, Job, LogicalTime, ProtocolError,
    RunId,
};

/// A response authored by a simulated command executor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScheduledJob {
    /// The response's absolute time from the session's logical origin.
    pub time: LogicalTime,
    /// The semantic response delivered to the ordinary session driver.
    pub job: Job,
}

impl ScheduledJob {
    /// Associates a response with its logical delivery time.
    pub fn new(time: LogicalTime, job: Job) -> Self {
        Self { time, job }
    }
}

/// A rejected scheduling operation; the scheduler remains unchanged.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum SimulationError {
    /// An identity, time increment, or protocol envelope is invalid.
    #[error(transparent)]
    Protocol(#[from] ProtocolError),
    /// Delivery before the current virtual time would violate monotonicity.
    #[error("cannot schedule {scheduled:?} before current time {now:?}")]
    BackwardTime {
        /// Current scheduler time.
        now: LogicalTime,
        /// Rejected delivery time.
        scheduled: LogicalTime,
    },
    /// A callback response does not belong to the command being executed.
    #[error("response to command {command} has invalid correlation or job kind")]
    ResponseMismatch {
        /// The command whose response was rejected.
        command: CommandId,
    },
    /// A simulated executor failed before returning any responses.
    #[error("simulated executor failed: {0}")]
    Executor(String),
}

#[derive(Clone, Debug)]
struct PendingJob {
    job: Job,
    owner: Option<RunId>,
}

/// A virtual clock with FIFO ordering at equal deadlines.
///
/// [`Self::schedule_at`] explicitly injects external jobs, including stale
/// responses for race tests. Callback responses from [`Self::execute_with`] are
/// owned by their solver run and removed when its cancellation command executes.
/// Cancellation acknowledgements and injected jobs survive cancellation.
///
/// # Examples
///
/// ```
/// use ku_session::{action::Action, protocol::LogicalTime, simulation::VirtualScheduler};
///
/// let mut scheduler = VirtualScheduler::new();
/// let time = LogicalTime::from_millis(100);
/// scheduler.schedule_at(time, Action::ToggleMode.into())?;
/// scheduler.schedule_at(time, Action::StepForward.into())?;
/// let activation = scheduler.next_activation_auto()?.unwrap();
/// assert_eq!(activation.time(), time);
/// assert_eq!(activation.jobs(), &[Action::ToggleMode.into(), Action::StepForward.into()]);
/// assert_eq!(scheduler.now(), time); // Time skips directly to the deadline.
/// assert!(scheduler.is_empty());
/// assert!(scheduler.schedule_at(LogicalTime::ZERO, Action::Quit.into()).is_err());
/// # Ok::<(), ku_session::simulation::SimulationError>(())
/// ```
#[derive(Clone, Debug, Default)]
pub struct VirtualScheduler {
    now: LogicalTime,
    queue: BTreeMap<(LogicalTime, u64), PendingJob>,
    next_order: u64,
    last_command: Option<CommandId>,
    last_activation: Option<ActivationId>,
}

impl VirtualScheduler {
    /// Starts at logical time zero with no pending jobs.
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the current logical time.
    pub fn now(&self) -> LogicalTime {
        self.now
    }

    /// Synchronizes an accepted activation's time without discarding queued jobs.
    /// Overdue jobs retain their ordering and are delivered at the new time.
    ///
    /// # Errors
    /// Rejects a time earlier than the current scheduler time.
    pub fn advance_to(&mut self, time: LogicalTime) -> Result<(), SimulationError> {
        self.validate_time(time)?;
        self.now = time;
        Ok(())
    }

    /// Returns the number of pending jobs, including explicit injections.
    pub fn len(&self) -> usize {
        self.queue.len()
    }

    /// Whether no job can currently advance the simulation.
    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }

    /// Discards every pending job during mandatory simulation cleanup.
    ///
    /// Logical time and command, activation, and insertion identities remain
    /// unchanged, so cleanup cannot make a subsequent delivery appear earlier.
    pub fn clear(&mut self) {
        self.queue.clear();
    }

    /// Injects a job independently of any command or run ownership.
    ///
    /// Use this to author actions and deliberate stale-response races. Ordinary
    /// solver responses should be returned from [`Self::execute_with`] instead.
    ///
    /// # Errors
    /// Rejects past delivery times and exhausted insertion identities.
    pub fn schedule_at(&mut self, time: LogicalTime, job: Job) -> Result<(), SimulationError> {
        self.validate_time(time)?;
        self.reserve_order(1)?;
        self.insert(ScheduledJob::new(time, job), None);
        Ok(())
    }

    /// Executes timers and cancellation without authoring solver responses.
    ///
    /// Start commands produce no response here; inject one explicitly or use
    /// [`Self::execute_with`] with a callback representing the solver capability.
    ///
    /// # Errors
    /// Rejects reused/out-of-order commands, past timer deadlines, and exhausted
    /// insertion identities, as with [`Self::execute_with`].
    pub fn execute(&mut self, command: &Command) -> Result<(), SimulationError> {
        self.execute_with(command, &mut |_| Ok(Vec::new()))
    }

    /// Executes one command with callback-authored start or cancel responses.
    ///
    /// The callback runs once for a start or cancel command and never for a
    /// timer. Start responses must carry that command and run identity; cancel
    /// responses must be matching cancellation acknowledgements. An empty cancel
    /// response list produces an acknowledgement at the current time. A timer is
    /// delivered automatically as [`Job::TimerFired`] with its command identity.
    /// Validation occurs before queue mutation, including cancellation removal.
    ///
    /// # Errors
    /// Rejects reused/out-of-order commands, past times, mismatched responses,
    /// callback failures, and exhausted insertion identities.
    pub fn execute_with(
        &mut self,
        command: &Command,
        handler: &mut impl FnMut(&Command) -> Result<Vec<ScheduledJob>, SimulationError>,
    ) -> Result<(), SimulationError> {
        if self
            .last_command
            .is_some_and(|previous| command.id <= previous)
        {
            return Err(ProtocolError::CommandOrder.into());
        }
        let (responses, owner, cancelled) = match &command.kind {
            CommandKind::ScheduleTimer { deadline } => (
                vec![ScheduledJob::new(
                    *deadline,
                    Job::TimerFired {
                        command: command.id,
                    },
                )],
                None,
                None,
            ),
            CommandKind::StartSolver { run, .. } => (handler(command)?, Some(*run), None),
            CommandKind::CancelSolver { run } => {
                let mut responses = handler(command)?;
                if responses.is_empty() {
                    responses.push(ScheduledJob::new(
                        self.now,
                        Job::SolverCancelled {
                            run: *run,
                            command: command.id,
                        },
                    ));
                }
                (responses, None, Some(*run))
            }
        };
        for response in &responses {
            self.validate_time(response.time)?;
            if !matches_response(command, &response.job) {
                return Err(SimulationError::ResponseMismatch {
                    command: command.id,
                });
            }
        }
        self.reserve_order(responses.len())?;
        if let Some(run) = cancelled {
            self.queue.retain(|_, pending| pending.owner != Some(run));
        }
        for response in responses {
            self.insert(response, owner);
        }
        self.last_command = Some(command.id);
        Ok(())
    }

    /// Skips time to the next deadline and drains its jobs in insertion order.
    ///
    /// Returns `None` without advancing time when the queue is empty. Jobs
    /// scheduled at the current time form another batch on the next call.
    pub fn next_jobs(&mut self) -> Option<(LogicalTime, Vec<Job>)> {
        let (&(time, _), _) = self.queue.first_key_value()?;
        let mut jobs = Vec::new();
        while self
            .queue
            .first_key_value()
            .is_some_and(|(key, _)| key.0 == time)
        {
            if let Some((_, pending)) = self.queue.pop_first() {
                jobs.push(pending.job);
            }
        }
        self.now = self.now.max(time);
        Some((self.now, jobs))
    }

    /// Builds the next deadline's activation using a supplied monotonic identity.
    ///
    /// # Errors
    /// Rejects activation identities already used by this scheduler. Rejection
    /// leaves pending jobs and time unchanged; an empty queue returns `None`.
    pub fn next_activation(
        &mut self,
        id: ActivationId,
    ) -> Result<Option<Activation>, SimulationError> {
        if self.is_empty() {
            return Ok(None);
        }
        if self.last_activation.is_some_and(|previous| id <= previous) {
            return Err(ProtocolError::ActivationOrder.into());
        }
        let Some((time, jobs)) = self.next_jobs() else {
            return Ok(None);
        };
        let activation = Activation::new(id, time, jobs)?;
        self.last_activation = Some(id);
        Ok(Some(activation))
    }

    /// Allocates the next activation identity, starting at [`ActivationId::FIRST`].
    ///
    /// # Errors
    /// Reports exhaustion before removing any pending jobs.
    pub fn next_activation_auto(&mut self) -> Result<Option<Activation>, SimulationError> {
        if self.is_empty() {
            return Ok(None);
        }
        let id = self
            .last_activation
            .map_or(Ok(ActivationId::FIRST), ActivationId::checked_next)?;
        self.next_activation(id)
    }

    fn validate_time(&self, time: LogicalTime) -> Result<(), SimulationError> {
        if time < self.now {
            return Err(SimulationError::BackwardTime {
                now: self.now,
                scheduled: time,
            });
        }
        Ok(())
    }

    fn reserve_order(&self, count: usize) -> Result<(), SimulationError> {
        let count = u64::try_from(count).map_err(|_| ProtocolError::Exhausted)?;
        self.next_order
            .checked_add(count)
            .ok_or(ProtocolError::Exhausted)?;
        Ok(())
    }

    fn insert(&mut self, response: ScheduledJob, owner: Option<RunId>) {
        self.queue.insert(
            (response.time, self.next_order),
            PendingJob {
                job: response.job,
                owner,
            },
        );
        self.next_order += 1;
    }
}

fn matches_response(command: &Command, job: &Job) -> bool {
    match (&command.kind, job) {
        (CommandKind::ScheduleTimer { .. }, Job::TimerFired { command: response }) => {
            *response == command.id
        }
        (
            CommandKind::CancelSolver { run },
            Job::SolverCancelled {
                run: response_run,
                command: response,
            },
        ) => *response_run == *run && *response == command.id,
        (
            CommandKind::StartSolver { run, .. },
            Job::SolverStarted {
                run: response_run,
                command: response,
            }
            | Job::SolverEvents {
                run: response_run,
                command: response,
                ..
            }
            | Job::SolverCompleted {
                run: response_run,
                command: response,
                ..
            }
            | Job::SolverFailed {
                run: response_run,
                command: response,
                ..
            },
        ) => *response_run == *run && *response == command.id,
        _ => false,
    }
}

/// Adapts a function into an executor usable with any virtual scheduler.
#[derive(Clone, Copy, Debug)]
pub struct FunctionExecutor<F> {
    handler: F,
}

impl<F> FunctionExecutor<F> {
    /// Wraps a callback that authors scheduled command responses.
    pub fn new(handler: F) -> Self {
        Self { handler }
    }

    /// Returns the callback and any state it captured.
    pub fn into_inner(self) -> F {
        self.handler
    }
}

impl<F: FnMut(&Command) -> Result<Vec<ScheduledJob>, SimulationError>> FunctionExecutor<F> {
    /// Executes a command, including automatic timers and run cancellation.
    pub fn execute(
        &mut self,
        scheduler: &mut VirtualScheduler,
        command: &Command,
    ) -> Result<(), SimulationError> {
        scheduler.execute_with(command, &mut self.handler)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exhausted_insertion_identity_leaves_the_queue_unchanged() {
        let mut scheduler = VirtualScheduler::new();
        scheduler.next_order = u64::MAX;
        assert_eq!(
            scheduler.schedule_at(
                LogicalTime::ZERO,
                Job::TimerFired {
                    command: CommandId::FIRST
                }
            ),
            Err(SimulationError::Protocol(ProtocolError::Exhausted)),
        );
        assert!(scheduler.is_empty());
        assert_eq!(scheduler.now(), LogicalTime::ZERO);
    }
}
