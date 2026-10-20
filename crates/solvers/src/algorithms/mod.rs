use std::num::NonZeroUsize;

use rand::{Rng, seq::SliceRandom};

#[cfg(test)]
use crate::registry::built_in_solvers;
use crate::{
    Board, CandidateMask, CellIndex, SearchExtent, SearchProgress, SearchResult, SearchStats,
    Solver, TraceEvent, TraceKind, TraceOutcome, TraceSink, TraceSnapshot, TraceSolver,
};

mod backtracking;
mod bitmask;
mod bitset_cover;
mod constraint_propagation;
mod dancing_links;
mod domains;
mod forward_checking;
mod topology;

pub use backtracking::Backtracking;
pub use bitmask::Bitmask;
pub use bitset_cover::BitsetCover;
pub use constraint_propagation::ConstraintPropagation;
pub use dancing_links::DancingLinks;
pub use forward_checking::ForwardChecking;

fn shuffled_digits<R: Rng + ?Sized>(
    mut mask: u16,
    rng: &mut R,
) -> impl Iterator<Item = u8> + use<R> {
    let mut digits = [0; 9];
    let mut count = 0;
    while mask != 0 {
        digits[count] = mask.trailing_zeros() as u8;
        mask &= mask - 1;
        count += 1;
    }
    digits[..count].shuffle(rng);
    digits.into_iter().take(count)
}

// Mutable search bookkeeping belongs to implementations, not the public result API.
#[derive(Clone, Debug, PartialEq, Eq)]
struct SearchState {
    solution: Option<[u8; 81]>,
    solutions_found: usize,
    stats: SearchStats,
    extent: SearchExtent,
}
impl SearchState {
    fn empty() -> Self {
        Self {
            solution: None,
            solutions_found: 0,
            stats: SearchStats::default(),
            extent: SearchExtent::Exhausted,
        }
    }
    fn limit_reached(&self) -> bool {
        matches!(self.extent, SearchExtent::Capped(_))
    }
    #[inline]
    fn finish(self, puzzle: &Board) -> SearchResult {
        SearchResult::try_from_cells(
            puzzle,
            self.solution,
            self.solutions_found,
            self.extent,
            self.stats,
        )
        .expect("solver results preserve the input puzzle and search cap")
    }
}

fn collect(limit: NonZeroUsize, run: impl FnOnce(&mut Collector)) -> SearchState {
    let mut collector = Collector {
        result: SearchState::empty(),
        limit,
        observer: NoObserver,
    };
    run(&mut collector);
    collector.result
}

trait Observer {
    const ENABLED: bool;
    fn event(&mut self, event: TraceEvent);
    fn stopped(&self) -> bool {
        false
    }
    fn focus(&self) -> CellIndex {
        CellIndex::FIRST
    }
}
struct NoObserver;
impl Observer for NoObserver {
    const ENABLED: bool = false;
    fn event(&mut self, _: TraceEvent) {}
}
struct Observed<'a> {
    sink: &'a mut dyn TraceSink,
    stopped: bool,
    last_snapshot: TraceSnapshot,
}
impl Observer for Observed<'_> {
    const ENABLED: bool = true;
    fn event(&mut self, event: TraceEvent) {
        if let TraceEvent::Step { snapshot, .. } = &event {
            self.last_snapshot = snapshot.clone();
        }
        self.stopped = self.sink.event(event).is_break();
    }
    fn stopped(&self) -> bool {
        self.stopped
    }
    fn focus(&self) -> CellIndex {
        self.last_snapshot.focus()
    }
}
fn collect_traced(
    limit: NonZeroUsize,
    name: &'static str,
    puzzle: &Board,
    sink: &mut dyn TraceSink,
    run: impl FnOnce(&mut Collector<Observed<'_>>),
) -> TraceOutcome {
    let mut collector = Collector {
        result: SearchState::empty(),
        limit,
        observer: Observed {
            sink,
            stopped: false,
            last_snapshot: TraceSnapshot::new(
                puzzle.clone(),
                None,
                SearchStats::default(),
                0,
                CellIndex::FIRST,
            ),
        },
    };
    collector.observer.event(TraceEvent::Start {
        solver: name.to_owned(),
        puzzle: puzzle.clone(),
    });
    if !collector.stopped() {
        run(&mut collector);
    }
    if collector.stopped() {
        return TraceOutcome::Cancelled(
            SearchProgress::new(
                puzzle,
                collector.result.solution.map(|cells| {
                    Board::try_from(cells).expect("solver solutions are valid Sudoku boards")
                }),
                collector.result.solutions_found,
                collector.result.stats,
            )
            .expect("solver progress preserves the input puzzle"),
        );
    }
    let result = collector.result.finish(puzzle);
    let previous = &collector.observer.last_snapshot;
    let snapshot = TraceSnapshot::new(
        result
            .solution()
            .cloned()
            .unwrap_or_else(|| previous.board().clone()),
        None,
        result.stats(),
        previous.depth(),
        previous.focus(),
    );
    collector.observer.event(TraceEvent::Finished {
        result: result.clone(),
        snapshot,
    });
    TraceOutcome::Completed(result)
}

struct Collector<O = NoObserver> {
    result: SearchState,
    limit: NonZeroUsize,
    observer: O,
}

impl<O: Observer> Collector<O> {
    fn observe(
        &mut self,
        kind: impl FnOnce() -> TraceKind,
        depth: usize,
        snapshot: impl FnOnce() -> ([u8; 81], Option<[u16; 81]>),
    ) {
        if O::ENABLED && !self.observer.stopped() {
            let kind = kind();
            let (cells, candidates) = snapshot();
            let focus = match kind {
                TraceKind::Assignment { cell, .. } => cell,
                _ => self.observer.focus(),
            };
            let snapshot = TraceSnapshot::new(
                Board::try_from(cells).expect("solver snapshots are valid Sudoku boards"),
                candidates.map(|masks| {
                    masks.map(|bits| {
                        CandidateMask::try_from(bits)
                            .expect("solver candidates contain only legal digits")
                    })
                }),
                self.result.stats,
                depth,
                focus,
            );
            self.observer.event(TraceEvent::Step { kind, snapshot });
        }
    }
    fn stopped(&self) -> bool {
        O::ENABLED && self.observer.stopped()
    }
    fn visit(&mut self, depth: usize) {
        self.result.stats.nodes += 1;
        self.result.stats.max_depth = self.result.stats.max_depth.max(depth);
    }

    fn record(&mut self, cells: &[u8; 81]) {
        if self.result.solution.is_none() {
            self.result.solution = Some(*cells);
        }
        self.result.solutions_found += 1;
        if self.result.solutions_found == self.limit.get() {
            self.result.extent = SearchExtent::Capped(self.limit);
        }
    }
}

// These conversions enforce the same public domain invariants as external solvers.
fn cell_index(value: usize) -> CellIndex {
    CellIndex::try_from(value).expect("search indices are within the board")
}
fn search_digit(value: u8) -> crate::Digit {
    crate::Digit::try_from(value).expect("search digits are in 1..=9")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::OnceLock;

    use ku_core::Record;

    // Self-contained test data lets published crates run their unit tests.
    // The integration suite independently verifies the external 128-puzzle corpus.
    pub(super) fn generated_records() -> &'static [Record] {
        static RECORDS: OnceLock<Vec<Record>> = OnceLock::new();
        RECORDS.get_or_init(|| {
            (0..32)
                .map(|seed| {
                    let (puzzle, solution) = crate::generate(seed, crate::MinClues::default())
                        .unwrap()
                        .into_parts();
                    Record::new(puzzle, Some(solution)).unwrap()
                })
                .collect()
        })
    }

    #[test]
    fn unobserved_collector_has_no_trace_state_overhead() {
        // The ordinary search collector has exactly the result and limit
        // footprint; cancellation state belongs to the tracing observer.
        assert_eq!(std::mem::size_of::<NoObserver>(), 0);
        assert_eq!(
            std::mem::size_of::<Collector>(),
            std::mem::size_of::<(SearchResult, usize)>(),
        );
    }

    #[test]
    fn unobserved_search_never_constructs_trace_payloads() {
        let mut collector = Collector {
            result: SearchState::empty(),
            limit: NonZeroUsize::MIN,
            observer: NoObserver,
        };
        collector.observe(
            || panic!("ordinary search must not construct checked trace values"),
            0,
            || panic!("ordinary search must not construct snapshots"),
        );
    }

    #[test]
    fn solvers_agree_on_deterministically_permuted_partial_grids() {
        use rand::{Rng, SeedableRng, rngs::SmallRng, seq::SliceRandom};

        let solution: Board =
            "534678912672195348198342567859761423426853791713924856961537284287419635345286179"
                .parse()
                .unwrap();
        let cap = NonZeroUsize::new(3).unwrap();
        let mut rng = SmallRng::seed_from_u64(0x0050_d0c0);
        for removed in [0, 4, 16, 32, 48, 64] {
            for _ in 0..4 {
                let mut digits = [1, 2, 3, 4, 5, 6, 7, 8, 9];
                digits.shuffle(&mut rng);
                let mut cells = solution.cells().map(|digit| digits[usize::from(digit - 1)]);
                let mut positions = std::array::from_fn::<_, 81, _>(|index| index);
                positions.shuffle(&mut rng);
                for &cell in &positions[..removed] {
                    cells[cell] = 0;
                }
                // Transposition preserves Sudoku but changes traversal order.
                if rng.random::<bool>() {
                    cells = std::array::from_fn(|index| cells[index % 9 * 9 + index / 9]);
                }
                let puzzle = Board::try_from(cells).unwrap();
                let expected = DancingLinks.search(&puzzle, cap);
                for &solver in built_in_solvers() {
                    let actual = solver.search(&puzzle, cap);
                    assert_eq!(
                        actual.solutions_found(),
                        expected.solutions_found(),
                        "{} {puzzle}",
                        solver.name()
                    );
                    assert_eq!(actual.limit_reached(), expected.limit_reached());
                    let solved = actual.into_solution().unwrap();
                    assert!(
                        Board::try_from(*solved.cells())
                            .unwrap()
                            .is_solution_for(&puzzle)
                    );
                    assert_eq!(*puzzle.cells(), cells);
                }
            }
        }
    }

    pub(super) fn assert_solver_contract(solver: &dyn Solver) {
        let limit = |value| NonZeroUsize::new(value).unwrap();
        for record in generated_records() {
            let original = record.puzzle().clone();
            let result = solver.search(record.puzzle(), limit(2));
            assert_eq!(
                result.solutions_found(),
                1,
                "{} {}",
                solver.name(),
                record.puzzle()
            );
            assert_eq!(result.solution(), record.solution());
            assert!(!result.limit_reached());
            assert_eq!(record.puzzle(), &original);
            let solution = result.into_solution().unwrap();
            assert!(solution.is_solution_for(record.puzzle()));
            assert_eq!(Board::try_from(*solution.cells()).unwrap(), solution);
        }
        let solution: Board =
            "534678912672195348198342567859761423426853791713924856961537284287419635345286179"
                .parse()
                .unwrap();
        assert_eq!(solver.search(&solution, limit(2)).solutions_found(), 1);
        let mut rectangle = *solution.cells();
        for cell in [3, 4, 30, 31] {
            rectangle[cell] = 0;
        }
        let puzzle = Board::try_from(rectangle).unwrap();
        for cap in [1, 2, 3] {
            let result = solver.search(&puzzle, limit(cap));
            assert_eq!(result.solutions_found(), cap.min(2));
            assert_eq!(result.limit_reached(), cap <= 2);
            assert!(result.into_solution().unwrap().is_solution_for(&puzzle));
        }
        let result = solver.search(&Board::default(), limit(2));
        assert_eq!(result.solutions_found(), 2);
        assert!(result.limit_reached());
        assert!(
            Board::try_from(*result.into_solution().unwrap().cells())
                .unwrap()
                .is_solved()
        );
        let mut contradictory = [0; 81];
        contradictory[..8].copy_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8]);
        contradictory[17] = 9;
        let puzzle = Board::try_from(contradictory).unwrap();
        let result = solver.search(&puzzle, limit(2));
        assert_eq!(result.solutions_found(), 0);
        assert_eq!(result.solution(), None);
        assert!(!result.limit_reached());
        let mut deeper: [u8; 81] =
            "530070000600195000098000060800060003400803001700020006060000280000419005000080079"
                .parse::<Board>()
                .unwrap()
                .into();
        deeper[2] = 1;
        let result = solver.search(&Board::try_from(deeper).unwrap(), limit(2));
        assert_eq!(result.solutions_found(), 0);
        assert_eq!(result.solution(), None);
        assert!(!result.limit_reached());
    }
}
