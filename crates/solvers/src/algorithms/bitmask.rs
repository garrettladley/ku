use std::num::NonZeroUsize;

use rand::Rng;

use super::{
    Collector, Observer, SearchResult, Solver, cell_index, collect, collect_traced, search_digit,
    shuffled_digits,
};
use crate::{
    Board, GridGenerator, TraceKind, TraceSink,
    algorithms::topology::{ALL_DIGITS, UNITS, box_index},
};

#[derive(Clone)]
struct State {
    cells: [u8; 81],
    rows: [u16; 9],
    columns: [u16; 9],
    boxes: [u16; 9],
    // Column bits for the unfilled cells in each row.
    empty: [u16; 9],
}

impl State {
    fn new(board: &Board) -> Self {
        let mut state = Self {
            cells: [0; 81],
            rows: [0; 9],
            columns: [0; 9],
            boxes: [0; 9],
            empty: [(1 << 9) - 1; 9],
        };
        for (index, &digit) in board.cells().iter().enumerate() {
            if digit != 0 {
                state.place(index, digit);
            }
        }
        state
    }

    fn place(&mut self, index: usize, digit: u8) {
        self.cells[index] = digit;
        self.empty[index / 9] &= !(1 << (index % 9));
        let bit = 1 << digit;
        self.rows[index / 9] |= bit;
        self.columns[index % 9] |= bit;
        self.boxes[box_index(index)] |= bit;
    }

    #[cfg(test)]
    fn candidates(&self, index: usize) -> u16 {
        ALL_DIGITS
            & !(self.rows[index / 9] | self.columns[index % 9] | self.boxes[box_index(index)])
    }

    fn propagate(&mut self, forced: &mut u64) -> Option<[u16; 81]> {
        let mut output = None;
        let result = collect(NonZeroUsize::MIN, |collector| {
            output = Some(self.propagate_observed(collector, 0));
        });
        *forced += result.stats.forced_moves;
        output.unwrap()
    }
    fn propagate_observed<O: Observer>(
        &mut self,
        collector: &mut Collector<O>,
        depth: usize,
    ) -> Option<[u16; 81]> {
        loop {
            let mut candidates = [0; 81];
            let mut changed = false;
            // Keep the original row-major order, visiting only unfilled cells.
            for row in 0..9 {
                let mut pending = self.empty[row];
                while pending != 0 {
                    let column = pending.trailing_zeros() as usize;
                    pending &= pending - 1;
                    let index = row * 9 + column;
                    let mask = &mut candidates[index];
                    *mask = ALL_DIGITS
                        & !(self.rows[row]
                            | self.columns[column]
                            | self.boxes[row / 3 * 3 + column / 3]);
                    if *mask == 0 {
                        return None;
                    }
                    if mask.is_power_of_two() {
                        self.place(index, mask.trailing_zeros() as u8);
                        collector.result.stats.forced_moves += 1;
                        collector.observe(
                            || TraceKind::Assignment {
                                cell: cell_index(index),
                                digit: search_digit(self.cells[index]),
                                forced: true,
                            },
                            depth,
                            || (self.cells, None),
                        );
                        if collector.stopped() {
                            return None;
                        }
                        changed = true;
                    }
                }
            }
            // Recompute after assignments before using the cached masks for hidden singles.
            if changed {
                continue;
            }
            collector.observe(
                || TraceKind::Candidates,
                depth,
                || (self.cells, Some(candidates)),
            );
            if collector.stopped() {
                return None;
            }
            'units: for (unit, cells) in UNITS.iter().enumerate() {
                let used = match unit {
                    0..9 => self.rows[unit],
                    9..18 => self.columns[unit - 9],
                    _ => self.boxes[unit - 18],
                };
                let unique = unique_candidates(&candidates, cells, ALL_DIGITS & !used)?;
                if unique != 0 {
                    let digit = unique.trailing_zeros() as u8;
                    let bit = 1 << digit;
                    let &index = cells.iter().find(|&&index| candidates[index] & bit != 0)?;
                    self.place(index, digit);
                    collector.result.stats.forced_moves += 1;
                    collector.observe(
                        || TraceKind::Assignment {
                            cell: cell_index(index),
                            digit: search_digit(self.cells[index]),
                            forced: true,
                        },
                        depth,
                        || (self.cells, None),
                    );
                    if collector.stopped() {
                        return None;
                    }
                    changed = true;
                    break 'units;
                }
            }
            if !changed {
                return Some(candidates);
            }
        }
    }
}

fn unique_candidates(candidates: &[u16; 81], cells: &[usize; 9], missing: u16) -> Option<u16> {
    // Track digits seen once versus at least twice in one pass over the unit.
    let (seen, twice) = cells.iter().fold((0, 0), |(seen, twice), &cell| {
        let mask = candidates[cell];
        (seen | mask, twice | (seen & mask))
    });
    if missing & !seen != 0 {
        return None;
    }
    Some(seen & !twice)
}

fn select(candidates: &[u16; 81]) -> Option<(usize, u16)> {
    debug_assert!(
        candidates
            .iter()
            .all(|&mask| mask == 0 || !mask.is_power_of_two())
    );
    let mut best = None;
    let mut size = 10;
    for (cell, &mask) in candidates.iter().enumerate() {
        if mask == 0 {
            continue;
        }
        let count = mask.count_ones();
        if count < size {
            best = Some((cell, mask));
            size = count;
            // Propagation exhausted singles, so the first pair is optimal.
            if count == 2 {
                break;
            }
        }
    }
    best
}

/// Bitmask search with MRV selection and naked/hidden single propagation.
#[derive(Clone, Copy, Debug, Default)]
pub struct Bitmask;

impl Solver for Bitmask {
    fn name(&self) -> &'static str {
        "bitmask"
    }

    fn search(&self, puzzle: &Board, limit: NonZeroUsize) -> SearchResult {
        collect(limit, |collector| visit(State::new(puzzle), collector, 0)).finish(puzzle)
    }
}

impl super::TraceSolver for Bitmask {
    fn trace_search(
        &self,
        puzzle: &Board,
        limit: NonZeroUsize,
        sink: &mut dyn TraceSink,
    ) -> super::TraceOutcome {
        collect_traced(limit, self.name(), puzzle, sink, |collector| {
            visit(State::new(puzzle), collector, 0)
        })
    }
}

fn visit<O: Observer>(mut state: State, collector: &mut Collector<O>, depth: usize) {
    collector.visit(depth);
    collector.observe(|| TraceKind::Node, depth, || (state.cells, None));
    if collector.stopped() {
        return;
    }
    let before = collector.result.solutions_found;
    let Some(candidates) = state.propagate_observed(collector, depth) else {
        if collector.stopped() {
            return;
        }
        collector.result.stats.backtracks += 1;
        collector.observe(|| TraceKind::Contradiction, depth, || (state.cells, None));
        return;
    };
    let Some((index, mut mask)) = select(&candidates) else {
        collector.record(&state.cells);
        collector.observe(
            || TraceKind::Solution,
            depth,
            || (state.cells, Some(candidates)),
        );
        return;
    };
    collector.result.stats.guesses += 1;
    while mask != 0 {
        let digit = mask.trailing_zeros() as u8;
        mask &= mask - 1;
        // The compact snapshot restores both propagation and the branch assignment.
        let mut child = state.clone();
        child.place(index, digit);
        collector.observe(
            || TraceKind::Assignment {
                cell: cell_index(index),
                digit: search_digit(digit),
                forced: false,
            },
            depth + 1,
            || (child.cells, None),
        );
        if collector.stopped() {
            return;
        }
        visit(child, collector, depth + 1);
        collector.observe(
            || TraceKind::Restore,
            depth,
            || (state.cells, Some(candidates)),
        );
        if collector.stopped() || collector.result.limit_reached() {
            break;
        }
    }
    if collector.stopped() {
        return;
    }
    if before == collector.result.solutions_found {
        collector.result.stats.backtracks += 1;
        collector.observe(
            || TraceKind::Contradiction,
            depth,
            || (state.cells, Some(candidates)),
        );
    }
}

impl GridGenerator for Bitmask {
    fn generate_grid(&self, rng: &mut (impl Rng + ?Sized)) -> Option<Board> {
        random_visit(State::new(&Board::default()), rng)
    }
}

fn random_visit(mut state: State, rng: &mut (impl Rng + ?Sized)) -> Option<Board> {
    let candidates = state.propagate(&mut 0)?;
    let Some((index, mask)) = select(&candidates) else {
        return Some(
            Board::try_from(state.cells)
                .expect("randomized search produces a valid completed board"),
        );
    };
    for digit in shuffled_digits(mask, rng) {
        let mut child = state.clone();
        child.place(index, digit);
        if let Some(solution) = random_visit(child, rng) {
            return Some(solution);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn solver_matches_generated_cases_and_enumerates_caps() {
        super::super::tests::assert_solver_contract(&Bitmask);
    }

    #[test]
    fn bit_parallel_support_matches_scalar_counts_for_every_nine_bit_pattern() {
        for pattern in 0_u16..512 {
            let mut candidates = [0; 81];
            for (position, &cell) in UNITS[18].iter().enumerate() {
                for digit in 1..=9 {
                    if pattern.rotate_left(digit) & (1 << position) != 0 {
                        candidates[cell] |= 1 << digit;
                    }
                }
            }
            let mut seen = 0;
            let mut unique = 0;
            for digit in 1..=9 {
                let count = UNITS[18]
                    .iter()
                    .filter(|&&cell| candidates[cell] & (1 << digit) != 0)
                    .count();
                if count != 0 {
                    seen |= 1 << digit;
                }
                if count == 1 {
                    unique |= 1 << digit;
                }
            }
            assert_eq!(
                unique_candidates(&candidates, &UNITS[18], seen),
                Some(unique)
            );
            if seen != ALL_DIGITS {
                assert_eq!(unique_candidates(&candidates, &UNITS[18], ALL_DIGITS), None);
            }
        }
    }

    #[test]
    fn candidate_mask_combines_row_column_and_box_constraints() {
        let mut cells = [0; 81];
        cells[0] = 1;
        cells[10] = 2;
        cells[27] = 3;
        let mut state = State::new(&Board::try_from(cells).unwrap());
        assert_eq!(
            state.candidates(9),
            ALL_DIGITS & !((1 << 1) | (1 << 2) | (1 << 3))
        );
        state.place(9, 4);
        for peer in [17, 72, 20] {
            assert_eq!(state.candidates(peer) & (1 << 4), 0);
        }
        assert_ne!(state.candidates(80) & (1 << 4), 0);
    }

    #[test]
    fn mrv_selects_the_smallest_nonempty_candidate_set() {
        let mut candidates = [0; 81];
        candidates[1] = 0b1110;
        candidates[80] = 0b1100;
        assert_eq!(select(&candidates), Some((80, 0b1100)));
        assert_eq!(select(&[0; 81]), None);
    }
    #[test]
    fn completion_after_propagation_returns_only_empty_candidate_masks() {
        let solution: Board =
            "534678912672195348198342567859761423426853791713924856961537284287419635345286179"
                .parse()
                .unwrap();
        for removed in [0, 1, 9] {
            let mut cells = *solution.cells();
            cells[..removed].fill(0);
            let mut state = State::new(&Board::try_from(cells).unwrap());
            let mut forced = 0;
            assert_eq!(state.propagate(&mut forced), Some([0; 81]));
            assert_eq!(state.cells, *solution.cells());
            assert_eq!(forced, removed as u64);
        }
    }

    #[test]
    fn optimized_bitmask_matches_dlx_on_holdout_generation_and_extreme_floors() {
        use crate::{DancingLinks, MinClues, generate};
        for (floor, count) in [(48, 64), (36, 64), (28, 64), (22, 64), (17, 8), (81, 8)] {
            for offset in 0..count {
                let seed = 100_000 + floor as u64 * 1000 + offset;
                let generated = generate(seed, MinClues::try_from(floor).unwrap()).unwrap();
                for cap in [1, 2, 3] {
                    let cap = NonZeroUsize::new(cap).unwrap();
                    let actual = Bitmask.search(generated.puzzle(), cap);
                    let expected = DancingLinks.search(generated.puzzle(), cap);
                    assert_eq!(actual.solutions_found(), expected.solutions_found());
                    assert_eq!(actual.extent(), expected.extent());
                    assert_eq!(actual.solution(), Some(generated.solution()));
                }
            }
        }
    }

    #[test]
    fn row_empty_bits_follow_assignments_and_restore_with_branch_snapshots() {
        let check = |state: &State| {
            for row in 0..9 {
                let expected = state.cells[row * 9..row * 9 + 9]
                    .iter()
                    .enumerate()
                    .fold(0_u16, |bits, (column, &digit)| {
                        bits | (u16::from(digit == 0) << column)
                    });
                assert_eq!(state.empty[row], expected);
            }
        };
        println!(
            "Bitmask State: size={} alignment={}",
            std::mem::size_of::<State>(),
            std::mem::align_of::<State>()
        );
        for floor in [48, 36, 28, 22] {
            for offset in 0..8 {
                let generated = crate::generate(
                    floor as u64 * 1000 + offset,
                    crate::MinClues::try_from(floor).unwrap(),
                )
                .unwrap();
                let mut state = State::new(generated.puzzle());
                check(&state);
                let candidates = state.propagate(&mut 0).unwrap();
                check(&state);
                let saved = state.clone();
                if let Some((cell, mut mask)) = select(&candidates) {
                    while mask != 0 {
                        let digit = mask.trailing_zeros() as u8;
                        mask &= mask - 1;
                        let mut child = state.clone();
                        child.place(cell, digit);
                        check(&child);
                        let _ = child.propagate(&mut 0);
                        check(&child);
                        assert_eq!(state.cells, saved.cells);
                        assert_eq!(state.empty, saved.empty);
                    }
                }
            }
        }
    }
}
