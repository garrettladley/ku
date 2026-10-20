//! Sudoku search strategies and reproducible, uniqueness-checked generation.
//!
//! Domain values and extensible solver contracts live in [`ku_core`].
//! Use [`solver_registry`] to discover strategies and their tracing capabilities,
//! or implement [`ku_core::Solver`] to supply a custom strategy.
//!
//! # Solving and proving uniqueness
//!
//! A first solution does not prove uniqueness. Search with a cap of two and
//! inspect [`ku_core::SearchResult::unique_solution`] for that guarantee.
//!
//! ```
//! use ku_core::{Board, Solver};
//! use ku_solvers::Bitmask;
//! let puzzle = Board::default();
//! let solution = Bitmask.solve(&puzzle).ok_or("no solution")?;
//! assert!(solution.is_solution_for(&puzzle));
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! ```
//! use std::num::NonZeroUsize;
//! use ku_core::{Board, Multiplicity, Solver};
//! use ku_solvers::Bitmask;
//!
//! let puzzle: Board = concat!(
//!     "530070000600195000098000060",
//!     "800060003400803001700020006",
//!     "060000280000419005000080079",
//! ).parse()?;
//! let result = Bitmask.search(&puzzle, NonZeroUsize::MIN.saturating_add(1));
//! assert_eq!(result.multiplicity(), Multiplicity::Unique);
//! assert!(result.unique_solution().unwrap().is_solution_for(&puzzle));
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! # Recording actual solver events
//!
//! A closure implements [`ku_core::TraceSink`]. Here the sink retains validated
//! events while the solver runs; replay later needs no solver.
//!
//! ```
//! use std::{num::NonZeroUsize, ops::ControlFlow};
//! use ku_core::{Board, HistoryLimits, TraceHistory, TraceOutcome, TraceSolver};
//! use ku_solvers::Bitmask;
//!
//! let puzzle = Board::default();
//! let mut history = TraceHistory::new(puzzle.clone(), "bitmask", HistoryLimits::default());
//! let outcome = Bitmask.trace_search(&puzzle, NonZeroUsize::MIN, &mut |event| {
//!     history.push(event).unwrap();
//!     ControlFlow::Continue(())
//! });
//! assert!(matches!(outcome, TraceOutcome::Completed(_)));
//! history.seek(history.len())?;
//! assert!(history.state().finished().is_some());
//! assert!(history.state().snapshot().board().is_solution_for(&puzzle));
//! # Ok::<(), ku_core::HistoryError>(())
//! ```
//!
//! Returning a break requests cooperative cancellation without claiming a
//! completed search or establishing uniqueness:
//!
//! ```
//! use std::{num::NonZeroUsize, ops::ControlFlow};
//! use ku_core::{Board, TraceOutcome, TraceSolver};
//! use ku_solvers::Bitmask;
//!
//! let outcome = Bitmask.trace_search(&Board::default(), NonZeroUsize::MIN,
//!     &mut |_| ControlFlow::Break(()));
//! assert!(matches!(outcome, TraceOutcome::Cancelled(progress)
//!     if progress.solutions_found() == 0));
//! ```
//!
//! # Generation
//!
//! [`generate`] uses the default strategies. [`Generator`] accepts independent
//! grid-construction and uniqueness-checking capabilities. [`MinClues`] is a
//! lower bound on clues, so generation can return more clues than requested.

mod algorithms;
mod generator;
mod registry;

use ku_core::{
    Board, CandidateMask, CellIndex, Digit, SearchExtent, SearchProgress, SearchResult,
    SearchStats, Solver, TraceEvent, TraceKind, TraceOutcome, TraceSink, TraceSnapshot,
    TraceSolver,
};

pub use algorithms::{
    Backtracking, Bitmask, BitsetCover, ConstraintPropagation, DancingLinks, ForwardChecking,
};
pub use generator::{
    GenerateError, GeneratedPuzzle, Generator, GridGenerator, MinClues, MinCluesError, generate,
    generate_with_solver,
};
pub use registry::{SolverEntry, built_in_solvers, built_in_trace_solvers, solver_registry};
