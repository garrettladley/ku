//! Cross-strategy correctness, solution caps, and search-state restoration.

mod support;

use std::num::NonZeroUsize;

use ku_core::{Board, SearchResult, Solver};
use ku_solvers::{Bitmask, built_in_solvers};

const PUZZLE: &str =
    "530070000600195000098000060800060003400803001700020006060000280000419005000080079";
const SOLUTION: &str =
    "534678912672195348198342567859761423426853791713924856961537284287419635345286179";

fn limit(value: usize) -> NonZeroUsize {
    NonZeroUsize::new(value).unwrap()
}

#[test]
fn registered_solvers_agree_on_generated_puzzles_and_prove_uniqueness() {
    for (min_clues, records) in support::groups() {
        assert_eq!(records.len(), 32);
        for (index, record) in records.iter().enumerate() {
            let original = record.puzzle().clone();
            for &solver in built_in_solvers() {
                let result = solver.search(record.puzzle(), limit(2));
                assert_eq!(
                    result.solutions_found(),
                    1,
                    "{} min_clues={min_clues} index={index}",
                    solver.name()
                );
                assert_eq!(
                    result.solution(),
                    record.solution(),
                    "{} min_clues={min_clues} index={index}",
                    solver.name()
                );
                assert!(!result.limit_reached());
                let solved = result.solution().unwrap();
                assert!(solved.is_solution_for(record.puzzle()));
                // Independently revalidate every solver's returned grid.
                assert_eq!(
                    Board::try_from(<[u8; 81]>::from(solved.clone())).unwrap(),
                    solved.clone()
                );
                assert_eq!(record.puzzle(), &original);
                assert_eq!(solver.solve(record.puzzle()), record.solution().cloned());
                assert!(result.stats().nodes > 0);
                assert!(result.stats().max_depth <= 81);
            }
        }
    }
}

#[test]
fn complete_board_has_one_solution_and_is_unchanged() {
    let board: Board = SOLUTION.parse().unwrap();
    for &solver in built_in_solvers() {
        let result = solver.search(&board, limit(2));
        assert_eq!(result.solutions_found(), 1, "{}", solver.name());
        assert_eq!(result.solution(), Some(&board));
        assert!(!result.limit_reached());
    }
}

#[test]
fn valid_but_contradictory_boards_have_no_solution() {
    let mut immediate = [0; 81];
    immediate[..8].copy_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8]);
    immediate[17] = 9;
    let mut deeper: [u8; 81] = PUZZLE.parse::<Board>().unwrap().into();
    deeper[2] = 1; // Legal as a given, but incompatible with the puzzle's unique solution.
    for cells in [immediate, deeper] {
        let puzzle = Board::try_from(cells).unwrap();
        for &solver in built_in_solvers() {
            let result = solver.search(&puzzle, limit(2));
            assert_eq!(result.solutions_found(), 0, "{}", solver.name());
            assert_eq!(result.solution(), None);
            assert!(!result.limit_reached());
            assert!(result.stats().backtracks > 0);
            assert_eq!(puzzle.cells(), &cells);
        }
    }
}

#[test]
fn exactly_two_solutions_respect_caps_and_restore_search_state() {
    let mut cells: [u8; 81] = SOLUTION.parse::<Board>().unwrap().into();
    for index in [3, 4, 30, 31] {
        cells[index] = 0;
    }
    let puzzle = Board::try_from(cells).unwrap();
    for &solver in built_in_solvers() {
        for cap in [1, 2, 3] {
            let result = solver.search(&puzzle, limit(cap));
            assert_eq!(
                result.solutions_found(),
                cap.min(2),
                "{} cap={cap}",
                solver.name()
            );
            assert_eq!(result.limit_reached(), cap <= 2);
            assert!(result.solution().unwrap().is_solution_for(&puzzle));
        }
        assert_eq!(puzzle.cells(), &cells);
    }
}

#[test]
fn empty_board_stops_at_the_requested_cap() {
    let puzzle = Board::default();
    for &solver in built_in_solvers() {
        let result = solver.search(&puzzle, limit(2));
        assert_eq!(result.solutions_found(), 2, "{}", solver.name());
        assert!(result.limit_reached());
        assert!(result.solution().unwrap().is_solution_for(&puzzle));
    }
}

#[test]
fn custom_solver_works_without_a_builtin_enum_or_registry_entry() {
    struct CustomSolver;
    impl Solver for CustomSolver {
        fn name(&self) -> &'static str {
            "custom"
        }
        fn search(&self, puzzle: &Board, cap: NonZeroUsize) -> SearchResult {
            Bitmask.search(puzzle, cap)
        }
    }
    let solver: &dyn Solver = &CustomSolver;
    assert!(
        !built_in_solvers()
            .iter()
            .any(|builtin| builtin.name() == solver.name())
    );
    let puzzle = PUZZLE.parse().unwrap();
    assert_eq!(solver.solve(&puzzle), Some(SOLUTION.parse().unwrap()));
}

#[test]
fn registry_names_are_unique_and_nonempty() {
    let names: Vec<_> = built_in_solvers()
        .iter()
        .map(|solver| solver.name())
        .collect();
    for (index, name) in names.iter().enumerate() {
        assert!(!name.is_empty());
        assert!(!names[..index].contains(name));
    }
}

#[test]
fn solver_heap_allocation_budgets_are_measured() {
    for record in &support::groups().last().unwrap().1 {
        for &solver in built_in_solvers() {
            let info = allocation_counter::measure(|| {
                std::hint::black_box(solver.solve(record.puzzle()));
            });
            assert_eq!(
                info.count_total,
                u64::from(solver.name() == "dlx"),
                "{}",
                solver.name()
            );
            assert_eq!(info.count_current, 0);
            assert_eq!(info.bytes_current, 0);
        }
    }
}

#[test]
fn constraint_frontier_matches_independent_checker_on_holdout_generation() {
    use ku_solvers::{ConstraintPropagation, DancingLinks, MinClues, generate};

    let check = |puzzle: &Board, solution: &Board| {
        for cap in [1, 2, 3] {
            let actual = ConstraintPropagation.search(puzzle, limit(cap));
            let expected = DancingLinks.search(puzzle, limit(cap));
            assert_eq!(actual.solutions_found(), expected.solutions_found());
            assert_eq!(actual.extent(), expected.extent());
            assert_eq!(actual.solution(), Some(solution));
            assert!(actual.solution().unwrap().is_solution_for(puzzle));
        }
    };
    for (_, records) in support::generated_groups(100_000, 64) {
        for record in records {
            check(record.puzzle(), record.solution().unwrap());
        }
    }
    // Exercise both ends of the generator's supported clue range too.
    for floor in [MinClues::MIN, MinClues::MAX] {
        for seed in 600_000..600_008 {
            let generated = generate(seed, floor).unwrap();
            check(generated.puzzle(), generated.solution());
        }
    }
}
