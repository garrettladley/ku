use std::num::NonZeroUsize;

use rand::Rng;

use super::{
    Collector, Observer, SearchResult, Solver, cell_index, collect, collect_traced, search_digit,
    shuffled_digits, topology::PEERS,
};
use crate::{Board, GridGenerator, TraceKind, TraceSink, algorithms::topology::ALL_DIGITS};

/// Scan-based backtracking with minimum-remaining-values cell selection.
#[derive(Clone, Copy, Debug, Default)]
pub struct Backtracking;

impl Solver for Backtracking {
    fn name(&self) -> &'static str {
        "backtracking"
    }

    fn search(&self, puzzle: &Board, limit: NonZeroUsize) -> SearchResult {
        collect(limit, |collector| {
            let mut cells = *puzzle.cells();
            // Encode digits once: the hot peer scan can OR bits directly.
            // Empty cells use bit zero, which ALL_DIGITS excludes.
            let mut bits = cells.map(|digit| 1u16 << digit);
            visit(&mut cells, &mut bits, collector, 0);
        })
        .finish(puzzle)
    }
}

impl GridGenerator for Backtracking {
    fn generate_grid(&self, rng: &mut (impl Rng + ?Sized)) -> Option<Board> {
        random_visit(&mut [0; 81], rng)
    }
}

fn random_visit(cells: &mut [u8; 81], rng: &mut (impl Rng + ?Sized)) -> Option<Board> {
    let mut best = None;
    let mut best_count = 10;
    for (index, &digit) in cells.iter().enumerate() {
        if digit != 0 {
            continue;
        }
        let mask = candidate_mask(cells, index);
        let count = mask.count_ones();
        if count == 0 {
            return None;
        }
        if count < best_count {
            best = Some((index, mask));
            best_count = count;
        }
    }
    let Some((index, mask)) = best else {
        return Board::try_from(*cells).ok();
    };
    for digit in shuffled_digits(mask, rng) {
        cells[index] = digit;
        let solution = random_visit(cells, rng);
        cells[index] = 0;
        if solution.is_some() {
            return solution;
        }
    }
    None
}

impl super::TraceSolver for Backtracking {
    fn trace_search(
        &self,
        puzzle: &Board,
        limit: NonZeroUsize,
        sink: &mut dyn TraceSink,
    ) -> super::TraceOutcome {
        collect_traced(limit, self.name(), puzzle, sink, |collector| {
            let mut cells = *puzzle.cells();
            // Encode digits once: the hot peer scan can OR bits directly.
            // Empty cells use bit zero, which ALL_DIGITS excludes.
            let mut bits = cells.map(|digit| 1u16 << digit);
            visit(&mut cells, &mut bits, collector, 0);
        })
    }
}

fn visit<O: Observer>(
    cells: &mut [u8; 81],
    bits: &mut [u16; 81],
    collector: &mut Collector<O>,
    depth: usize,
) {
    collector.visit(depth);
    collector.observe(|| TraceKind::Node, depth, || (*cells, None));
    if collector.stopped() {
        return;
    }
    let before = collector.result.solutions_found;
    let mut best = None;
    let mut best_count = 10;
    let mut best_candidates = 0;
    let mut observed_candidates = [0; 81];
    for index in 0..81 {
        if cells[index] != 0 {
            continue;
        }
        let candidates = candidate_bits(bits, index);
        if O::ENABLED {
            observed_candidates[index] = candidates;
        }
        let count = candidates.count_ones();
        if count == 0 {
            collector.result.stats.backtracks += 1;
            collector.observe(|| TraceKind::Contradiction, depth, || (*cells, None));
            return;
        }
        if count < best_count {
            best = Some(index);
            best_count = count;
            best_candidates = candidates;
        }
    }
    let Some(index) = best else {
        collector.record(cells);
        collector.observe(|| TraceKind::Solution, depth, || (*cells, None));
        return;
    };
    collector.observe(
        || TraceKind::Candidates,
        depth,
        || (*cells, Some(observed_candidates)),
    );
    if collector.stopped() {
        return;
    }
    if best_count == 1 {
        collector.result.stats.forced_moves += 1;
    } else {
        collector.result.stats.guesses += 1;
    }
    while best_candidates != 0 {
        let digit = best_candidates.trailing_zeros() as u8;
        best_candidates &= best_candidates - 1;
        cells[index] = digit;
        bits[index] = 1 << digit;
        collector.observe(
            || TraceKind::Assignment {
                cell: cell_index(index),
                digit: search_digit(digit),
                forced: best_count == 1,
            },
            depth,
            || (*cells, None),
        );
        if collector.stopped() {
            cells[index] = 0;
            bits[index] = 1;
            return;
        }
        visit(cells, bits, collector, depth + 1);
        cells[index] = 0;
        bits[index] = 1;
        collector.observe(|| TraceKind::Restore, depth, || (*cells, None));
        if collector.stopped() || collector.result.limit_reached() {
            break;
        }
    }
    if collector.stopped() {
        return;
    }
    if before == collector.result.solutions_found {
        collector.result.stats.backtracks += 1;
        collector.observe(|| TraceKind::Contradiction, depth, || (*cells, None));
    }
}

// Keep the digit board for recording and tracing; this mirror removes the
// variable shifts from each of the twenty peer loads at every search node.
fn candidate_bits(bits: &[u16; 81], index: usize) -> u16 {
    let mut used = 0;
    for &peer in &PEERS[index] {
        used |= bits[peer];
    }
    ALL_DIGITS & !used
}

fn candidate_mask(cells: &[u8; 81], index: usize) -> u16 {
    let mut used = 0;
    for &peer in &PEERS[index] {
        used |= 1 << cells[peer];
    }
    ALL_DIGITS & !used
}

#[cfg(test)]
mod tests {
    use super::super::tests::generated_records;
    use super::*;

    fn scalar_legal(cells: &[u8; 81], index: usize, digit: u8) -> bool {
        let row = index / 9;
        let column = index % 9;
        let box_row = row / 3 * 3;
        let box_column = column / 3 * 3;
        (0..9).all(|offset| {
            cells[row * 9 + offset] != digit
                && cells[offset * 9 + column] != digit
                && cells[(box_row + offset / 3) * 9 + box_column + offset % 3] != digit
        })
    }

    fn scalar_candidates(cells: &[u8; 81], index: usize) -> u16 {
        (1..=9).fold(0, |mask, digit| {
            mask | if scalar_legal(cells, index, digit) {
                1 << digit
            } else {
                0
            }
        })
    }

    #[test]
    fn solver_matches_generated_cases_and_enumerates_caps() {
        super::super::tests::assert_solver_contract(&Backtracking);
    }

    #[test]
    fn candidates_check_each_kind_of_peer_but_allow_unrelated_digits() {
        assert_eq!(candidate_mask(&[0; 81], 0), ALL_DIGITS);
        for peer in [8, 72, 10] {
            let mut cells = [0; 81];
            cells[peer] = 5;
            assert_eq!(candidate_mask(&cells, 0), ALL_DIGITS & !(1 << 5));
            assert_eq!(candidate_mask(&cells, 0), scalar_candidates(&cells, 0));
        }
        let mut cells = [0; 81];
        cells[80] = 5;
        assert_eq!(candidate_mask(&cells, 0), ALL_DIGITS);
    }

    #[test]
    fn candidates_match_scalar_checks_on_generated_cases() {
        for record in generated_records() {
            for (index, &digit) in record.puzzle().cells().iter().enumerate() {
                if digit == 0 {
                    assert_eq!(
                        candidate_mask(record.puzzle().cells(), index),
                        scalar_candidates(record.puzzle().cells(), index),
                        "cell={index} puzzle={}",
                        record.puzzle()
                    );
                }
            }
            let mut cells = *record.solution().unwrap().cells();
            for index in 0..81 {
                let digit = cells[index];
                cells[index] = 0;
                assert_eq!(candidate_mask(&cells, index), 1 << digit);
                assert_eq!(
                    candidate_mask(&cells, index),
                    scalar_candidates(&cells, index)
                );
                cells[index] = digit;
            }
        }
    }

    #[test]
    fn encoded_peers_match_digits_and_restore_after_capped_search() {
        for record in generated_records() {
            let original = *record.puzzle().cells();
            let encoded = original.map(|digit| 1u16 << digit);
            for index in 0..81 {
                assert_eq!(
                    candidate_bits(&encoded, index),
                    candidate_mask(&original, index)
                );
            }
            for cap in [1, 2, 3] {
                let mut cells = original;
                let mut bits = encoded;
                collect(NonZeroUsize::new(cap).unwrap(), |collector| {
                    visit(&mut cells, &mut bits, collector, 0);
                });
                assert_eq!(cells, original);
                assert_eq!(bits, encoded);
            }
        }
    }

    #[test]
    fn encoded_board_is_restored_when_trace_observation_cancels() {
        struct CancelAfter(usize);
        impl Observer for CancelAfter {
            const ENABLED: bool = true;

            fn event(&mut self, _: crate::TraceEvent) {
                self.0 -= 1;
            }

            fn stopped(&self) -> bool {
                self.0 == 0
            }
        }

        let original = *generated_records()[0].puzzle().cells();
        let encoded = original.map(|digit| 1u16 << digit);
        // Cancel at nodes, candidate snapshots, assignments, and nested restores.
        for events in 1..=40 {
            let mut cells = original;
            let mut bits = encoded;
            let mut collector = Collector {
                result: super::super::SearchState::empty(),
                limit: NonZeroUsize::new(2).unwrap(),
                observer: CancelAfter(events),
            };
            visit(&mut cells, &mut bits, &mut collector, 0);
            assert!(collector.stopped());
            assert_eq!(cells, original, "events={events}");
            assert_eq!(bits, encoded, "events={events}");
        }
    }

    // Keep the original scalar traversal as an independent test oracle for
    // candidate ordering, MRV ties, and every publicly returned search counter.
    fn scalar_visit(cells: &mut [u8; 81], collector: &mut Collector, depth: usize) {
        collector.visit(depth);
        let before = collector.result.solutions_found;
        let mut best = None;
        let mut best_count = 10;
        let mut best_candidates = [false; 10];
        for index in 0..81 {
            if cells[index] != 0 {
                continue;
            }
            let mut candidates = [false; 10];
            let mut count = 0;
            for digit in 1..=9 {
                if scalar_legal(cells, index, digit) {
                    candidates[usize::from(digit)] = true;
                    count += 1;
                }
            }
            if count == 0 {
                collector.result.stats.backtracks += 1;
                return;
            }
            if count < best_count {
                best = Some(index);
                best_count = count;
                best_candidates = candidates;
            }
        }
        let Some(index) = best else {
            collector.record(cells);
            return;
        };
        if best_count == 1 {
            collector.result.stats.forced_moves += 1;
        } else {
            collector.result.stats.guesses += 1;
        }
        for digit in 1..=9 {
            if best_candidates[usize::from(digit)] {
                cells[index] = digit;
                scalar_visit(cells, collector, depth + 1);
                cells[index] = 0;
                if collector.result.limit_reached() {
                    break;
                }
            }
        }
        if before == collector.result.solutions_found {
            collector.result.stats.backtracks += 1;
        }
    }

    #[test]
    fn scalar_search_order_and_statistics_are_preserved() {
        let mut puzzles: Vec<_> = generated_records()
            .iter()
            .take(4)
            .map(|record| record.puzzle().clone())
            .collect();
        for floor in [17, 22, 28, 36, 48, 81] {
            for seed in [100_000, 100_001] {
                puzzles.push(
                    crate::generate(seed, crate::MinClues::try_from(floor).unwrap())
                        .unwrap()
                        .into_parts()
                        .0,
                );
            }
        }
        let solution: Board =
            "534678912672195348198342567859761423426853791713924856961537284287419635345286179"
                .parse()
                .unwrap();
        let mut ambiguous = *solution.cells();
        for index in [3, 4, 30, 31] {
            ambiguous[index] = 0;
        }
        let mut contradictory = [0; 81];
        contradictory[..8].copy_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8]);
        contradictory[17] = 9;
        puzzles.extend([
            solution,
            Board::try_from(ambiguous).unwrap(),
            Board::try_from(contradictory).unwrap(),
            Board::default(),
        ]);
        for puzzle in puzzles {
            for cap in [1, 2, 3] {
                let limit = NonZeroUsize::new(cap).unwrap();
                let expected = collect(limit, |collector| {
                    let mut cells = *puzzle.cells();
                    scalar_visit(&mut cells, collector, 0);
                })
                .finish(&puzzle);
                assert_eq!(
                    Backtracking.search(&puzzle, limit),
                    expected,
                    "cap={cap} {puzzle}"
                );
            }
        }
    }
}
