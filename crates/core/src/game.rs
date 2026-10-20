use crate::{Board, BoardError, CellIndex, Digit};

/// A serializable-style change record using raw cell and digit values.
///
/// Events returned by [`Game::edit`] are accepted changes. Values constructed
/// externally are untrusted until [`Game::apply`] checks the cell, values,
/// protected givens, and expected prior state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GameEvent {
    /// Replaces one editable cell; zero denotes an empty cell.
    CellChanged {
        /// Zero-based row-major cell index.
        cell: usize,
        /// Value immediately before the change.
        before: u8,
        /// Value immediately after the change.
        after: u8,
    },
}

/// A rejected move or inconsistent recorded event.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum GameError {
    /// The supplied cell is outside the board.
    #[error("cell {cell} is outside the 81-cell board")]
    InvalidCell {
        /// Rejected zero-based cell index.
        cell: usize,
    },
    /// The selected cell contains an original clue.
    #[error("r{}c{} is a given clue and cannot be changed", .cell / 9 + 1, .cell % 9 + 1)]
    Given {
        /// Zero-based cell index of the protected clue.
        cell: usize,
    },
    /// The resulting board has an invalid digit or a Sudoku conflict.
    #[error("{}", describe_board_error(.0))]
    Board(#[from] BoardError),
    /// A recorded event does not match the current board.
    #[error("cell {cell} contains {actual}, but the event expects {expected}")]
    ReplayMismatch {
        /// Zero-based row-major cell index.
        cell: usize,
        /// Value required by the recorded event.
        expected: u8,
        /// Value currently in the board.
        actual: u8,
    },
    /// Recorded history contains a change that does not change a cell.
    #[error("recorded event for cell {cell} does not change its value")]
    UnchangedEvent {
        /// Zero-based row-major cell index.
        cell: usize,
    },
}

fn describe_board_error(error: &BoardError) -> String {
    match error {
        BoardError::Duplicate { unit, index, digit } => {
            format!("digit {digit} conflicts in {unit} {}", index + 1)
        }
        BoardError::InvalidDigit { index, digit } => {
            format!(
                "cell {} contains {digit}; expected a digit in 0..=9",
                index + 1
            )
        }
        error => error.to_string(),
    }
}

/// A playable Sudoku board with protected givens and an accepted-move history.
///
/// Every accepted state satisfies Sudoku's row, column and box constraints.
/// A move can still make a puzzle unsatisfiable; this type does not consult a
/// solver or compare entries with an expected solution.
///
/// # Examples
///
/// ```
/// use ku_core::{Board, CellIndex, Digit, Game};
///
/// let puzzle = Board::default();
/// let mut game = Game::new(puzzle.clone());
/// assert!(game.edit_cell(CellIndex::FIRST, Some(Digit::try_from(5)?))?.is_some());
/// assert!(game.edit(0, 5)?.is_none()); // Unchanged cells add no history entry.
/// assert!(game.edit(1, 5).is_err()); // Duplicate in the same row.
/// assert_eq!(game.events().len(), 1);
/// let replayed = Game::replay(puzzle, game.events())?;
/// assert_eq!(replayed.board(), game.board());
/// assert!(game.edit_cell(CellIndex::FIRST, None)?.is_some()); // Clear the cell.
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Game {
    puzzle: Board,
    board: Board,
    events: Vec<GameEvent>,
}

impl Game {
    /// Starts a game from a validated puzzle without changing its clues.
    pub fn new(puzzle: Board) -> Self {
        Self {
            board: puzzle.clone(),
            puzzle,
            events: Vec::new(),
        }
    }

    /// Returns the original puzzle, whose nonzero cells remain protected.
    pub fn puzzle(&self) -> &Board {
        &self.puzzle
    }

    /// Returns the current validated player board.
    pub fn board(&self) -> &Board {
        &self.board
    }

    /// Returns accepted edits in chronological order.
    pub fn events(&self) -> &[GameEvent] {
        &self.events
    }

    /// Whether the current board completely solves the original puzzle.
    pub fn is_complete(&self) -> bool {
        self.board.is_solution_for(&self.puzzle)
    }

    /// Enters a validated digit or clears an editable cell.
    ///
    /// # Errors
    /// Rejects changes to original givens and moves creating row, column, or box
    /// conflicts. Rejected edits leave the board and history unchanged.
    pub fn edit_cell(
        &mut self,
        cell: CellIndex,
        digit: Option<Digit>,
    ) -> Result<Option<GameEvent>, GameError> {
        self.edit(cell.get(), digit.map_or(0, Digit::get))
    }

    /// Enters or clears a cell, recording only accepted changes.
    ///
    /// Returns `None` when an editable cell already contains `digit`. Zero
    /// clears a cell. Invalid moves leave both board and history unchanged.
    ///
    /// # Errors
    /// Rejects out-of-range cells, digits above nine, changes to original
    /// clues, and row, column or box conflicts.
    pub fn edit(&mut self, cell: usize, digit: u8) -> Result<Option<GameEvent>, GameError> {
        let before = self.value(cell)?;
        if digit > 9 {
            return Err(BoardError::InvalidDigit { index: cell, digit }.into());
        }
        if self.puzzle.cells()[cell] != 0 {
            return Err(GameError::Given { cell });
        }
        if before == digit {
            return Ok(None);
        }
        let mut cells = *self.board.cells();
        cells[cell] = digit;
        let board = Board::try_from(cells)?;
        let event = GameEvent::CellChanged {
            cell,
            before,
            after: digit,
        };
        self.board = board;
        self.events.push(event);
        Ok(Some(event))
    }

    /// Applies and records one validated event from a game history.
    ///
    /// # Errors
    /// Rejects mismatched previous values, unchanged events, and events that
    /// would violate the same rules as [`Self::edit`]. Rejected events leave
    /// the game unchanged.
    pub fn apply(&mut self, event: &GameEvent) -> Result<(), GameError> {
        let GameEvent::CellChanged {
            cell,
            before,
            after,
        } = *event;
        let actual = self.value(cell)?;
        if actual != before {
            return Err(GameError::ReplayMismatch {
                cell,
                expected: before,
                actual,
            });
        }
        if before == after {
            return Err(GameError::UnchangedEvent { cell });
        }
        self.edit(cell, after)?;
        Ok(())
    }

    /// Reconstructs a game by applying events to its original puzzle.
    ///
    /// Replaying a prefix reconstructs the board at that point in history.
    /// No solver, random source or wall clock participates in replay.
    ///
    /// # Errors
    /// Returns the first event error reported by [`Self::apply`].
    pub fn replay(puzzle: Board, events: &[GameEvent]) -> Result<Self, GameError> {
        let mut game = Self::new(puzzle);
        for event in events {
            game.apply(event)?;
        }
        Ok(game)
    }

    fn value(&self, cell: usize) -> Result<u8, GameError> {
        self.board
            .cells()
            .get(cell)
            .copied()
            .ok_or(GameError::InvalidCell { cell })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PUZZLE: &str =
        "530070000600195000098000060800060003400803001700020006060000280000419005000080079";
    const SOLUTION: &str =
        "534678912672195348198342567859761423426853791713924856961537284287419635345286179";

    #[test]
    fn edits_replace_clear_and_record_only_changes() {
        let puzzle = PUZZLE.parse().unwrap();
        let mut game = Game::new(puzzle);
        assert_eq!(game.edit(2, 0), Ok(None));
        assert_eq!(
            game.edit(2, 4),
            Ok(Some(GameEvent::CellChanged {
                cell: 2,
                before: 0,
                after: 4,
            }))
        );
        assert_eq!(game.edit(2, 4), Ok(None));
        game.edit(2, 1).unwrap();
        game.edit(2, 0).unwrap();
        assert_eq!(game.board(), game.puzzle());
        assert_eq!(game.puzzle().to_string(), PUZZLE);
        assert_eq!(game.events().len(), 3);
    }

    #[test]
    fn rejected_edits_leave_board_and_history_unchanged() {
        let mut game = Game::new(PUZZLE.parse().unwrap());
        for (cell, digit) in [(0, 0), (0, 5), (81, 1), (usize::MAX, 1), (2, 10), (2, 5)] {
            let before = game.clone();
            assert!(game.edit(cell, digit).is_err());
            assert_eq!(game, before);
        }
    }

    #[test]
    fn row_column_and_box_conflicts_are_rejected() {
        for (first, second, unit) in [(0, 8, "row"), (0, 72, "column"), (0, 10, "box")] {
            let mut game = Game::new(Board::default());
            game.edit(first, 4).unwrap();
            assert!(matches!(
                game.edit(second, 4),
                Err(GameError::Board(BoardError::Duplicate { unit: found, .. })) if found == unit
            ));
            assert_eq!(game.board().cells()[second], 0);
            assert_eq!(game.events().len(), 1);
        }
    }

    #[test]
    fn move_errors_use_human_coordinates() {
        let mut game = Game::new(PUZZLE.parse().unwrap());
        assert_eq!(
            game.edit(0, 0).unwrap_err().to_string(),
            "r1c1 is a given clue and cannot be changed"
        );
        assert_eq!(
            game.edit(2, 5).unwrap_err().to_string(),
            "digit 5 conflicts in row 1"
        );
        assert_eq!(
            game.edit(2, 10).unwrap_err().to_string(),
            "cell 3 contains 10; expected a digit in 0..=9"
        );
    }

    #[test]
    fn replay_reconstructs_each_prefix_and_rejects_stale_events() {
        let mut game = Game::new(PUZZLE.parse().unwrap());
        let mut boards = vec![game.board().clone()];
        for digit in [4, 1, 0, 4] {
            game.edit(2, digit).unwrap();
            boards.push(game.board().clone());
        }
        for (length, board) in boards.iter().enumerate() {
            let replay = Game::replay(game.puzzle().clone(), &game.events()[..length]).unwrap();
            assert_eq!(replay.board(), board);
            assert_eq!(replay.events(), &game.events()[..length]);
        }
        assert_eq!(
            Game::replay(game.puzzle().clone(), game.events()).unwrap(),
            game
        );
        let before = game.clone();
        assert_eq!(
            game.apply(&GameEvent::CellChanged {
                cell: 2,
                before: 0,
                after: 1,
            }),
            Err(GameError::ReplayMismatch {
                cell: 2,
                expected: 0,
                actual: 4,
            })
        );
        assert_eq!(game, before);
    }

    #[test]
    fn replay_rejects_invalid_and_protected_changes() {
        let mut game = Game::new(PUZZLE.parse().unwrap());
        for event in [
            GameEvent::CellChanged {
                cell: 81,
                before: 0,
                after: 1,
            },
            GameEvent::CellChanged {
                cell: 0,
                before: 5,
                after: 0,
            },
            GameEvent::CellChanged {
                cell: 2,
                before: 0,
                after: 10,
            },
            GameEvent::CellChanged {
                cell: 2,
                before: 0,
                after: 5,
            },
            GameEvent::CellChanged {
                cell: 2,
                before: 0,
                after: 0,
            },
        ] {
            let before = game.clone();
            assert!(game.apply(&event).is_err());
            assert_eq!(game, before);
        }
    }

    #[test]
    fn completion_is_derived_and_clearing_resumes_the_game() {
        let solution: Board = SOLUTION.parse().unwrap();
        let mut cells = *solution.cells();
        cells[2] = 0;
        let mut game = Game::new(Board::try_from(cells).unwrap());
        assert!(!game.is_complete());
        game.edit(2, 4).unwrap();
        assert!(game.is_complete());
        game.edit(2, 0).unwrap();
        assert!(!game.is_complete());
        assert!(Game::new(solution).is_complete());
    }
}
