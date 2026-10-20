//! Solver capabilities and validated search outcomes, independent of implementations.
use crate::{Board, TraceSink};
use std::num::NonZeroUsize;

/// A Sudoku search strategy. Implement this trait to supply another solver.
///
/// The interface is object-safe: callers may choose static dispatch with a
/// concrete solver or store heterogeneous strategies as `&dyn Solver`.
pub trait Solver: Send + Sync {
    /// A stable, unique name used by the registry, CLI, and benchmark labels.
    fn name(&self) -> &'static str;

    /// Enumerates at most `limit` solutions without changing the input board.
    ///
    /// A valid but unsatisfiable puzzle returns zero solutions. For a cap of
    /// two, zero, one, and two mean unsatisfiable, unique, and multiple. The
    /// first solution must be valid and preserve every given. The extent must
    /// be [`SearchExtent::Capped`] exactly when the count reaches the requested
    /// cap, and [`SearchExtent::Exhausted`] otherwise.
    fn search(&self, puzzle: &Board, limit: NonZeroUsize) -> SearchResult;

    /// Finds the first solution without establishing uniqueness.
    fn solve(&self, puzzle: &Board) -> Option<Board> {
        self.search(puzzle, NonZeroUsize::MIN).into_solution()
    }
}

/// A solver capable of reporting its actual search operations.
pub trait TraceSolver: Solver {
    /// Runs a traced search; a sink break during start or search returns progress
    /// without claiming completion. Once the terminal [`crate::TraceEvent::Finished`]
    /// is emitted, the search is complete regardless of the sink's response.
    fn trace_search(
        &self,
        puzzle: &Board,
        limit: NonZeroUsize,
        sink: &mut dyn TraceSink,
    ) -> TraceOutcome;
}

/// Search work; counts include work performed while proving uniqueness.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SearchStats {
    /// Recursive search calls, including completed and contradictory states.
    pub nodes: u64,
    /// Decisions at which more than one candidate was available.
    pub guesses: u64,
    /// Search calls that yielded no solutions.
    pub backtracks: u64,
    /// Assignments made with only one remaining choice, including propagation.
    pub forced_moves: u64,
    /// Maximum number of nested recursive calls below the root.
    pub max_depth: usize,
}

/// How far a completed enumeration searched.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SearchExtent {
    /// Every possible branch was examined.
    Exhausted,
    /// Search stopped after reaching this solution cap.
    Capped(NonZeroUsize),
}
/// What a completed search establishes about solution multiplicity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Multiplicity {
    /// Exhaustive search found no solution.
    Unsatisfiable,
    /// Exhaustive search found exactly one solution.
    Unique,
    /// At least two distinct solutions were found.
    Multiple,
    /// Search stopped at one solution before proving uniqueness.
    Undetermined,
}
/// An inconsistent search result supplied by an external strategy.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("solution, count, and search extent must agree and preserve the puzzle")]
pub struct SearchResultError;

/// A validated completed enumeration; cancellation uses [`SearchProgress`].
///
/// # Examples
///
/// ```
/// use std::num::NonZeroUsize;
/// use ku_core::{Board, Multiplicity, SearchExtent, SearchResult, SearchStats};
///
/// let solved: Board = concat!(
///     "534678912672195348198342567",
///     "859761423426853791713924856",
///     "961537284287419635345286179",
/// ).parse()?;
/// let capped = SearchResult::new(
///     &solved, Some(solved.clone()), 1,
///     SearchExtent::Capped(NonZeroUsize::MIN), SearchStats::default(),
/// )?;
/// assert_eq!(capped.multiplicity(), Multiplicity::Undetermined);
/// assert!(capped.unique_solution().is_none());
/// let exhausted = SearchResult::new(
///     &solved, Some(solved.clone()), 1, SearchExtent::Exhausted, SearchStats::default(),
/// )?;
/// assert_eq!(exhausted.unique_solution(), Some(&solved));
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
#[must_use = "inspect the search extent before inferring uniqueness"]
pub struct SearchResult {
    solution: Option<Board>,
    solutions_found: usize,
    stats: SearchStats,
    extent: SearchExtent,
}
impl SearchResult {
    /// Constructs a completed result after validating its data relationships.
    ///
    /// # Errors
    /// Returns [`SearchResultError`] when the first solution is invalid for the puzzle,
    /// solution presence and count disagree, or a capped count differs from its cap.
    pub fn new(
        puzzle: &Board,
        solution: Option<Board>,
        solutions_found: usize,
        extent: SearchExtent,
        stats: SearchStats,
    ) -> Result<Self, SearchResultError> {
        if solution.is_some() != (solutions_found > 0)
            || solution
                .as_ref()
                .is_some_and(|solution| !solution.is_solution_for(puzzle))
            || matches!(extent, SearchExtent::Capped(cap) if solutions_found != cap.get())
        {
            return Err(SearchResultError);
        }
        Ok(Self {
            solution,
            solutions_found,
            stats,
            extent,
        })
    }
    /// Constructs a completed result directly from a solver's raw first solution.
    ///
    /// Checks the complete grid, preservation of givens, and result metadata in
    /// one boundary conversion. This avoids validating an intermediate board.
    ///
    /// # Errors
    ///
    /// Returns [`SearchResultError`] if the solution is incomplete, invalid, or
    /// changes a given, or if the solution presence, count, and cap disagree.
    #[inline(always)]
    pub fn try_from_cells(
        puzzle: &Board,
        solution: Option<[u8; 81]>,
        solutions_found: usize,
        extent: SearchExtent,
        stats: SearchStats,
    ) -> Result<Self, SearchResultError> {
        if solution.is_some() != (solutions_found > 0)
            || matches!(extent, SearchExtent::Capped(cap) if solutions_found != cap.get())
        {
            return Err(SearchResultError);
        }
        let solution = solution
            .map(|cells| Board::solution_from_cells(puzzle, cells).ok_or(SearchResultError))
            .transpose()?;
        Ok(Self {
            solution,
            solutions_found,
            stats,
            extent,
        })
    }
    /// Borrows the first solution, if one was found.
    pub fn solution(&self) -> Option<&Board> {
        self.solution.as_ref()
    }
    /// Transfers ownership of the first solution.
    pub fn into_solution(self) -> Option<Board> {
        self.solution
    }
    /// Returns the number of distinct solutions found.
    pub fn solutions_found(&self) -> usize {
        self.solutions_found
    }
    /// Whether the search reached its solution cap.
    pub fn limit_reached(&self) -> bool {
        matches!(self.extent, SearchExtent::Capped(_))
    }
    /// Returns accumulated work.
    pub fn stats(&self) -> SearchStats {
        self.stats
    }
    /// Returns the extent of completed search.
    pub fn extent(&self) -> SearchExtent {
        self.extent
    }
    /// Returns only what this search proves about multiplicity.
    pub fn multiplicity(&self) -> Multiplicity {
        match (self.solutions_found, self.extent) {
            (0, _) => Multiplicity::Unsatisfiable,
            (1, SearchExtent::Exhausted) => Multiplicity::Unique,
            (1, SearchExtent::Capped(_)) => Multiplicity::Undetermined,
            _ => Multiplicity::Multiple,
        }
    }
    /// Borrows a solution only when completed search proves it unique.
    pub fn unique_solution(&self) -> Option<&Board> {
        (self.multiplicity() == Multiplicity::Unique)
            .then_some(self.solution.as_ref())
            .flatten()
    }
}
/// Partial counters from an interrupted enumeration; they establish no uniqueness.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[must_use = "interrupted progress does not establish uniqueness"]
pub struct SearchProgress {
    solution: Option<Board>,
    solutions_found: usize,
    stats: SearchStats,
}
impl SearchProgress {
    /// Constructs interrupted progress after validating its optional solution.
    ///
    /// # Errors
    /// Returns [`SearchResultError`] if solution presence and count disagree or the
    /// supplied solution is invalid for the puzzle.
    pub fn new(
        puzzle: &Board,
        solution: Option<Board>,
        solutions_found: usize,
        stats: SearchStats,
    ) -> Result<Self, SearchResultError> {
        if solution.is_some() != (solutions_found > 0)
            || solution
                .as_ref()
                .is_some_and(|solution| !solution.is_solution_for(puzzle))
        {
            return Err(SearchResultError);
        }
        Ok(Self {
            solution,
            solutions_found,
            stats,
        })
    }
    /// Borrows the first solution found before cancellation.
    pub fn solution(&self) -> Option<&Board> {
        self.solution.as_ref()
    }
    /// Returns the number found before cancellation.
    pub fn solutions_found(&self) -> usize {
        self.solutions_found
    }
    /// Returns work accumulated before cancellation.
    pub fn stats(&self) -> SearchStats {
        self.stats
    }
}
/// Completion or cancellation of an observable search.
#[derive(Clone, Debug, PartialEq, Eq)]
#[must_use = "traced searches may be cancelled before completion"]
pub enum TraceOutcome {
    /// The enumeration completed under its configured cap.
    Completed(SearchResult),
    /// The sink requested interruption before completion.
    Cancelled(SearchProgress),
}
