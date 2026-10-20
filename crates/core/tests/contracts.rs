//! Public domain constructors, conversions, and search-result contracts.

use std::{collections::HashSet, num::NonZeroUsize};

use ku_core::{
    Board, CandidateMask, CellIndex, Digit, Game, Multiplicity, Record, SearchExtent,
    SearchProgress, SearchResult, SearchStats,
};

const SOLUTION: &str =
    "534678912672195348198342567859761423426853791713924856961537284287419635345286179";

#[test]
fn domain_types_validate_every_boundary_and_iterate_only_legal_values() {
    for index in 0..81 {
        let cell = CellIndex::try_from(index).unwrap();
        assert_eq!(cell.get(), index);
        assert_eq!(cell.row() * 9 + cell.column(), index);
    }
    for index in [81, 255, usize::MAX] {
        assert!(CellIndex::try_from(index).is_err());
    }
    assert_eq!(CellIndex::FIRST.moved(-1, -1).get(), 80);
    assert_eq!(
        CellIndex::try_from(80).unwrap().moved(1, 1),
        CellIndex::FIRST
    );
    assert_eq!(CellIndex::FIRST.moved(i8::MIN, i8::MAX).row(), 7);
    for value in 1..=9 {
        assert_eq!(Digit::try_from(value).unwrap().get(), value);
    }
    for value in [0, 10, 255] {
        assert!(Digit::try_from(value).is_err());
    }
    for bits in 0..=0x03ff {
        let result = CandidateMask::try_from(bits);
        assert_eq!(result.is_ok(), bits & 1 == 0);
        if let Ok(mask) = result {
            let reconstructed = mask.iter().fold(0, |bits, digit| {
                assert!(mask.contains(digit));
                bits | 1 << digit.get()
            });
            assert_eq!(reconstructed, bits);
        }
    }
    for bits in [0x0400, 0x8000, u16::MAX] {
        assert!(CandidateMask::try_from(bits).is_err());
    }
}

#[test]
fn board_supports_standard_slice_access_and_hash_collections() {
    let board: Board = SOLUTION.parse().unwrap();
    let slice: &[u8] = board.as_ref();
    let array: &[u8; 81] = board.as_ref();
    assert_eq!(slice, array);
    let mut set = HashSet::new();
    assert!(set.insert(board.clone()));
    assert!(!set.insert(board));
}

#[test]
fn record_construction_preserves_expected_solution_relationships() {
    let solution: Board = SOLUTION.parse().unwrap();
    assert!(Record::new(Board::default(), Some(Board::default())).is_err());
    let record = Record::new(Board::default(), Some(solution.clone())).unwrap();
    assert_eq!(record.solution(), Some(&solution));
    assert_eq!(record.into_parts(), (Board::default(), Some(solution)));
    assert!(Record::from(Board::default()).solution().is_none());
}

#[test]
fn completed_results_validate_counts_caps_and_puzzle_relationships() {
    let puzzle = Board::default();
    let solution: Board = SOLUTION.parse().unwrap();
    let stats = SearchStats::default();
    assert!(SearchResult::new(&puzzle, None, 1, SearchExtent::Exhausted, stats).is_err());
    assert!(
        SearchResult::new(
            &puzzle,
            Some(solution.clone()),
            0,
            SearchExtent::Exhausted,
            stats
        )
        .is_err()
    );
    assert!(
        SearchResult::new(
            &puzzle,
            Some(puzzle.clone()),
            1,
            SearchExtent::Exhausted,
            stats
        )
        .is_err()
    );
    assert!(
        SearchResult::new(
            &puzzle,
            Some(solution.clone()),
            1,
            SearchExtent::Capped(NonZeroUsize::new(2).unwrap()),
            stats
        )
        .is_err()
    );
    for (count, extent, expected) in [
        (0, SearchExtent::Exhausted, Multiplicity::Unsatisfiable),
        (1, SearchExtent::Exhausted, Multiplicity::Unique),
        (
            1,
            SearchExtent::Capped(NonZeroUsize::MIN),
            Multiplicity::Undetermined,
        ),
        (2, SearchExtent::Exhausted, Multiplicity::Multiple),
        (
            2,
            SearchExtent::Capped(NonZeroUsize::new(2).unwrap()),
            Multiplicity::Multiple,
        ),
    ] {
        let result = SearchResult::new(
            &puzzle,
            (count > 0).then(|| solution.clone()),
            count,
            extent,
            stats,
        )
        .unwrap();
        assert_eq!(result.multiplicity(), expected);
        assert_eq!(
            result.unique_solution().is_some(),
            expected == Multiplicity::Unique
        );
        assert_eq!(
            result.limit_reached(),
            matches!(extent, SearchExtent::Capped(_))
        );
    }
    assert!(SearchProgress::new(&puzzle, None, 1, stats).is_err());
    assert!(SearchProgress::new(&puzzle, Some(solution), 1, stats).is_ok());
}

#[test]
fn typed_game_edits_and_raw_checked_boundary_share_identical_events() {
    let mut typed = Game::new(Board::default());
    let mut raw = typed.clone();
    let cell = CellIndex::try_from(80).unwrap();
    assert_eq!(
        typed.edit_cell(cell, Some(Digit::try_from(9).unwrap())),
        raw.edit(80, 9)
    );
    assert_eq!(typed.edit_cell(cell, None), raw.edit(80, 0));
    assert_eq!(typed, raw);
    assert!(raw.edit(81, 1).is_err());
    assert!(raw.edit(0, 10).is_err());
}

#[test]
fn raw_search_results_match_checked_board_construction() {
    let solved: Board = SOLUTION.parse().unwrap();
    let reference = *solved.cells();
    let givens: Board =
        "530070000600195000098000060800060003400803001700020006060000280000419005000080079"
            .parse()
            .unwrap();
    let compare = |cells: [u8; 81]| {
        for puzzle in [&Board::default(), &givens, &solved] {
            let expected = Board::try_from(cells)
                .map_err(|_| ku_core::SearchResultError)
                .and_then(|board| {
                    SearchResult::new(
                        puzzle,
                        Some(board),
                        1,
                        SearchExtent::Exhausted,
                        SearchStats::default(),
                    )
                });
            assert_eq!(
                SearchResult::try_from_cells(
                    puzzle,
                    Some(cells),
                    1,
                    SearchExtent::Exhausted,
                    SearchStats::default(),
                ),
                expected,
                "puzzle={puzzle}, cells={cells:?}",
            );
        }
    };
    compare(reference);
    compare([0; 81]);
    // A Latin square satisfies every row and column, but violates each box.
    let latin_square = std::array::from_fn(|index| ((index / 9 + index % 9) % 9 + 1) as u8);
    assert!(
        SearchResult::try_from_cells(
            &Board::default(),
            Some(latin_square),
            1,
            SearchExtent::Exhausted,
            SearchStats::default(),
        )
        .is_err()
    );
    compare(latin_square);
    // Exercise every byte value at every position, including zero, invalid
    // digits, and every kind of duplicate in otherwise complete grids.
    for index in 0..81 {
        for digit in 0..=u8::MAX {
            let mut cells = reference;
            cells[index] = digit;
            compare(cells);
        }
    }
    // Swaps retain digit ranges and completeness, isolating unit and given
    // checks. This also covers swaps that retain rows or columns but break boxes.
    for first in 0..81 {
        for second in first + 1..81 {
            let mut cells = reference;
            cells.swap(first, second);
            compare(cells);
        }
    }
    // A different valid solution must still fail when it changes the givens.
    compare(reference.map(|digit| match digit {
        1 => 2,
        2 => 1,
        digit => digit,
    }));
}

#[test]
fn raw_search_results_match_count_and_extent_validation() {
    let puzzle = Board::default();
    let solved: Board = SOLUTION.parse().unwrap();
    let stats = SearchStats {
        nodes: 17,
        guesses: 3,
        backtracks: 2,
        forced_moves: 9,
        max_depth: 4,
    };
    for solution in [None, Some(solved)] {
        for count in 0..=3 {
            for extent in [
                SearchExtent::Exhausted,
                SearchExtent::Capped(NonZeroUsize::MIN),
                SearchExtent::Capped(NonZeroUsize::new(2).unwrap()),
                SearchExtent::Capped(NonZeroUsize::new(3).unwrap()),
            ] {
                assert_eq!(
                    SearchResult::try_from_cells(
                        &puzzle,
                        solution.as_ref().map(|board| *board.cells()),
                        count,
                        extent,
                        stats
                    ),
                    SearchResult::new(&puzzle, solution.clone(), count, extent, stats),
                );
            }
        }
    }
}
