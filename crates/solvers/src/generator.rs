use std::{fmt, num::NonZeroUsize, str::FromStr};

use rand::{Rng, SeedableRng, rngs::SmallRng, seq::SliceRandom};

use crate::{Bitmask, Board, Solver};

/// A validated generation clue floor in 17..=81; defaults to 32.
///
/// For standard 9×9 Sudoku, 17 givens is the proven minimum for a unique
/// solution; no 16-clue puzzle is unique ([proof](https://arxiv.org/abs/1201.0749)).
///
/// This is a lower bound on retained clues, not a difficulty rating.
/// Validation happens when converting input, before generation begins.
///
/// ```
/// use ku_solvers::{MinClues, generate};
///
/// let floor = MinClues::try_from(32)?;
/// let generated = generate(42, floor)?;
/// assert!(generated.puzzle().clue_count() >= floor.get());
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
///
/// Raw integers cannot be passed directly to generation:
///
/// ```compile_fail
/// use ku_solvers::generate;
/// let _ = generate(42, 16);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MinClues(u8);

impl MinClues {
    /// The smallest permitted clue floor: 17.
    pub const MIN: Self = Self(17);
    /// The largest permitted clue floor: 81 (a complete grid).
    pub const MAX: Self = Self(81);

    /// Returns the validated lower bound as a cell count.
    pub const fn get(self) -> usize {
        self.0 as usize
    }
}

impl Default for MinClues {
    fn default() -> Self {
        Self(32)
    }
}

/// Input that cannot represent a generation clue floor in 17..=81.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("minimum clue count must be an integer in 17..=81")]
pub struct MinCluesError;

impl TryFrom<usize> for MinClues {
    type Error = MinCluesError;

    fn try_from(value: usize) -> Result<Self, Self::Error> {
        if (Self::MIN.get()..=Self::MAX.get()).contains(&value) {
            Ok(Self(value as u8))
        } else {
            Err(MinCluesError)
        }
    }
}

impl FromStr for MinClues {
    type Err = MinCluesError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        Self::try_from(input.parse::<usize>().map_err(|_| MinCluesError)?)
    }
}

impl From<MinClues> for usize {
    fn from(value: MinClues) -> Self {
        value.get()
    }
}

impl fmt::Display for MinClues {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// A generated puzzle and its unique completed solution.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GeneratedPuzzle {
    /// The puzzle after uniqueness-preserving clue removal.
    puzzle: Board,
    /// The complete grid used to create the puzzle.
    solution: Board,
}

impl GeneratedPuzzle {
    /// Returns the generated unique puzzle.
    pub fn puzzle(&self) -> &Board {
        &self.puzzle
    }
    /// Returns its complete solution.
    pub fn solution(&self) -> &Board {
        &self.solution
    }
    /// Transfers ownership of the puzzle and solution.
    pub fn into_parts(self) -> (Board, Board) {
        (self.puzzle, self.solution)
    }
}

/// A grid construction or uniqueness-checking error.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum GenerateError {
    /// The grid generator failed to produce a complete Sudoku grid.
    #[error("grid generator did not produce a complete Sudoku grid")]
    ConstructionFailed,
    /// A supplied uniqueness checker returned data inconsistent with the grid.
    #[error("uniqueness checker returned an inconsistent solution or search extent")]
    InvalidSearchResult,
}

/// A capability for constructing randomized complete Sudoku grids.
///
/// Implementations use the caller's RNG and return a complete, valid [`Board`],
/// or `None` if construction fails. For a fixed implementation and RNG state,
/// construction must be deterministic; grids need not be sampled uniformly.
/// This capability is independent of [`Solver`] and uses static dispatch over
/// the RNG. [`Bitmask`], [`crate::Backtracking`], and [`crate::ForwardChecking`]
/// implement it.
pub trait GridGenerator: Send + Sync {
    /// Constructs a complete grid, consuming randomness from the supplied RNG.
    fn generate_grid(&self, rng: &mut (impl Rng + ?Sized)) -> Option<Board>;
}

/// Puzzle generation with independently supplied grid and uniqueness strategies.
///
/// Borrows both strategies without allocating or requiring them to be cloned.
/// The uniqueness checker may be a concrete type or a `dyn Solver`.
///
/// ```
/// use ku_solvers::{DancingLinks, ForwardChecking, Generator, MinClues};
///
/// let generator = Generator::new(&ForwardChecking, &DancingLinks);
/// let generated = generator.generate(42, MinClues::default())?;
/// assert!(generated.solution().is_solution_for(generated.puzzle()));
/// # Ok::<(), ku_solvers::GenerateError>(())
/// ```
///
/// A solver without randomized grid construction cannot fill that role:
///
/// ```compile_fail
/// use ku_solvers::{Bitmask, DancingLinks, Generator};
/// let generator = Generator::new(&DancingLinks, &Bitmask);
/// ```
#[derive(Debug)]
pub struct Generator<'a, G: ?Sized, S: ?Sized> {
    grid_generator: &'a G,
    uniqueness_checker: &'a S,
}

impl<'a, G: GridGenerator + ?Sized, S: Solver + ?Sized> Generator<'a, G, S> {
    /// Borrows a randomized grid generator and an independent uniqueness checker.
    pub const fn new(grid_generator: &'a G, uniqueness_checker: &'a S) -> Self {
        Self {
            grid_generator,
            uniqueness_checker,
        }
    }

    /// Generates a complete grid, then attempts each clue removal in shuffled order.
    ///
    /// Each accepted removal preserves uniqueness. Stops at `min_clues` or after
    /// all cells have been considered once, possibly retaining more clues.
    /// The seed drives both construction and removal ordering. Reproduction
    /// requires the same strategies, locked RNG dependencies, and target.
    ///
    /// # Errors
    /// Returns [`GenerateError::ConstructionFailed`] if the grid strategy fails
    /// or supplies an incomplete board, or [`GenerateError::InvalidSearchResult`]
    /// if the uniqueness checker returns an inconsistent result.
    pub fn generate(
        &self,
        seed: u64,
        min_clues: MinClues,
    ) -> Result<GeneratedPuzzle, GenerateError> {
        let mut rng = SmallRng::seed_from_u64(seed);
        let solution = self
            .grid_generator
            .generate_grid(&mut rng)
            .filter(Board::is_solved)
            .ok_or(GenerateError::ConstructionFailed)?;
        let mut cells = *solution.cells();
        let mut order: [usize; 81] = std::array::from_fn(|index| index);
        order.shuffle(&mut rng);
        let mut clues = 81;
        for index in order {
            if clues == min_clues.get() {
                break;
            }
            let digit = cells[index];
            cells[index] = 0;
            let puzzle = Board::try_from(cells).expect("removing a clue preserves board validity");
            let result = self
                .uniqueness_checker
                .search(&puzzle, NonZeroUsize::MIN.saturating_add(1));
            // The complete grid remains a solution after every clue removal. A
            // checker must enumerate up to two solutions for this exact puzzle.
            if result
                .solution()
                .is_none_or(|board| !board.is_solution_for(&puzzle))
                || result.solutions_found() > 2
                || result.limit_reached() != (result.solutions_found() == 2)
                || matches!(result.extent(), crate::SearchExtent::Capped(cap) if cap.get() != 2)
                || result
                    .unique_solution()
                    .is_some_and(|board| board != &solution)
            {
                return Err(GenerateError::InvalidSearchResult);
            }
            if result.unique_solution().is_some() {
                clues -= 1;
            } else {
                cells[index] = digit;
            }
        }
        Ok(GeneratedPuzzle {
            puzzle: Board::try_from(cells).expect("clue removal preserves board validity"),
            solution,
        })
    }
}

/// Generates a unique puzzle by randomized search followed by clue removal.
///
/// Uses [`Bitmask`] for grid construction and uniqueness checks. Use
/// [`Generator`] to choose those capabilities independently.
///
/// `min_clues` is a lower bound, not an exact target or a difficulty rating.
/// Each cell is considered for removal once, in shuffled order, and a removal
/// is retained only if a capped search finds exactly one solution. Generation
/// stops at the lower bound or when all removals have been attempted.
///
/// Seeds reproduce outputs with the locked dependencies on the same target;
/// [`SmallRng`] does not promise identical output across versions or platforms.
/// Grids are not sampled uniformly from all possible Sudoku solutions.
///
/// # Errors
/// Returns [`GenerateError::ConstructionFailed`] if grid construction fails.
pub fn generate(seed: u64, min_clues: MinClues) -> Result<GeneratedPuzzle, GenerateError> {
    Generator::new(&Bitmask, &Bitmask).generate(seed, min_clues)
}

/// Generates a puzzle using a supplied strategy for uniqueness checks.
///
/// Uses the same randomized grid construction and clue-removal policy as
/// [`generate`]. This lets custom solvers participate in generation without
/// joining the built-in registry. Use [`Generator`] to also select the grid
/// construction strategy.
///
/// # Errors
/// Returns the same construction error as [`generate`], or
/// [`GenerateError::InvalidSearchResult`] when the checker violates its contract.
pub fn generate_with_solver(
    seed: u64,
    min_clues: MinClues,
    solver: &(impl Solver + ?Sized),
) -> Result<GeneratedPuzzle, GenerateError> {
    Generator::new(&Bitmask, solver).generate(seed, min_clues)
}
