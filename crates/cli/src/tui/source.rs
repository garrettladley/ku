//! Replaceable live and function-backed solver event streams.

use std::{
    any::Any,
    collections::VecDeque,
    fmt, io,
    num::NonZeroUsize,
    ops::ControlFlow,
    sync::mpsc::{self, Receiver, TryRecvError},
    thread::{self, JoinHandle},
};

use ku_core::{Board, TraceEvent, TraceOutcome, TraceSolver};

/// A producer's current state, distinguishing temporary delays from termination.
#[derive(Clone, Debug, PartialEq, Eq)]
#[expect(
    clippy::large_enum_variant,
    reason = "live events are polled individually; keep their snapshots inline without a heap allocation per poll"
)]
pub enum SourcePoll {
    /// An available event from the active run.
    Event(TraceEvent),
    /// The producer is active but has no available event yet.
    Pending,
    /// The stream has ended or no run is active.
    Closed,
}

/// The capability at which simulations substitute search without running a solver.
pub trait SolverEventSource {
    /// Starts a fresh run, replacing and cancelling any previous stream.
    fn start(&mut self, solver: &'static dyn TraceSolver, puzzle: &Board) -> io::Result<()>;
    /// Reads an event without blocking; closure does not itself imply completion.
    fn poll_event(&mut self) -> io::Result<SourcePoll>;
    /// Stops the producer before a puzzle/solver change or session exit.
    ///
    /// Must be idempotent: effects, guaranteed session cleanup, and `Drop` may
    /// each request cancellation of the same run.
    fn cancel(&mut self);
}

#[derive(Default)]
enum WorkerState {
    #[default]
    Idle,
    Active {
        receiver: Receiver<TraceEvent>,
        worker: JoinHandle<TraceOutcome>,
    },
}

/// A bounded channel keeps search off the UI thread and applies backpressure.
#[derive(Default)]
pub struct WorkerSource {
    state: WorkerState,
}

impl SolverEventSource for WorkerSource {
    fn start(&mut self, solver: &'static dyn TraceSolver, puzzle: &Board) -> io::Result<()> {
        self.cancel();
        let (sender, receiver) = mpsc::sync_channel(32);
        let puzzle = puzzle.clone();
        let worker = thread::Builder::new()
            .name("ku-solver".into())
            .spawn(move || {
                let mut sink = |event| match sender.send(event) {
                    Ok(()) => ControlFlow::Continue(()),
                    Err(_) => ControlFlow::Break(()),
                };
                solver.trace_search(&puzzle, NonZeroUsize::MIN, &mut sink)
            })?;
        self.state = WorkerState::Active { receiver, worker };
        Ok(())
    }

    fn poll_event(&mut self) -> io::Result<SourcePoll> {
        let WorkerState::Active { receiver, .. } = &self.state else {
            return Ok(SourcePoll::Closed);
        };
        match receiver.try_recv() {
            Ok(event) => Ok(SourcePoll::Event(event)),
            Err(TryRecvError::Empty) => Ok(SourcePoll::Pending),
            Err(TryRecvError::Disconnected) => {
                if let WorkerState::Active { receiver, worker } = std::mem::take(&mut self.state) {
                    drop(receiver);
                    match worker.join() {
                        Ok(TraceOutcome::Completed(_) | TraceOutcome::Cancelled(_)) => {}
                        Err(payload) => return Err(io::Error::other(WorkerPanic::new(payload))),
                    }
                }
                Ok(SourcePoll::Closed)
            }
        }
    }

    fn cancel(&mut self) {
        if let WorkerState::Active { receiver, worker } = std::mem::take(&mut self.state) {
            // Drop before joining: a send may be waiting on the full channel.
            drop(receiver);
            let _ = worker.join();
        }
    }
}

impl Drop for WorkerSource {
    fn drop(&mut self) {
        self.cancel();
    }
}

/// The solver worker panicked; retained as the I/O error's underlying cause.
#[derive(Debug)]
pub struct WorkerPanic {
    message: String,
}

impl WorkerPanic {
    fn new(payload: Box<dyn Any + Send>) -> Self {
        let message = if let Some(message) = payload.downcast_ref::<String>() {
            message.clone()
        } else if let Some(message) = payload.downcast_ref::<&str>() {
            (*message).to_owned()
        } else {
            "non-string panic payload".to_owned()
        };
        Self { message }
    }
}

impl fmt::Display for WorkerPanic {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "solver worker panicked: {}", self.message)
    }
}

impl std::error::Error for WorkerPanic {}

#[derive(Default)]
enum FunctionState {
    #[default]
    Idle,
    Active(VecDeque<SourcePoll>),
}

/// Invokes a function once per run and plays its scripted source responses.
///
/// The callback sees the same strategy and puzzle as the live worker. It can
/// return events, explicit pending polls, or errors without starting a thread or
/// searching. An explicit [`SourcePoll::Closed`] discards subsequent responses;
/// exhaustion also closes the stream. Trace protocol validation belongs to the
/// application's history, so malformed scripts can test its error paths.
///
/// # Examples
///
/// ```
/// use ku_cli::tui::source::{FunctionSolverSource, SolverEventSource, SourcePoll};
/// use ku_core::{Board, TraceSolver};
/// use ku_solvers::Bitmask;
///
/// let mut source = FunctionSolverSource::new(|_: &'static dyn TraceSolver, _: &Board| {
///     Ok(vec![SourcePoll::Pending, SourcePoll::Closed])
/// });
/// source.start(&Bitmask, &Board::default())?;
/// assert_eq!(source.poll_event()?, SourcePoll::Pending);
/// assert_eq!(source.poll_event()?, SourcePoll::Closed);
/// assert_eq!(source.starts(), 1);
/// source.cancel(); // Safe to call repeatedly.
/// source.cancel();
/// # Ok::<(), std::io::Error>(())
/// ```
pub struct FunctionSolverSource<F> {
    function: F,
    state: FunctionState,
    starts: usize,
    cancellations: usize,
}

impl<F> FunctionSolverSource<F> {
    /// Wraps a function that produces a fresh scripted stream for each start.
    pub fn new(function: F) -> Self {
        Self {
            function,
            state: FunctionState::Idle,
            starts: 0,
            cancellations: 0,
        }
    }

    /// Returns callback invocations, including starts that returned an error.
    pub fn starts(&self) -> usize {
        self.starts
    }

    /// Returns active streams discarded by cancellation or replacement.
    pub fn cancellations(&self) -> usize {
        self.cancellations
    }

    fn clear(&mut self) {
        if matches!(self.state, FunctionState::Active(_)) {
            self.cancellations = self.cancellations.saturating_add(1);
            self.state = FunctionState::Idle;
        }
    }
}

impl<F> SolverEventSource for FunctionSolverSource<F>
where
    F: FnMut(&'static dyn TraceSolver, &Board) -> io::Result<Vec<SourcePoll>>,
{
    fn start(&mut self, solver: &'static dyn TraceSolver, puzzle: &Board) -> io::Result<()> {
        self.clear();
        self.starts = self.starts.saturating_add(1);
        let polls = (self.function)(solver, puzzle)?;
        self.state = FunctionState::Active(polls.into());
        Ok(())
    }

    fn poll_event(&mut self) -> io::Result<SourcePoll> {
        let FunctionState::Active(polls) = &mut self.state else {
            return Ok(SourcePoll::Closed);
        };
        let poll = polls.pop_front().unwrap_or(SourcePoll::Closed);
        if matches!(poll, SourcePoll::Closed) {
            self.state = FunctionState::Idle;
        }
        Ok(poll)
    }

    fn cancel(&mut self) {
        self.clear();
    }
}

#[cfg(test)]
mod tests {
    use std::{
        cell::Cell,
        sync::atomic::{AtomicBool, AtomicUsize, Ordering},
        time::{Duration, Instant},
    };

    use ku_core::{SearchExtent, SearchProgress, SearchResult, Solver, TraceSink};

    use super::*;

    fn empty_result(puzzle: &Board) -> SearchResult {
        SearchResult::new(puzzle, None, 0, SearchExtent::Exhausted, Default::default()).unwrap()
    }

    fn start_event(solver: &dyn TraceSolver, puzzle: &Board) -> TraceEvent {
        TraceEvent::Start {
            solver: solver.name().to_owned(),
            puzzle: puzzle.clone(),
        }
    }

    struct ScriptedTrace {
        attempts: AtomicUsize,
        canceled: AtomicBool,
    }

    impl Solver for ScriptedTrace {
        fn name(&self) -> &'static str {
            "scripted"
        }
        fn search(&self, puzzle: &Board, _: NonZeroUsize) -> SearchResult {
            empty_result(puzzle)
        }
    }

    impl TraceSolver for ScriptedTrace {
        fn trace_search(
            &self,
            puzzle: &Board,
            _: NonZeroUsize,
            sink: &mut dyn TraceSink,
        ) -> TraceOutcome {
            for _ in 0..100 {
                self.attempts.fetch_add(1, Ordering::SeqCst);
                if sink.event(start_event(self, puzzle)).is_break() {
                    self.canceled.store(true, Ordering::SeqCst);
                    return TraceOutcome::Cancelled(SearchProgress::default());
                }
            }
            TraceOutcome::Completed(empty_result(puzzle))
        }
    }

    #[test]
    fn cancel_wakes_and_joins_a_producer_blocked_on_a_full_channel() {
        static SCRIPT: ScriptedTrace = ScriptedTrace {
            attempts: AtomicUsize::new(0),
            canceled: AtomicBool::new(false),
        };
        let mut source = WorkerSource::default();
        source.start(&SCRIPT, &Board::default()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while SCRIPT.attempts.load(Ordering::SeqCst) < 33 {
            assert!(
                Instant::now() < deadline,
                "producer never reached backpressure"
            );
            thread::yield_now();
        }
        source.cancel();
        assert!(SCRIPT.canceled.load(Ordering::SeqCst));
        assert!(matches!(source.state, WorkerState::Idle));
        source.cancel();
        source
            .start(ku_solvers::built_in_trace_solvers()[1], &Board::default())
            .unwrap();
        source.cancel();
        assert_eq!(source.poll_event().unwrap(), SourcePoll::Closed);
    }

    #[test]
    fn dropping_a_source_cancels_its_blocked_worker() {
        static SCRIPT: ScriptedTrace = ScriptedTrace {
            attempts: AtomicUsize::new(0),
            canceled: AtomicBool::new(false),
        };
        let mut source = WorkerSource::default();
        source.start(&SCRIPT, &Board::default()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while SCRIPT.attempts.load(Ordering::SeqCst) < 33 {
            assert!(
                Instant::now() < deadline,
                "producer never reached backpressure"
            );
            thread::yield_now();
        }
        drop(source);
        assert!(SCRIPT.canceled.load(Ordering::SeqCst));
    }

    struct PanickingTrace;

    impl Solver for PanickingTrace {
        fn name(&self) -> &'static str {
            "panicking"
        }
        fn search(&self, puzzle: &Board, _: NonZeroUsize) -> SearchResult {
            empty_result(puzzle)
        }
    }

    impl TraceSolver for PanickingTrace {
        fn trace_search(&self, _: &Board, _: NonZeroUsize, _: &mut dyn TraceSink) -> TraceOutcome {
            panic!("scripted worker failure")
        }
    }

    #[test]
    fn worker_panics_preserve_a_typed_error_cause_and_close_the_source() {
        let mut source = WorkerSource::default();
        source.start(&PanickingTrace, &Board::default()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let error = loop {
            match source.poll_event() {
                Err(error) => break error,
                Ok(SourcePoll::Pending) => thread::yield_now(),
                poll => panic!("unexpected poll: {poll:?}"),
            }
            assert!(Instant::now() < deadline, "worker never stopped");
        };
        assert!(error.get_ref().unwrap().is::<WorkerPanic>());
        assert!(error.to_string().contains("scripted worker failure"));
        assert_eq!(source.poll_event().unwrap(), SourcePoll::Closed);
    }

    #[test]
    fn function_source_receives_run_context_and_preserves_pending_polls() {
        let calls = Cell::new(0);
        let puzzle = Board::default();
        let solver = ku_solvers::built_in_trace_solvers()[1];
        let event = start_event(solver, &puzzle);
        let mut source =
            FunctionSolverSource::new(|selected: &'static dyn TraceSolver, board: &Board| {
                calls.set(calls.get() + 1);
                assert_eq!(selected.name(), solver.name());
                assert_eq!(board, &puzzle);
                Ok(vec![SourcePoll::Pending, SourcePoll::Event(event.clone())])
            });
        assert_eq!(source.poll_event().unwrap(), SourcePoll::Closed);
        source.start(solver, &puzzle).unwrap();
        assert_eq!(source.poll_event().unwrap(), SourcePoll::Pending);
        assert_eq!(
            source.poll_event().unwrap(),
            SourcePoll::Event(event.clone())
        );
        assert_eq!(source.poll_event().unwrap(), SourcePoll::Closed);
        assert_eq!(source.poll_event().unwrap(), SourcePoll::Closed);
        assert_eq!(calls.get(), 1);
        assert_eq!(source.starts(), 1);
        assert_eq!(source.cancellations(), 0);
    }

    #[test]
    fn explicit_closure_discards_later_scripted_responses() {
        let mut source = FunctionSolverSource::new(|_: &'static dyn TraceSolver, _: &Board| {
            Ok(vec![SourcePoll::Closed, SourcePoll::Pending])
        });
        source
            .start(ku_solvers::built_in_trace_solvers()[0], &Board::default())
            .unwrap();
        assert_eq!(source.poll_event().unwrap(), SourcePoll::Closed);
        assert_eq!(source.poll_event().unwrap(), SourcePoll::Closed);
    }

    #[test]
    fn function_streams_can_be_canceled_replaced_and_reused() {
        let mut source = FunctionSolverSource::new(|_: &'static dyn TraceSolver, _: &Board| {
            Ok(vec![SourcePoll::Pending])
        });
        let solver = ku_solvers::built_in_trace_solvers()[0];
        source.start(solver, &Board::default()).unwrap();
        source.start(solver, &Board::default()).unwrap();
        assert_eq!(source.cancellations(), 1);
        source.cancel();
        source.cancel();
        assert_eq!(source.cancellations(), 2);
        assert_eq!(source.poll_event().unwrap(), SourcePoll::Closed);
        source.start(solver, &Board::default()).unwrap();
        assert_eq!(source.poll_event().unwrap(), SourcePoll::Pending);
        assert_eq!(source.starts(), 3);
    }

    #[test]
    fn failed_starts_cancel_previous_streams_and_leave_the_source_reusable() {
        let mut calls = 0;
        let mut source = FunctionSolverSource::new(|_: &'static dyn TraceSolver, _: &Board| {
            calls += 1;
            if calls == 2 {
                return Err(io::Error::from(io::ErrorKind::InvalidData));
            }
            Ok(vec![SourcePoll::Pending])
        });
        let solver = ku_solvers::built_in_trace_solvers()[0];
        source.start(solver, &Board::default()).unwrap();
        assert_eq!(
            source.start(solver, &Board::default()).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        assert_eq!(source.poll_event().unwrap(), SourcePoll::Closed);
        assert_eq!(source.cancellations(), 1);
        source.start(solver, &Board::default()).unwrap();
        assert_eq!(source.poll_event().unwrap(), SourcePoll::Pending);
        assert_eq!(source.starts(), 3);
    }
}
