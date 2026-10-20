use std::{fmt, str::FromStr};

/// A validated 9×9 Sudoku board in row-major order; zero denotes an empty cell.
///
/// Validation checks givens for conflicts, but does not imply solvability.
/// Use [`crate::Solver::search`] with a limit of two to determine uniqueness.
///
/// Fallible construction uses [`TryFrom`] or [`FromStr`]. Converting a board
/// back into its cells is infallible and uses [`From`] (and therefore [`Into`]).
///
/// ```
/// use ku_core::Board;
///
/// let mut cells = [0_u8; 81];
/// cells[0] = 7;
/// let board = Board::try_from(cells)?;
/// assert_eq!(board.cells()[0], 7);
/// let recovered: [u8; 81] = board.into();
/// assert_eq!(recovered, cells);
/// # Ok::<(), ku_core::BoardError>(())
/// ```
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Board {
    pub(crate) cells: [u8; 81],
}

/// Invalid syntax, digit values, or conflicting givens.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum BoardError {
    /// The input does not contain exactly 81 cells.
    #[error("expected 81 cells, found {0}")]
    CellCount(usize),
    /// A character other than an ASCII digit, dot, or whitespace was encountered.
    #[error("invalid character {character:?} at character offset {offset}")]
    InvalidCharacter {
        /// The rejected character.
        character: char,
        /// Its zero-based character offset in the input.
        offset: usize,
    },
    /// A numeric cell is outside the range 0..=9.
    #[error("cell {index} contains {digit}; expected a digit in 0..=9")]
    InvalidDigit {
        /// Zero-based cell index.
        index: usize,
        /// The rejected digit.
        digit: u8,
    },
    /// A digit appears twice in a row, column, or box.
    #[error("duplicate digit {digit} in {unit} {index} (zero-based)")]
    Duplicate {
        /// Row, column, or box.
        unit: &'static str,
        /// Zero-based unit index.
        index: usize,
        /// The duplicated digit.
        digit: u8,
    },
}

impl Board {
    // Complete grids need only a presence mask for each nine-cell unit: all
    // nine digits present means none can be duplicated. Range/givens validation
    // is separate so this compact check never exposes an unchecked Board.
    #[inline]
    pub(crate) fn solution_from_cells(puzzle: &Self, cells: [u8; 81]) -> Option<Self> {
        let mut invalid = 0_u8;
        for (&digit, &given) in cells.iter().zip(&puzzle.cells) {
            invalid |= u8::from(digit.wrapping_sub(1) > 8);
            invalid |= u8::from(given != 0 && given != digit);
        }
        if invalid != 0 {
            return None;
        }
        for unit in 0..9 {
            let mut row = 0_u16;
            let mut column = 0_u16;
            let mut square = 0_u16;
            for offset in 0..9 {
                row |= 1_u16 << cells[unit * 9 + offset];
                column |= 1_u16 << cells[offset * 9 + unit];
                square |=
                    1_u16 << cells[(unit / 3 * 3 + offset / 3) * 9 + unit % 3 * 3 + offset % 3];
            }
            if row != ALL_DIGITS || column != ALL_DIGITS || square != ALL_DIGITS {
                return None;
            }
        }
        Some(Self { cells })
    }

    /// Borrows the 81 row-major cells without permitting mutation of givens.
    pub fn cells(&self) -> &[u8; 81] {
        &self.cells
    }

    /// Returns the number of nonempty cells.
    pub fn clue_count(&self) -> usize {
        self.cells.iter().filter(|&&digit| digit != 0).count()
    }

    /// Whether this valid board is completely filled.
    pub fn is_solved(&self) -> bool {
        !self.cells.contains(&0)
    }

    /// Whether this complete board preserves every given in `puzzle`.
    pub fn is_solution_for(&self, puzzle: &Self) -> bool {
        self.is_solved()
            && self
                .cells
                .iter()
                .zip(&puzzle.cells)
                .all(|(&digit, &given)| given == 0 || digit == given)
    }
}

impl Default for Board {
    fn default() -> Self {
        Self { cells: [0; 81] }
    }
}

impl TryFrom<[u8; 81]> for Board {
    type Error = BoardError;

    fn try_from(cells: [u8; 81]) -> Result<Self, Self::Error> {
        let mut rows = [0_u16; 9];
        let mut columns = [0_u16; 9];
        let mut boxes = [0_u16; 9];
        for (index, &digit) in cells.iter().enumerate() {
            if digit > 9 {
                return Err(BoardError::InvalidDigit { index, digit });
            }
            if digit == 0 {
                continue;
            }
            let bit = 1 << digit;
            for (unit, unit_index, used) in [
                ("row", index / 9, &mut rows),
                ("column", index % 9, &mut columns),
                ("box", box_index(index), &mut boxes),
            ] {
                if used[unit_index] & bit != 0 {
                    return Err(BoardError::Duplicate {
                        unit,
                        index: unit_index,
                        digit,
                    });
                }
                used[unit_index] |= bit;
            }
        }
        Ok(Self { cells })
    }
}

impl TryFrom<&[u8]> for Board {
    type Error = BoardError;

    fn try_from(cells: &[u8]) -> Result<Self, Self::Error> {
        let array = <[u8; 81]>::try_from(cells).map_err(|_| BoardError::CellCount(cells.len()))?;
        Self::try_from(array)
    }
}

impl TryFrom<&str> for Board {
    type Error = BoardError;

    fn try_from(input: &str) -> Result<Self, Self::Error> {
        input.parse()
    }
}

impl From<Board> for [u8; 81] {
    fn from(board: Board) -> Self {
        board.cells
    }
}

impl AsRef<[u8; 81]> for Board {
    fn as_ref(&self) -> &[u8; 81] {
        &self.cells
    }
}

impl AsRef<[u8]> for Board {
    fn as_ref(&self) -> &[u8] {
        &self.cells
    }
}

impl FromStr for Board {
    type Err = BoardError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        let mut cells = [0; 81];
        let mut count = 0;
        for (offset, character) in input.chars().enumerate() {
            let digit = match character {
                '0' | '.' => 0,
                '1'..='9' => character as u8 - b'0',
                ch if ch.is_whitespace() => continue,
                _ => return Err(BoardError::InvalidCharacter { character, offset }),
            };
            if let Some(cell) = cells.get_mut(count) {
                *cell = digit;
            }
            count += 1;
        }
        if count != 81 {
            return Err(BoardError::CellCount(count));
        }
        Self::try_from(cells)
    }
}

impl fmt::Display for Board {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, &digit) in self.cells.iter().enumerate() {
            if f.alternate() && index != 0 {
                if index % 9 == 0 {
                    writeln!(f)?;
                    if index % 27 == 0 {
                        writeln!(f)?;
                    }
                } else {
                    f.write_str(if index % 3 == 0 { "  " } else { " " })?;
                }
            }
            write!(f, "{}", char::from(b'0' + digit))?;
        }
        Ok(())
    }
}

pub(crate) const fn box_index(cell: usize) -> usize {
    (cell / 27) * 3 + (cell % 9) / 3
}

pub(crate) const ALL_DIGITS: u16 = 0b11_1111_1110;

#[cfg(test)]
mod tests {
    use super::*;

    const PUZZLE: &str =
        "530070000600195000098000060800060003400803001700020006060000280000419005000080079";
    const SOLUTION: &str =
        "534678912672195348198342567859761423426853791713924856961537284287419635345286179";

    #[test]
    fn board_round_trips_through_standard_conversions() {
        let board: Board = PUZZLE.parse().unwrap();
        let cells: [u8; 81] = board.clone().into();
        assert_eq!(Board::try_from(cells).unwrap(), board);
        assert_eq!(Board::try_from(cells.as_slice()).unwrap(), board);
        assert_eq!(Board::try_from(PUZZLE).unwrap(), board);
        assert_eq!(board.cells(), &cells);
        assert_eq!(<Board as AsRef<[u8; 81]>>::as_ref(&board), &cells);
        assert_eq!(<Board as AsRef<[u8]>>::as_ref(&board), &cells);
    }

    #[test]
    fn compact_and_formatted_display_round_trip() {
        let board: Board = PUZZLE.parse().unwrap();
        assert_eq!(board.to_string(), PUZZLE);
        assert_eq!(format!("{board:#}").parse::<Board>().unwrap(), board);
        assert_eq!(PUZZLE.replace('0', ".").parse::<Board>().unwrap(), board);
    }

    #[test]
    fn short_and_long_inputs_report_actual_cell_counts() {
        for length in [0, 1, 80, 82, 1000] {
            assert_eq!(
                "0".repeat(length).parse::<Board>(),
                Err(BoardError::CellCount(length))
            );
            assert_eq!(
                Board::try_from(vec![0; length].as_slice()),
                Err(BoardError::CellCount(length))
            );
        }
    }

    #[test]
    fn parser_rejects_arbitrary_and_non_ascii_digits() {
        for character in ['x', '９', '١', '-', ','] {
            let input = format!("{character}{}", "0".repeat(80));
            assert_eq!(
                input.parse::<Board>(),
                Err(BoardError::InvalidCharacter {
                    character,
                    offset: 0
                })
            );
        }
    }

    #[test]
    fn numeric_conversion_rejects_digits_above_nine() {
        let mut cells = [0; 81];
        cells[80] = 255;
        assert_eq!(
            Board::try_from(cells),
            Err(BoardError::InvalidDigit {
                index: 80,
                digit: 255
            })
        );
    }

    #[test]
    fn each_kind_of_conflicting_given_is_rejected() {
        for (first, second, unit, index) in
            [(0, 8, "row", 0), (0, 72, "column", 0), (0, 10, "box", 0)]
        {
            let mut cells = [0; 81];
            cells[first] = 4;
            cells[second] = 4;
            assert_eq!(
                Board::try_from(cells),
                Err(BoardError::Duplicate {
                    unit,
                    index,
                    digit: 4
                })
            );
        }
    }

    #[test]
    fn a_complete_valid_grid_must_preserve_the_givens() {
        let puzzle: Board = PUZZLE.parse().unwrap();
        let solution: Board = SOLUTION.parse().unwrap();
        let swapped = SOLUTION
            .chars()
            .map(|ch| match ch {
                '1' => '2',
                '2' => '1',
                _ => ch,
            })
            .collect::<String>();
        let other: Board = swapped.parse().unwrap();
        assert!(solution.is_solution_for(&puzzle));
        assert!(!other.is_solution_for(&puzzle));
        assert!(!puzzle.is_solution_for(&puzzle));
        assert!(solution.is_solved());
        assert!(!Board::default().is_solved());
        assert_eq!(puzzle.clue_count(), 30);
    }

    #[test]
    fn board_accessors_and_empty_board_report_expected_values() {
        let board = Board::default();
        assert_eq!(board.cells(), &[0; 81]);
        assert_eq!(board.clue_count(), 0);
        assert!(!board.is_solved());
        assert!(!board.is_solution_for(&Board::default()));
    }

    #[test]
    fn parser_ignores_whitespace_and_reports_character_offsets() {
        let board = Board::try_from(format!(" \n{PUZZLE}\t").as_str()).unwrap();
        assert_eq!(board.to_string(), PUZZLE);

        let invalid = format!("0é{}", "0".repeat(80));
        assert_eq!(
            invalid.parse::<Board>(),
            Err(BoardError::InvalidCharacter {
                character: 'é',
                offset: 1,
            })
        );
    }

    #[test]
    fn alternate_display_uses_three_by_three_spacing() {
        let board: Board = SOLUTION.parse().unwrap();
        assert_eq!(
            format!("{board:#}"),
            concat!(
                "5 3 4  6 7 8  9 1 2\n",
                "6 7 2  1 9 5  3 4 8\n",
                "1 9 8  3 4 2  5 6 7\n",
                "\n",
                "8 5 9  7 6 1  4 2 3\n",
                "4 2 6  8 5 3  7 9 1\n",
                "7 1 3  9 2 4  8 5 6\n",
                "\n",
                "9 6 1  5 3 7  2 8 4\n",
                "2 8 7  4 1 9  6 3 5\n",
                "3 4 5  2 8 6  1 7 9"
            )
        );
    }

    #[test]
    fn box_index_maps_every_cell_to_its_three_by_three_box() {
        for cell in 0..81 {
            let row = cell / 9;
            let column = cell % 9;
            assert_eq!(box_index(cell), (row / 3) * 3 + column / 3);
        }
        assert_eq!(ALL_DIGITS, 0b11_1111_1110);
    }
}

/// An invalid domain value at an API boundary.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum DomainError {
    /// A cell index outside the board.
    #[error("cell index {0} is outside 0..81")]
    Cell(usize),
    /// A digit outside one through nine.
    #[error("digit {0} is outside 1..=9")]
    Digit(u8),
    /// Candidate bits outside the legal Sudoku digits.
    #[error("candidate mask {0:#x} contains invalid bits")]
    Candidates(u16),
}

/// A validated row-major index into an 81-cell board.
///
/// # Examples
///
/// ```
/// use ku_core::CellIndex;
///
/// let cell = CellIndex::try_from(40)?;
/// assert_eq!((cell.row(), cell.column()), (4, 4));
/// assert_eq!(CellIndex::FIRST.moved(-1, -1).get(), 80);
/// assert!(CellIndex::try_from(81).is_err());
/// # Ok::<(), ku_core::DomainError>(())
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CellIndex(u8);
impl CellIndex {
    /// The first cell, at row zero and column zero.
    pub const FIRST: Self = Self(0);
    /// Returns the zero-based row-major index.
    pub const fn get(self) -> usize {
        self.0 as usize
    }
    /// Returns the zero-based row.
    pub const fn row(self) -> usize {
        self.get() / 9
    }
    /// Returns the zero-based column.
    pub const fn column(self) -> usize {
        self.get() % 9
    }
    /// Moves by row and column offsets, wrapping independently at board edges.
    pub fn moved(self, rows: i8, columns: i8) -> Self {
        let row = (self.row() as i16 + i16::from(rows)).rem_euclid(9);
        let column = (self.column() as i16 + i16::from(columns)).rem_euclid(9);
        Self((row * 9 + column) as u8)
    }
}
impl TryFrom<usize> for CellIndex {
    type Error = DomainError;
    fn try_from(value: usize) -> Result<Self, Self::Error> {
        if value < 81 {
            Ok(Self(value as u8))
        } else {
            Err(DomainError::Cell(value))
        }
    }
}

/// A validated Sudoku digit from one through nine.
///
/// # Examples
///
/// ```
/// use ku_core::Digit;
///
/// let digit = Digit::try_from(9)?;
/// assert_eq!(digit.get(), 9);
/// // Clearing a cell uses `None` or raw zero at the game boundary, not a Digit.
/// assert!(Digit::try_from(0).is_err());
/// # Ok::<(), ku_core::DomainError>(())
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Digit(u8);
impl Digit {
    /// Returns the digit's numeric value.
    pub const fn get(self) -> u8 {
        self.0
    }
}
impl TryFrom<u8> for Digit {
    type Error = DomainError;
    fn try_from(value: u8) -> Result<Self, Self::Error> {
        if (1..=9).contains(&value) {
            Ok(Self(value))
        } else {
            Err(DomainError::Digit(value))
        }
    }
}

/// A set of valid candidate digits; bit `digit` denotes membership.
///
/// # Examples
///
/// ```
/// use ku_core::{CandidateMask, Digit};
///
/// let mask = CandidateMask::try_from((1_u16 << 2) | (1 << 7))?;
/// assert!(mask.contains(Digit::try_from(7)?));
/// assert_eq!(mask.iter().map(Digit::get).collect::<Vec<_>>(), [2, 7]);
/// assert_eq!(CandidateMask::default().bits(), 0);
/// assert!(CandidateMask::try_from(1_u16).is_err()); // Bit zero is invalid.
/// # Ok::<(), ku_core::DomainError>(())
/// ```
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct CandidateMask(u16);
impl CandidateMask {
    /// Returns the compact representation.
    pub const fn bits(self) -> u16 {
        self.0
    }
    /// Whether the set contains a digit.
    pub const fn contains(self, digit: Digit) -> bool {
        self.0 & (1 << digit.get()) != 0
    }
    /// Iterates present digits in ascending order.
    pub fn iter(self) -> impl Iterator<Item = Digit> {
        (1..=9)
            .filter(move |&digit| self.0 & (1 << digit) != 0)
            .map(Digit)
    }
}
impl TryFrom<u16> for CandidateMask {
    type Error = DomainError;
    fn try_from(bits: u16) -> Result<Self, Self::Error> {
        if bits & !ALL_DIGITS == 0 {
            Ok(Self(bits))
        } else {
            Err(DomainError::Candidates(bits))
        }
    }
}

#[cfg(test)]
mod domain_tests {
    use super::*;

    #[test]
    fn cell_index_converts_and_reports_row_and_column() {
        for (raw, row, column) in [(0, 0, 0), (8, 0, 8), (9, 1, 0), (40, 4, 4), (80, 8, 8)] {
            let cell = CellIndex::try_from(raw).unwrap();
            assert_eq!(cell.get(), raw);
            assert_eq!(cell.row(), row);
            assert_eq!(cell.column(), column);
        }
        assert_eq!(CellIndex::FIRST, CellIndex::try_from(0).unwrap());
        assert_eq!(CellIndex::try_from(81), Err(DomainError::Cell(81)));
        assert_eq!(
            CellIndex::try_from(usize::MAX),
            Err(DomainError::Cell(usize::MAX))
        );
    }

    #[test]
    fn cell_index_movement_wraps_rows_and_columns_independently() {
        let first = CellIndex::FIRST;
        assert_eq!(first.moved(-1, -1).get(), 80);
        assert_eq!(CellIndex::try_from(80).unwrap().moved(1, 1), first);
        assert_eq!(first.moved(18, -18), first);
        assert_eq!(first.moved(-128, 127).get(), 64);
    }

    #[test]
    fn digit_conversion_accepts_only_one_through_nine() {
        for value in 1..=9 {
            let digit = Digit::try_from(value).unwrap();
            assert_eq!(digit.get(), value);
        }
        for value in [0, 10, u8::MAX] {
            assert_eq!(Digit::try_from(value), Err(DomainError::Digit(value)));
        }
    }

    #[test]
    fn candidate_masks_validate_bits_and_iterate_digits_in_order() {
        let empty = CandidateMask::default();
        assert_eq!(empty.bits(), 0);
        assert_eq!(empty.iter().collect::<Vec<_>>(), []);
        for value in 1..=9 {
            assert!(!empty.contains(Digit::try_from(value).unwrap()));
        }

        let bits = (1 << 1) | (1 << 5) | (1 << 9);
        let mask = CandidateMask::try_from(bits).unwrap();
        assert_eq!(mask.bits(), bits);
        assert!(mask.contains(Digit::try_from(1).unwrap()));
        assert!(!mask.contains(Digit::try_from(2).unwrap()));
        assert!(mask.contains(Digit::try_from(5).unwrap()));
        assert!(mask.contains(Digit::try_from(9).unwrap()));
        assert_eq!(mask.iter().map(Digit::get).collect::<Vec<_>>(), [1, 5, 9]);

        let all = CandidateMask::try_from(ALL_DIGITS).unwrap();
        assert_eq!(
            all.iter().map(Digit::get).collect::<Vec<_>>(),
            (1..=9).collect::<Vec<_>>()
        );
        for value in [1, 1 << 10, u16::MAX] {
            assert_eq!(
                CandidateMask::try_from(value),
                Err(DomainError::Candidates(value))
            );
        }
    }
}
