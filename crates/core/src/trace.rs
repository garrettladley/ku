//! Solver events, validated bounded history, and deterministic replay.
use std::{mem::size_of, num::NonZeroUsize, ops::ControlFlow};

use crate::{Board, CandidateMask, CellIndex, Digit, SearchResult, SearchStats};

/// The operation that produced a solver snapshot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TraceKind {
    /// Entering a recursive search node.
    Node,
    /// Computing or changing candidate sets.
    Candidates,
    /// Placing a digit in the search state.
    Assignment {
        /// The cell receiving the digit.
        cell: CellIndex,
        /// The placed digit.
        digit: Digit,
        /// Whether the selected constraint had one remaining choice.
        forced: bool,
    },
    /// Restoring the parent state after a branch.
    Restore,
    /// Discovering a contradiction or unsuccessful node.
    Contradiction,
    /// Finding a complete solution.
    Solution,
}

/// Complete observable state after an operation, independent of replay direction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TraceSnapshot {
    board: Board,
    candidates: Option<[CandidateMask; 81]>,
    stats: SearchStats,
    depth: usize,
    focus: CellIndex,
}
impl TraceSnapshot {
    /// Records a validated board, candidates, work, depth, and focused cell.
    pub fn new(
        board: Board,
        candidates: Option<[CandidateMask; 81]>,
        stats: SearchStats,
        depth: usize,
        focus: CellIndex,
    ) -> Self {
        Self {
            board,
            candidates,
            stats,
            depth,
            focus,
        }
    }
    /// Returns the current validated board.
    pub fn board(&self) -> &Board {
        &self.board
    }
    /// Returns the current row-major cells.
    pub fn cells(&self) -> &[u8; 81] {
        self.board.cells()
    }
    /// Returns actual candidate sets when the strategy exposes them.
    pub fn candidates(&self) -> Option<&[CandidateMask; 81]> {
        self.candidates.as_ref()
    }
    /// Returns accumulated search work.
    pub fn stats(&self) -> SearchStats {
        self.stats
    }
    /// Returns recursion depth.
    pub fn depth(&self) -> usize {
        self.depth
    }
    /// Returns the focused cell.
    pub fn focus(&self) -> CellIndex {
        self.focus
    }
}

/// A self-contained event emitted by an actual solver search.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TraceEvent {
    /// Begins a run from immutable givens.
    Start {
        /// Stable strategy name.
        solver: String,
        /// Original puzzle.
        puzzle: Board,
    },
    /// State immediately after a search operation.
    Step {
        /// The search operation.
        kind: TraceKind,
        /// Complete observable state.
        snapshot: TraceSnapshot,
    },
    /// Completed search; interrupted searches emit no finish event.
    Finished {
        /// Validated completed enumeration.
        result: SearchResult,
        /// Complete terminal state, including unsatisfiable searches.
        snapshot: TraceSnapshot,
    },
}

/// Receives synchronous events; a break requests cooperative cancellation.
pub trait TraceSink {
    /// Records an event and controls continuation of the search.
    fn event(&mut self, event: TraceEvent) -> ControlFlow<()>;
}
impl<F: FnMut(TraceEvent) -> ControlFlow<()>> TraceSink for F {
    fn event(&mut self, event: TraceEvent) -> ControlFlow<()> {
        self(event)
    }
}

/// Current observable state derived solely from recorded events.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TraceState {
    snapshot: TraceSnapshot,
    finished: Option<SearchResult>,
}
impl TraceState {
    /// Returns the complete current snapshot.
    pub fn snapshot(&self) -> &TraceSnapshot {
        &self.snapshot
    }
    /// Returns current cells.
    pub fn cells(&self) -> &[u8; 81] {
        self.snapshot.cells()
    }
    /// Returns current candidates when available.
    pub fn candidates(&self) -> Option<&[CandidateMask; 81]> {
        self.snapshot.candidates()
    }
    /// Returns current search work.
    pub fn stats(&self) -> SearchStats {
        self.snapshot.stats()
    }
    /// Returns current search depth.
    pub fn depth(&self) -> usize {
        self.snapshot.depth()
    }
    /// Returns current focused cell.
    pub fn focus(&self) -> CellIndex {
        self.snapshot.focus()
    }
    /// Borrows the completed result if the cursor includes a finish event.
    pub fn finished(&self) -> Option<&SearchResult> {
        self.finished.as_ref()
    }
}

/// A reducer that executes no solver and performs no terminal I/O.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TraceReplay {
    state: TraceState,
}
impl TraceReplay {
    /// Initializes at the puzzle's givens.
    pub fn new(puzzle: &Board) -> Self {
        Self {
            state: TraceState {
                snapshot: TraceSnapshot::new(
                    puzzle.clone(),
                    None,
                    SearchStats::default(),
                    0,
                    CellIndex::FIRST,
                ),
                finished: None,
            },
        }
    }
    /// Applies a self-contained event. Use [`TraceHistory`] to validate protocols.
    pub fn apply(&mut self, event: &TraceEvent) {
        match event {
            TraceEvent::Start { puzzle, .. } => *self = Self::new(puzzle),
            TraceEvent::Step { snapshot, .. } => {
                self.state = TraceState {
                    snapshot: snapshot.clone(),
                    finished: None,
                }
            }
            TraceEvent::Finished { result, snapshot } => {
                self.state = TraceState {
                    snapshot: snapshot.clone(),
                    finished: Some(result.clone()),
                }
            }
        }
    }
    /// Returns the current replay state.
    pub fn state(&self) -> &TraceState {
        &self.state
    }
}

/// Limits retained events and their allocated entry storage.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HistoryLimits {
    max_events: NonZeroUsize,
    max_bytes: NonZeroUsize,
}
impl HistoryLimits {
    /// Sets independent nonzero event and allocation budgets.
    pub const fn new(max_events: NonZeroUsize, max_bytes: NonZeroUsize) -> Self {
        Self {
            max_events,
            max_bytes,
        }
    }
    /// Returns the maximum retained event count.
    pub const fn max_events(self) -> NonZeroUsize {
        self.max_events
    }
    /// Returns the maximum allocated entry bytes.
    pub const fn max_bytes(self) -> NonZeroUsize {
        self.max_bytes
    }
}
impl Default for HistoryLimits {
    fn default() -> Self {
        Self {
            max_events: NonZeroUsize::MIN.saturating_add(249_999),
            max_bytes: NonZeroUsize::MIN.saturating_add(64 * 1024 * 1024 - 1),
        }
    }
}

/// A rejected history operation; recorded state remains unchanged.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum HistoryError {
    /// The event count limit was reached.
    #[error("trace event limit reached")]
    EventLimit,
    /// Another entry would exceed the allocated byte budget.
    #[error("trace allocation byte limit reached")]
    ByteLimit,
    /// Storage could not be allocated.
    #[error("could not allocate trace history storage")]
    Allocation,
    /// Event order or data violates the declared trace protocol.
    #[error("invalid trace protocol: {0}")]
    Protocol(&'static str),
    /// Requested event count lies outside the history.
    #[error("cursor {cursor} exceeds history length {len}")]
    Cursor {
        /// Rejected cursor.
        cursor: usize,
        /// Available event count.
        len: usize,
    },
}

#[derive(Clone, Debug)]
struct HistoryEntry {
    event: TraceEvent,
    previous_candidate: Option<usize>,
}

/// An append-only validated trace with a separately controlled replay cursor.
///
/// The cursor is the number of applied events, from zero through [`Self::len`].
/// Appending never advances it. Self-contained snapshots make seeking constant
/// time, and metadata locates prior candidate sets without rescanning history.
///
/// # Examples
///
/// ```
/// use ku_core::{Board, CellIndex, HistoryLimits, SearchStats, TraceEvent,
///     TraceHistory, TraceKind, TraceSnapshot};
///
/// let puzzle = Board::default();
/// let mut history = TraceHistory::new(puzzle.clone(), "example", HistoryLimits::default());
/// history.push(TraceEvent::Start { solver: "example".into(), puzzle: puzzle.clone() })?;
/// history.push(TraceEvent::Step {
///     kind: TraceKind::Node,
///     snapshot: TraceSnapshot::new(puzzle, None, SearchStats::default(), 0, CellIndex::FIRST),
/// })?;
/// assert_eq!(history.len(), 2);
/// assert_eq!(history.cursor(), 0); // Appending does not advance the view.
/// history.seek(history.len())?;
/// assert!(history.step_back());
/// assert_eq!(history.cursor(), 1);
/// assert!(history.seek(3).is_err());
/// assert_eq!(history.cursor(), 1); // Rejected navigation leaves the view unchanged.
/// # Ok::<(), ku_core::HistoryError>(())
/// ```
#[derive(Clone, Debug)]
pub struct TraceHistory {
    puzzle: Board,
    solver: String,
    limits: HistoryLimits,
    entries: Vec<HistoryEntry>,
    cursor: usize,
    replay: TraceReplay,
    latest_candidate: Option<usize>,
}
impl TraceHistory {
    /// Creates an empty history with immutable expected puzzle and strategy.
    pub fn new(puzzle: Board, solver: impl Into<String>, limits: HistoryLimits) -> Self {
        Self {
            replay: TraceReplay::new(&puzzle),
            puzzle,
            solver: solver.into(),
            limits,
            entries: Vec::new(),
            cursor: 0,
            latest_candidate: None,
        }
    }
    /// Validates and appends an event without moving the replay cursor.
    ///
    /// # Errors
    /// Rejects invalid trace ordering or snapshots, exceeded event/byte budgets,
    /// and storage allocation failures. Rejection leaves history unchanged.
    pub fn push(&mut self, event: TraceEvent) -> Result<(), HistoryError> {
        self.validate(&event)?;
        if self.entries.len() >= self.limits.max_events.get() {
            return Err(HistoryError::EventLimit);
        }
        let payload_bytes = self.payload_bytes()
            + match &event {
                TraceEvent::Start { solver, .. } => solver.capacity(),
                _ => 0,
            };
        let max_capacity = (self.limits.max_bytes.get().saturating_sub(payload_bytes)
            / size_of::<HistoryEntry>())
        .min(self.limits.max_events.get());
        if self.entries.len() >= max_capacity {
            return Err(HistoryError::ByteLimit);
        }
        if self.entries.len() == self.entries.capacity() {
            let target = self
                .entries
                .capacity()
                .saturating_mul(2)
                .max(16)
                .min(max_capacity);
            self.entries
                .try_reserve_exact(target - self.entries.len())
                .map_err(|_| HistoryError::Allocation)?;
        }
        let has_candidates =
            event_snapshot(&event).is_some_and(|snapshot| snapshot.candidates().is_some());
        let index = self.entries.len();
        self.entries.push(HistoryEntry {
            event,
            previous_candidate: self.latest_candidate,
        });
        if has_candidates {
            self.latest_candidate = Some(index);
        }
        Ok(())
    }
    /// Checks the next event's protocol and domain relationships without mutation.
    ///
    /// Runtime adapters can turn malformed producer data into a portable failure
    /// before delivering it to a session. Storage budgets are checked by `push`.
    pub fn validate_next(&self, event: &TraceEvent) -> Result<(), HistoryError> {
        self.validate(event)
    }
    fn validate(&self, event: &TraceEvent) -> Result<(), HistoryError> {
        if self.is_complete() {
            return Err(HistoryError::Protocol("event after completion"));
        }
        match event {
            TraceEvent::Start { solver, puzzle } => {
                if !self.entries.is_empty() {
                    return Err(HistoryError::Protocol("duplicate start"));
                }
                if solver != &self.solver || puzzle != &self.puzzle {
                    return Err(HistoryError::Protocol("start does not match run"));
                }
            }
            TraceEvent::Step { kind, snapshot } => {
                if self.entries.is_empty() {
                    return Err(HistoryError::Protocol("step before start"));
                }
                self.validate_snapshot(snapshot)?;
                if let TraceKind::Assignment { cell, digit, .. } = kind
                    && (snapshot.cells()[cell.get()] != digit.get() || snapshot.focus() != *cell)
                {
                    return Err(HistoryError::Protocol("assignment does not match snapshot"));
                }
                if matches!(kind, TraceKind::Solution)
                    && !snapshot.board().is_solution_for(&self.puzzle)
                {
                    return Err(HistoryError::Protocol("solution snapshot is incomplete"));
                }
            }
            TraceEvent::Finished { result, snapshot } => {
                if self.entries.is_empty() {
                    return Err(HistoryError::Protocol("finish before start"));
                }
                self.validate_snapshot(snapshot)?;
                if SearchResult::new(
                    &self.puzzle,
                    result.solution().cloned(),
                    result.solutions_found(),
                    result.extent(),
                    result.stats(),
                )
                .is_err()
                    || result.stats() != snapshot.stats()
                    || result
                        .solution()
                        .is_some_and(|solution| solution != snapshot.board())
                {
                    return Err(HistoryError::Protocol("finish does not match result"));
                }
            }
        }
        Ok(())
    }
    fn validate_snapshot(&self, snapshot: &TraceSnapshot) -> Result<(), HistoryError> {
        if self
            .puzzle
            .cells()
            .iter()
            .zip(snapshot.cells())
            .any(|(&given, &actual)| given != 0 && given != actual)
        {
            return Err(HistoryError::Protocol("snapshot changes givens"));
        }
        if let Some(previous) = self.last_event().and_then(event_snapshot) {
            let previous = previous.stats();
            let current = snapshot.stats();
            if current.nodes < previous.nodes
                || current.guesses < previous.guesses
                || current.backtracks < previous.backtracks
                || current.forced_moves < previous.forced_moves
                || current.max_depth < previous.max_depth
            {
                return Err(HistoryError::Protocol("search work decreases"));
            }
        }
        Ok(())
    }
    /// Moves to an applied-event count, restoring the full observable state.
    ///
    /// # Errors
    /// Returns [`HistoryError::Cursor`] if `cursor` exceeds the retained event count.
    pub fn seek(&mut self, cursor: usize) -> Result<(), HistoryError> {
        if cursor > self.len() {
            return Err(HistoryError::Cursor {
                cursor,
                len: self.len(),
            });
        }
        if cursor == 0 {
            self.replay = TraceReplay::new(&self.puzzle);
        } else {
            self.replay.apply(&self.entries[cursor - 1].event);
        }
        self.cursor = cursor;
        Ok(())
    }
    /// Applies one more retained event, returning whether the cursor moved.
    pub fn step_forward(&mut self) -> bool {
        if self.cursor == self.len() {
            return false;
        }
        self.replay.apply(&self.entries[self.cursor].event);
        self.cursor += 1;
        true
    }
    /// Rewinds one retained event, returning whether the cursor moved.
    pub fn step_back(&mut self) -> bool {
        if self.cursor == 0 {
            return false;
        }
        // The bounds are established above; this seek cannot fail.
        self.seek(self.cursor - 1).is_ok()
    }
    /// Returns the applied-event count.
    pub fn cursor(&self) -> usize {
        self.cursor
    }
    /// Returns the retained event count.
    pub fn len(&self) -> usize {
        self.entries.len()
    }
    /// Whether no event has been retained.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
    /// Returns state at the current cursor.
    pub fn state(&self) -> &TraceState {
        self.replay.state()
    }
    /// Borrows an event by its zero-based storage index.
    pub fn event(&self, index: usize) -> Option<&TraceEvent> {
        self.entries.get(index).map(|entry| &entry.event)
    }
    /// Iterates recorded events in chronological order.
    pub fn iter(&self) -> impl ExactSizeIterator<Item = &TraceEvent> {
        self.entries.iter().map(|entry| &entry.event)
    }
    /// Returns the last retained event independently of the replay cursor.
    pub fn last_event(&self) -> Option<&TraceEvent> {
        self.entries.last().map(|entry| &entry.event)
    }
    /// Returns the most recent candidate snapshot before the current event.
    pub fn previous_candidates(&self) -> Option<&[CandidateMask; 81]> {
        let index = self
            .entries
            .get(self.cursor.checked_sub(1)?)?
            .previous_candidate?;
        event_snapshot(&self.entries[index].event)?.candidates()
    }
    fn payload_bytes(&self) -> usize {
        match self.event(0) {
            Some(TraceEvent::Start { solver, .. }) => solver.capacity(),
            _ => 0,
        }
    }
    /// Returns allocated entry storage bytes, including unused capacity and names.
    pub fn capacity_bytes(&self) -> usize {
        self.entries.capacity() * size_of::<HistoryEntry>() + self.payload_bytes()
    }
    /// Returns the configured storage budgets.
    pub fn limits(&self) -> HistoryLimits {
        self.limits
    }
    /// Whether the retained stream has a completed terminal event.
    pub fn is_complete(&self) -> bool {
        matches!(self.last_event(), Some(TraceEvent::Finished { .. }))
    }
}
fn event_snapshot(event: &TraceEvent) -> Option<&TraceSnapshot> {
    match event {
        TraceEvent::Start { .. } => None,
        TraceEvent::Step { snapshot, .. } | TraceEvent::Finished { snapshot, .. } => Some(snapshot),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Digit, SearchExtent};

    const SOLUTION: &str =
        "534678912672195348198342567859761423426853791713924856961537284287419635345286179";

    fn snapshot(board: Board, stats: SearchStats, focus: CellIndex) -> TraceSnapshot {
        TraceSnapshot::new(board, None, stats, 0, focus)
    }

    fn started(puzzle: Board) -> TraceHistory {
        let mut history = TraceHistory::new(puzzle.clone(), "test", HistoryLimits::default());
        history
            .push(TraceEvent::Start {
                solver: "test".into(),
                puzzle,
            })
            .unwrap();
        history
    }

    fn assert_rejected(history: &mut TraceHistory, event: TraceEvent, reason: &'static str) {
        let capacity = history.capacity_bytes();
        let before = history.clone();
        assert_eq!(
            history.validate_next(&event),
            Err(HistoryError::Protocol(reason))
        );
        assert_eq!(history.push(event), Err(HistoryError::Protocol(reason)));
        assert_eq!(
            history.iter().collect::<Vec<_>>(),
            before.iter().collect::<Vec<_>>()
        );
        assert_eq!(history.cursor(), before.cursor());
        assert_eq!(history.state(), before.state());
        assert_eq!(history.capacity_bytes(), capacity);
    }

    #[test]
    fn assignment_requires_both_the_declared_digit_and_focus() {
        let mut history = started(Board::default());
        let mut cells = [0; 81];
        cells[80] = 9;
        let board = Board::try_from(cells).unwrap();
        let cell = CellIndex::try_from(80).unwrap();
        for (digit, focus) in [(8, cell), (9, CellIndex::FIRST)] {
            assert_rejected(
                &mut history,
                TraceEvent::Step {
                    kind: TraceKind::Assignment {
                        cell,
                        digit: Digit::try_from(digit).unwrap(),
                        forced: true,
                    },
                    snapshot: snapshot(board.clone(), SearchStats::default(), focus),
                },
                "assignment does not match snapshot",
            );
        }
        history
            .push(TraceEvent::Step {
                kind: TraceKind::Assignment {
                    cell,
                    digit: Digit::try_from(9).unwrap(),
                    forced: true,
                },
                snapshot: snapshot(board, SearchStats::default(), cell),
            })
            .unwrap();
    }

    #[test]
    fn each_work_counter_is_monotonic_against_the_tail_even_while_rewound() {
        let stats = SearchStats {
            nodes: 5,
            guesses: 5,
            backtracks: 5,
            forced_moves: 5,
            max_depth: 5,
        };
        let mut history = started(Board::default());
        history
            .push(TraceEvent::Step {
                kind: TraceKind::Node,
                snapshot: snapshot(Board::default(), stats, CellIndex::FIRST),
            })
            .unwrap();
        history.seek(0).unwrap();
        for decreased in [
            SearchStats { nodes: 4, ..stats },
            SearchStats {
                guesses: 4,
                ..stats
            },
            SearchStats {
                backtracks: 4,
                ..stats
            },
            SearchStats {
                forced_moves: 4,
                ..stats
            },
            SearchStats {
                max_depth: 4,
                ..stats
            },
        ] {
            assert_rejected(
                &mut history,
                TraceEvent::Step {
                    kind: TraceKind::Restore,
                    snapshot: snapshot(Board::default(), decreased, CellIndex::FIRST),
                },
                "search work decreases",
            );
        }
        // Recursion depth can decrease on restore; maximum depth cannot.
        history
            .push(TraceEvent::Step {
                kind: TraceKind::Restore,
                snapshot: snapshot(Board::default(), stats, CellIndex::FIRST),
            })
            .unwrap();
        assert_eq!(history.cursor(), 0);
    }

    #[test]
    fn solution_events_require_a_complete_board_and_finish_requires_a_start() {
        let mut history = started(Board::default());
        assert_rejected(
            &mut history,
            TraceEvent::Step {
                kind: TraceKind::Solution,
                snapshot: snapshot(Board::default(), SearchStats::default(), CellIndex::FIRST),
            },
            "solution snapshot is incomplete",
        );
        let mut history = TraceHistory::new(Board::default(), "test", HistoryLimits::default());
        assert_rejected(
            &mut history,
            TraceEvent::Finished {
                result: SearchResult::new(
                    &Board::default(),
                    None,
                    0,
                    SearchExtent::Exhausted,
                    SearchStats::default(),
                )
                .unwrap(),
                snapshot: snapshot(Board::default(), SearchStats::default(), CellIndex::FIRST),
            },
            "finish before start",
        );
    }

    #[test]
    fn finish_requires_the_results_board_and_all_counters_to_match() {
        let solved: Board = SOLUTION.parse().unwrap();
        let alternate = Board::try_from(solved.cells().map(|digit| match digit {
            1 => 2,
            2 => 1,
            digit => digit,
        }))
        .unwrap();
        let result = SearchResult::new(
            &Board::default(),
            Some(solved.clone()),
            1,
            SearchExtent::Exhausted,
            SearchStats::default(),
        )
        .unwrap();
        let mut history = started(Board::default());
        for (board, stats) in [
            (alternate, SearchStats::default()),
            (
                solved.clone(),
                SearchStats {
                    guesses: 1,
                    ..SearchStats::default()
                },
            ),
        ] {
            assert_rejected(
                &mut history,
                TraceEvent::Finished {
                    result: result.clone(),
                    snapshot: snapshot(board, stats, CellIndex::FIRST),
                },
                "finish does not match result",
            );
        }
        history
            .push(TraceEvent::Finished {
                result,
                snapshot: snapshot(solved, SearchStats::default(), CellIndex::FIRST),
            })
            .unwrap();
        assert!(history.is_complete());
    }

    #[test]
    fn byte_budget_includes_the_start_events_allocated_solver_name() {
        let mut solver = String::with_capacity(128);
        solver.push_str("test");
        let required = size_of::<HistoryEntry>() + solver.capacity();
        let event = TraceEvent::Start {
            solver,
            puzzle: Board::default(),
        };
        let limits =
            |bytes| HistoryLimits::new(NonZeroUsize::MIN, NonZeroUsize::new(bytes).unwrap());
        let mut too_small = TraceHistory::new(Board::default(), "test", limits(required - 1));
        assert_eq!(too_small.validate_next(&event), Ok(()));
        // Cloning a String may discard spare capacity, so move the original here.
        assert_eq!(too_small.push(event), Err(HistoryError::ByteLimit));
        assert!(too_small.is_empty());
        assert_eq!(too_small.capacity_bytes(), 0);

        let mut solver = String::with_capacity(128);
        solver.push_str("test");
        let mut exact = TraceHistory::new(Board::default(), "test", limits(required));
        exact
            .push(TraceEvent::Start {
                solver,
                puzzle: Board::default(),
            })
            .unwrap();
        assert_eq!(exact.capacity_bytes(), required);
    }
}
