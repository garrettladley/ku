//! Seeded generation, clue bounds, and independent uniqueness checks.

use std::{
    collections::HashSet,
    num::NonZeroUsize,
    sync::atomic::{AtomicUsize, Ordering},
};

use ku_core::{Board, SearchExtent, SearchResult, Solver};
use ku_solvers::{
    Backtracking, Bitmask, DancingLinks, ForwardChecking, GenerateError, Generator, GridGenerator,
    MinClues, MinCluesError, built_in_solvers, generate, generate_with_solver,
};
use rand::{Rng, RngCore, SeedableRng, rngs::SmallRng};

fn unique_limit() -> NonZeroUsize {
    NonZeroUsize::new(2).unwrap()
}

#[test]
fn generated_puzzles_have_one_solution_matching_the_saved_grid() {
    for seed in 0..24 {
        let generated = generate(seed, MinClues::default()).unwrap();
        assert!(generated.puzzle().clue_count() >= 32);
        assert!(generated.solution().is_solution_for(generated.puzzle()));
        for &solver in built_in_solvers() {
            let result = solver.search(generated.puzzle(), unique_limit());
            assert_eq!(result.solutions_found(), 1, "{} seed={seed}", solver.name());
            assert_eq!(result.solution(), Some(generated.solution()));
        }
    }
}

#[test]
fn seeds_reproduce_puzzles_and_distinct_seeds_change_grids() {
    assert_eq!(
        generate(42, MinClues::default()).unwrap(),
        generate(42, MinClues::default()).unwrap()
    );
    assert_ne!(
        generate(42, MinClues::default()).unwrap().solution(),
        generate(43, MinClues::default()).unwrap().solution()
    );
}

#[test]
fn requesting_all_clues_returns_a_complete_grid() {
    let generated = generate(7, MinClues::MAX).unwrap();
    assert_eq!(generated.puzzle(), generated.solution());
    assert_eq!(generated.puzzle().clue_count(), 81);
}

#[test]
fn clue_floor_conversions_validate_bounds_and_round_trip() {
    for raw in (0..=82).chain(std::iter::once(usize::MAX)) {
        let parsed = raw.to_string().parse::<MinClues>();
        let converted = MinClues::try_from(raw);
        assert_eq!(parsed, converted);
        if (17..=81).contains(&raw) {
            let floor = converted.unwrap();
            assert_eq!(floor.get(), raw);
            assert_eq!(usize::from(floor), raw);
            assert_eq!(floor.to_string(), raw.to_string());
        } else {
            assert_eq!(converted, Err(MinCluesError));
        }
    }
    assert_eq!(MinClues::MIN.get(), 17);
    assert_eq!(MinClues::MAX.get(), 81);
    assert_eq!(MinClues::default().get(), 32);
}

#[test]
fn clue_floor_parsing_rejects_malformed_and_overflowing_numbers() {
    for input in [
        "",
        "many",
        "17.5",
        "-1",
        "999999999999999999999999999999999999",
    ] {
        assert_eq!(input.parse::<MinClues>(), Err(MinCluesError));
    }
}

#[test]
fn exhausted_removal_pass_returns_a_minimal_puzzle_above_the_floor() {
    let generated = generate(123, MinClues::MIN).unwrap();
    assert!(generated.puzzle().clue_count() >= 17);
    if generated.puzzle().clue_count() > 17 {
        for (index, &digit) in generated.puzzle().cells().iter().enumerate() {
            if digit == 0 {
                continue;
            }
            let mut cells = *generated.puzzle().cells();
            cells[index] = 0;
            let reduced = Board::try_from(cells).unwrap();
            for &solver in built_in_solvers() {
                assert_eq!(
                    solver.search(&reduced, unique_limit()).solutions_found(),
                    2,
                    "{} removable cell={index}",
                    solver.name()
                );
            }
        }
    }
}

#[test]
fn generation_accepts_an_external_uniqueness_checker() {
    struct CountingSolver(AtomicUsize);
    impl Solver for CountingSolver {
        fn name(&self) -> &'static str {
            "counting"
        }
        fn search(&self, puzzle: &Board, cap: NonZeroUsize) -> SearchResult {
            self.0.fetch_add(1, Ordering::Relaxed);
            Bitmask.search(puzzle, cap)
        }
    }
    let solver = CountingSolver(AtomicUsize::new(0));
    let generated = generate_with_solver(99, MinClues::default(), &solver).unwrap();
    assert!(solver.0.load(Ordering::Relaxed) >= 49);
    assert_eq!(
        Bitmask
            .search(generated.puzzle(), unique_limit())
            .solutions_found(),
        1
    );
}

#[test]
fn generation_rejects_inconsistent_external_checker_results() {
    struct InvalidChecker(u8);
    impl Solver for InvalidChecker {
        fn name(&self) -> &'static str {
            "invalid-checker"
        }
        fn search(&self, puzzle: &Board, _: NonZeroUsize) -> SearchResult {
            match self.0 {
                0 => {
                    SearchResult::new(puzzle, None, 0, SearchExtent::Exhausted, Default::default())
                        .unwrap()
                }
                1 => Bitmask.search(puzzle, NonZeroUsize::MIN),
                2 => {
                    let unrelated = Bitmask.solve(&Board::default()).unwrap();
                    SearchResult::new(
                        &Board::default(),
                        Some(unrelated),
                        1,
                        SearchExtent::Exhausted,
                        Default::default(),
                    )
                    .unwrap()
                }
                3 => {
                    let solution = Bitmask.solve(puzzle).unwrap();
                    SearchResult::new(
                        puzzle,
                        Some(solution),
                        3,
                        SearchExtent::Exhausted,
                        Default::default(),
                    )
                    .unwrap()
                }
                _ => {
                    let solution = Bitmask.solve(puzzle).unwrap();
                    SearchResult::new(
                        puzzle,
                        Some(solution),
                        2,
                        SearchExtent::Exhausted,
                        Default::default(),
                    )
                    .unwrap()
                }
            }
        }
    }
    for invalid in 0..5 {
        assert_eq!(
            generate_with_solver(
                42,
                MinClues::try_from(80).unwrap(),
                &InvalidChecker(invalid)
            ),
            Err(GenerateError::InvalidSearchResult),
            "checker {invalid}"
        );
    }
}

#[test]
fn randomized_grid_strategies_are_valid_reproducible_and_varied() {
    fn check(strategy: &impl GridGenerator) {
        let mut grids = HashSet::new();
        for seed in 0..24 {
            let mut first = SmallRng::seed_from_u64(seed);
            let mut second = SmallRng::seed_from_u64(seed);
            let grid = strategy.generate_grid(&mut first).unwrap();
            assert!(grid.is_solved());
            assert_eq!(Board::try_from(*grid.cells()).unwrap(), grid);
            assert_eq!(strategy.generate_grid(&mut second), Some(grid.clone()));
            assert_eq!(first.random::<u64>(), second.random::<u64>());
            grids.insert(grid);
        }
        assert!(
            grids.len() > 1,
            "different seeds must vary the complete grid"
        );
        let mut rng = SmallRng::seed_from_u64(42);
        let erased: &mut dyn RngCore = &mut rng;
        assert!(strategy.generate_grid(erased).unwrap().is_solved());
    }
    check(&Bitmask);
    check(&Backtracking);
    check(&ForwardChecking);
}

#[test]
fn every_grid_strategy_composes_with_every_registered_uniqueness_checker() {
    fn check(grid_strategy: &impl GridGenerator) {
        for &checker in built_in_solvers() {
            let generator = Generator::new(grid_strategy, checker);
            for seed in [0, 42, u64::MAX] {
                let generated = generator.generate(seed, MinClues::default()).unwrap();
                let mut rng = SmallRng::seed_from_u64(seed);
                assert_eq!(
                    grid_strategy.generate_grid(&mut rng).as_ref(),
                    Some(generated.solution())
                );
                assert!(generated.puzzle().clue_count() >= MinClues::default().get());
                let verified = DancingLinks.search(generated.puzzle(), unique_limit());
                assert_eq!(verified.unique_solution(), Some(generated.solution()));
            }
        }
    }
    check(&Bitmask);
    check(&Backtracking);
    check(&ForwardChecking);
}

#[test]
fn externally_supplied_grid_capability_is_used_without_requiring_solver() {
    struct FixedGrid {
        board: Board,
        calls: AtomicUsize,
    }
    impl GridGenerator for FixedGrid {
        fn generate_grid(&self, _: &mut (impl Rng + ?Sized)) -> Option<Board> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            Some(self.board.clone())
        }
    }
    let strategy = FixedGrid {
        board: Backtracking.solve(&Board::default()).unwrap(),
        calls: AtomicUsize::new(0),
    };
    let generated = Generator::new(&strategy, &DancingLinks)
        .generate(42, MinClues::MAX)
        .unwrap();
    assert_eq!(strategy.calls.load(Ordering::Relaxed), 1);
    assert_eq!(generated.solution(), &strategy.board);
    assert_eq!(generated.puzzle(), generated.solution());
}

#[test]
fn failed_or_incomplete_grid_construction_is_rejected() {
    struct SuppliedGrid(Option<Board>);
    impl GridGenerator for SuppliedGrid {
        fn generate_grid(&self, _: &mut (impl Rng + ?Sized)) -> Option<Board> {
            self.0.clone()
        }
    }
    for supplied in [None, Some(Board::default())] {
        let strategy = SuppliedGrid(supplied);
        assert_eq!(
            Generator::new(&strategy, &Bitmask).generate(42, MinClues::default()),
            Err(GenerateError::ConstructionFailed)
        );
    }
}
