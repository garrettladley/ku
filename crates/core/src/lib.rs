//! Validated Sudoku domain types and implementation-independent solver contracts.
//!
//! Boards validate digits and givens at construction. Solvers leave the input
//! unchanged and distinguish a valid but unsatisfiable puzzle from invalid input.
//!
//! ```
//! use ku_core::Board;
//!
//! let puzzle: Board = concat!(
//!     "530070000600195000098000060",
//!     "800060003400803001700020006",
//!     "060000280000419005000080079",
//! ).parse()?;
//! assert_eq!(puzzle.clue_count(), 30);
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

mod board;
pub mod dataset;
mod game;
mod search;
mod trace;

pub use board::{Board, BoardError, CandidateMask, CellIndex, Digit, DomainError};
pub use dataset::{Record, RecordError};
pub use game::{Game, GameError, GameEvent};
pub use search::{
    Multiplicity, SearchExtent, SearchProgress, SearchResult, SearchResultError, SearchStats,
    Solver, TraceOutcome, TraceSolver,
};
pub use trace::{
    HistoryError, HistoryLimits, TraceEvent, TraceHistory, TraceKind, TraceReplay, TraceSink,
    TraceSnapshot, TraceState,
};
