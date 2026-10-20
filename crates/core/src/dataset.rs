//! Read compact puzzle lines, unquoted `puzzle,solution` CSV, or Sudoku Exchange
//! `hash puzzle rating` records. CSV solutions are validated against the givens.
//! Empty lines and lines starting with `#` are ignored.

use crate::{Board, BoardError};

/// A puzzle with an optional expected complete solution.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Record {
    /// Validated starting board.
    puzzle: Board,
    /// Validated complete solution preserving every given, when supplied.
    solution: Option<Board>,
}

/// A supplied expected solution violates the puzzle contract.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("expected solution must be complete and preserve every given")]
pub struct RecordError;

impl Record {
    /// Constructs a record after validating an optional expected solution.
    ///
    /// # Errors
    /// Rejects an incomplete expected solution or one that changes a puzzle given.
    pub fn new(puzzle: Board, solution: Option<Board>) -> Result<Self, RecordError> {
        if solution
            .as_ref()
            .is_some_and(|solution| !solution.is_solution_for(&puzzle))
        {
            return Err(RecordError);
        }
        Ok(Self { puzzle, solution })
    }
    /// Returns the validated starting board.
    pub fn puzzle(&self) -> &Board {
        &self.puzzle
    }
    /// Returns the expected complete solution, when supplied.
    pub fn solution(&self) -> Option<&Board> {
        self.solution.as_ref()
    }
    /// Transfers ownership of the puzzle and optional solution.
    pub fn into_parts(self) -> (Board, Option<Board>) {
        (self.puzzle, self.solution)
    }
}
impl From<Board> for Record {
    fn from(puzzle: Board) -> Self {
        Self {
            puzzle,
            solution: None,
        }
    }
}

/// A malformed dataset record, with a one-based source line number.
#[derive(Debug, thiserror::Error)]
pub enum DatasetError {
    /// Invalid puzzle or solution syntax.
    #[error("line {line}: {source}")]
    Board {
        /// One-based line number.
        line: usize,
        /// Underlying board error.
        #[source]
        source: BoardError,
    },
    /// CSV contains other than two fields, or an Exchange record is malformed.
    #[error("line {0}: expected a puzzle, puzzle/solution CSV pair, or hash/puzzle/rating record")]
    Format(usize),
    /// A supplied solution is incomplete or changes a given.
    #[error("line {0}: expected solution must be complete and preserve every given")]
    InvalidSolution(usize),
}

/// Parses a dataset into validated records.
///
/// CSV is intentionally restricted to two unquoted ASCII board fields, with an
/// optional `puzzle,solution` header. `quizzes,solutions` is also accepted for
/// common Kaggle exports. An empty solution field denotes a puzzle-only record.
///
/// # Errors
/// Returns [`DatasetError`] identifying the first malformed source line.
///
/// # Examples
///
/// ```
/// use ku_core::dataset::{self, DatasetError};
///
/// let blank = ".".repeat(81);
/// let csv = format!("# optional solution\npuzzle,solution\n{blank},\n");
/// let records = dataset::parse(&csv)?;
/// assert_eq!(records.len(), 1);
/// assert_eq!(records[0].puzzle().clue_count(), 0);
/// assert!(records[0].solution().is_none());
/// let error = dataset::parse("# source comment\ninvalid").unwrap_err();
/// assert!(matches!(error, DatasetError::Board { line: 2, .. }));
/// # Ok::<(), DatasetError>(())
/// ```
pub fn parse(input: &str) -> Result<Vec<Record>, DatasetError> {
    let mut records = Vec::new();
    for (index, line) in input.lines().enumerate() {
        let line_number = index + 1;
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if records.is_empty() && matches!(line, "puzzle,solution" | "quizzes,solutions") {
            continue;
        }
        let (puzzle, solution) = if let Some((puzzle, solution)) = line.split_once(',') {
            if solution.contains(',') {
                return Err(DatasetError::Format(line_number));
            }
            (
                puzzle.trim(),
                Some(solution.trim()).filter(|value| !value.is_empty()),
            )
        } else {
            let mut fields = line.split_whitespace();
            match (fields.next(), fields.next(), fields.next(), fields.next()) {
                (Some(hash), Some(puzzle), Some(rating), None)
                    if hash.len() == 12
                        && hash.bytes().all(|ch| ch.is_ascii_hexdigit())
                        && rating.parse::<f32>().is_ok() =>
                {
                    (puzzle, None)
                }
                _ => (line, None),
            }
        };
        let parse_board = |value: &str| {
            value
                .parse::<Board>()
                .map_err(|source| DatasetError::Board {
                    line: line_number,
                    source,
                })
        };
        let puzzle = parse_board(puzzle)?;
        let solution = solution.map(parse_board).transpose()?;
        records.push(
            Record::new(puzzle, solution)
                .map_err(|_| DatasetError::InvalidSolution(line_number))?,
        );
    }
    Ok(records)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PUZZLE: &str =
        "530070000600195000098000060800060003400803001700020006060000280000419005000080079";
    const SOLUTION: &str =
        "534678912672195348198342567859761423426853791713924856961537284287419635345286179";

    #[test]
    fn supported_dataset_formats_produce_equivalent_puzzles() {
        for input in [
            format!("# comment\n\n{PUZZLE}\n"),
            format!("puzzle,solution\n{PUZZLE},{SOLUTION}\n"),
            format!("quizzes,solutions\r\n{PUZZLE},{SOLUTION}\r\n"),
            format!("012345abcdef {PUZZLE} 1.2\n"),
            format!("{PUZZLE},\n"),
        ] {
            let records = parse(&input).unwrap();
            assert_eq!(records.len(), 1);
            assert_eq!(records[0].puzzle.to_string(), PUZZLE);
        }
    }

    #[test]
    fn malformed_boards_report_original_line_numbers() {
        let error = parse("# comment\n\npuzzle,solution\n123,\n").unwrap_err();
        assert!(matches!(
            error,
            DatasetError::Board {
                line: 4,
                source: BoardError::CellCount(3)
            }
        ));
    }

    #[test]
    fn extra_csv_columns_are_rejected() {
        assert!(matches!(
            parse(&format!("{PUZZLE},{SOLUTION},extra")),
            Err(DatasetError::Format(1))
        ));
    }

    #[test]
    fn incomplete_or_changed_expected_solutions_are_rejected() {
        let swapped = SOLUTION
            .chars()
            .map(|ch| match ch {
                '1' => '2',
                '2' => '1',
                _ => ch,
            })
            .collect::<String>();
        for solution in [PUZZLE, swapped.as_str()] {
            assert!(matches!(
                parse(&format!("{PUZZLE},{solution}")),
                Err(DatasetError::InvalidSolution(1))
            ));
        }
    }

    #[test]
    fn empty_dataset_is_distinct_from_a_malformed_record() {
        assert!(parse("# nothing here\n\n").unwrap().is_empty());
    }
}
