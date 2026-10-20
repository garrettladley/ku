use std::num::NonZeroUsize;

use super::{
    Collector, Observer, SearchResult, Solver, collect, collect_traced,
    domains::LogicalDomains as Domains,
};
use crate::{
    Board, TraceKind, TraceSink,
    algorithms::topology::{ALL_DIGITS, UNITS, box_index},
};

/// MRV search with singles, naked pairs, and pointing/claiming locked candidates.
#[derive(Clone, Copy, Debug, Default)]
pub struct ConstraintPropagation;

impl Solver for ConstraintPropagation {
    fn name(&self) -> &'static str {
        "constraint-propagation"
    }

    fn search(&self, puzzle: &Board, limit: NonZeroUsize) -> SearchResult {
        collect(limit, |collector| {
            if let Some(state) = Domains::new(puzzle) {
                visit(state, collector, 0);
            } else {
                collector.visit(0);
                collector.result.stats.backtracks += 1;
                collector.observe(|| TraceKind::Contradiction, 0, || (*puzzle.cells(), None));
            }
        })
        .finish(puzzle)
    }
}

impl super::TraceSolver for ConstraintPropagation {
    fn trace_search(
        &self,
        puzzle: &Board,
        limit: NonZeroUsize,
        sink: &mut dyn TraceSink,
    ) -> super::TraceOutcome {
        collect_traced(limit, self.name(), puzzle, sink, |collector| {
            if let Some(state) = Domains::new(puzzle) {
                visit(state, collector, 0);
            } else {
                collector.visit(0);
                collector.result.stats.backtracks += 1;
                collector.observe(|| TraceKind::Contradiction, 0, || (*puzzle.cells(), None));
            }
        })
    }
}

#[cfg(test)]
fn hidden_singles(state: &mut Domains, unit: &[usize; 9], forced: &mut u64) -> Option<bool> {
    let mut output = None;
    let result = collect(NonZeroUsize::MIN, |collector| {
        output = Some(hidden_singles_observed(state, unit, collector, 0));
    });
    *forced += result.stats.forced_moves;
    output.unwrap()
}
fn hidden_singles_observed<O: Observer>(
    state: &mut Domains,
    unit: &[usize; 9],
    collector: &mut Collector<O>,
    depth: usize,
) -> Option<bool> {
    let mut seen = 0;
    let mut twice = 0;
    let mut used = 0;
    for &cell in unit {
        let mask = if state.cells[cell] == 0 {
            state.masks[cell]
        } else {
            let bit = 1 << state.cells[cell];
            used |= bit;
            bit
        };
        twice |= seen & mask;
        seen |= mask;
    }
    if seen != ALL_DIGITS {
        return None;
    }
    let unique = seen & !twice & !used;
    // Most unit checks have no assignment: skip the second candidate gather.
    if unique == 0 {
        return Some(false);
    }
    for &cell in unit {
        let bits = state.masks[cell] & unique;
        if bits == 0 {
            continue;
        }
        // Two uniquely supported digits in the same cell cannot both be placed.
        if !bits.is_power_of_two() {
            return None;
        }
        state.assign_observed(cell, bits, true, collector, depth)?;
        collector.result.stats.forced_moves += 1;
    }
    Some(true)
}

#[cfg(test)]
fn naked_pairs(state: &mut Domains, unit: &[usize; 9]) -> Option<bool> {
    let mut output = None;
    let _ = collect(NonZeroUsize::MIN, |collector| {
        output = Some(naked_pairs_observed(state, unit, collector, 0));
    });
    output.unwrap()
}
fn naked_pairs_observed<O: Observer>(
    state: &mut Domains,
    unit: &[usize; 9],
    collector: &mut Collector<O>,
    depth: usize,
) -> Option<bool> {
    let mut changed = false;
    for (offset, &first) in unit.iter().enumerate() {
        let pair = state.masks[first];
        if pair.count_ones() != 2 {
            continue;
        }
        for &second in &unit[offset + 1..] {
            if state.masks[second] != pair {
                continue;
            }
            for &cell in unit {
                if cell != first && cell != second {
                    changed |= state.eliminate_observed(cell, pair, collector, depth)?;
                }
            }
        }
    }
    Some(changed)
}

struct Intersection {
    shared: [usize; 3],
    box_rest: [usize; 6],
    line_rest: [usize; 6],
    units: u32,
}

const INTERSECTIONS: [Intersection; 54] = make_intersections();

const fn make_intersections() -> [Intersection; 54] {
    let mut intersections = [const {
        Intersection {
            shared: [0; 3],
            box_rest: [0; 6],
            line_rest: [0; 6],
            units: 0,
        }
    }; 54];
    let mut index = 0;
    while index < 54 {
        let box_id = index / 6;
        let axis = index % 6 / 3;
        let offset = index % 3;
        let line = if axis == 0 {
            box_id / 3 * 3 + offset
        } else {
            9 + box_id % 3 * 3 + offset
        };
        intersections[index].units = (1 << line) | (1 << (18 + box_id));
        let mut shared = 0;
        let mut box_rest = 0;
        let mut line_rest = 0;
        let mut position = 0;
        while position < 9 {
            let cell = UNITS[line][position];
            if box_index(cell) == box_id {
                intersections[index].shared[shared] = cell;
                shared += 1;
            } else {
                intersections[index].line_rest[line_rest] = cell;
                line_rest += 1;
            }
            let cell = UNITS[18 + box_id][position];
            let in_line = if axis == 0 {
                cell / 9 == line
            } else {
                cell % 9 == line - 9
            };
            if !in_line {
                intersections[index].box_rest[box_rest] = cell;
                box_rest += 1;
            }
            position += 1;
        }
        index += 1;
    }
    intersections
}

// Compile inverse dependencies once; a pass only visits affected intersections.
const INTERSECTIONS_BY_UNIT: [u64; 27] = {
    let mut masks = [0; 27];
    let mut index = 0;
    while index < INTERSECTIONS.len() {
        let mut units = INTERSECTIONS[index].units;
        while units != 0 {
            let unit = units.trailing_zeros() as usize;
            units &= units - 1;
            masks[unit] |= 1 << index;
        }
        index += 1;
    }
    masks
};

fn affected_intersections(mut dirty: u32) -> u64 {
    // Initialization commonly dirties every unit. Avoid 27 table lookups then.
    if dirty == (1 << 27) - 1 {
        return (1 << 54) - 1;
    }
    let mut affected = 0;
    while dirty != 0 {
        let unit = dirty.trailing_zeros() as usize;
        dirty &= dirty - 1;
        affected |= INTERSECTIONS_BY_UNIT[unit];
    }
    affected
}

#[cfg(test)]
fn locked_candidates(state: &mut Domains, intersection: &Intersection) -> Option<bool> {
    let mut output = None;
    let _ = collect(NonZeroUsize::MIN, |collector| {
        output = Some(locked_candidates_observed(
            state,
            intersection,
            collector,
            0,
        ));
    });
    output.unwrap()
}
fn locked_candidates_observed<O: Observer>(
    state: &mut Domains,
    intersection: &Intersection,
    collector: &mut Collector<O>,
    depth: usize,
) -> Option<bool> {
    let union = |cells: &[usize]| cells.iter().fold(0, |bits, &cell| bits | state.masks[cell]);
    let shared = union(&intersection.shared);
    let pointing = shared & !union(&intersection.box_rest);
    let claiming = shared & !union(&intersection.line_rest);
    let mut changed = false;
    if pointing != 0 {
        for &cell in &intersection.line_rest {
            changed |= state.eliminate_observed(cell, pointing, collector, depth)?;
        }
    }
    if claiming != 0 {
        for &cell in &intersection.box_rest {
            changed |= state.eliminate_observed(cell, claiming, collector, depth)?;
        }
    }
    Some(changed)
}

fn propagate<O: Observer>(
    state: &mut Domains,
    collector: &mut Collector<O>,
    depth: usize,
) -> Option<()> {
    loop {
        state.singles_observed(collector, depth)?;
        if !state.cells.contains(&0) {
            return Some(());
        }
        let dirty = state.take_dirty();
        let mut changed = false;
        // Clearing the lowest bit preserves the original unit and rule order.
        let mut pending_units = dirty;
        while pending_units != 0 {
            let unit = pending_units.trailing_zeros() as usize;
            pending_units &= pending_units - 1;
            changed |= hidden_singles_observed(state, &UNITS[unit], collector, depth)?;
        }
        if changed {
            continue;
        }
        let mut pending_units = dirty;
        while pending_units != 0 {
            let unit = pending_units.trailing_zeros() as usize;
            pending_units &= pending_units - 1;
            changed |= naked_pairs_observed(state, &UNITS[unit], collector, depth)?;
        }
        let mut pending_intersections = affected_intersections(dirty);
        while pending_intersections != 0 {
            let index = pending_intersections.trailing_zeros() as usize;
            pending_intersections &= pending_intersections - 1;
            changed |= locked_candidates_observed(state, &INTERSECTIONS[index], collector, depth)?;
        }
        if !changed {
            return Some(());
        }
    }
}

fn visit<O: Observer>(mut state: Domains, collector: &mut Collector<O>, depth: usize) {
    collector.visit(depth);
    collector.observe(
        || TraceKind::Node,
        depth,
        || (state.cells, Some(state.masks)),
    );
    if collector.stopped() {
        return;
    }
    let before = collector.result.solutions_found;
    if propagate(&mut state, collector, depth).is_none() {
        if collector.stopped() {
            return;
        }
        collector.result.stats.backtracks += 1;
        collector.observe(
            || TraceKind::Contradiction,
            depth,
            || (state.cells, Some(state.masks)),
        );
        return;
    }
    let Some((cell, mut mask)) = state.select() else {
        collector.record(&state.cells);
        collector.observe(
            || TraceKind::Solution,
            depth,
            || (state.cells, Some(state.masks)),
        );
        return;
    };
    collector.result.stats.guesses += 1;
    while mask != 0 {
        let bit = mask & mask.wrapping_neg();
        mask &= mask - 1;
        let mut child = state.clone();
        if child
            .assign_observed(cell, bit, false, collector, depth + 1)
            .is_some()
        {
            visit(child, collector, depth + 1);
        }
        collector.observe(
            || TraceKind::Restore,
            depth,
            || (state.cells, Some(state.masks)),
        );
        if collector.stopped() || collector.result.limit_reached() {
            break;
        }
    }
    if collector.stopped() {
        return;
    }
    if collector.result.solutions_found == before {
        collector.result.stats.backtracks += 1;
        collector.observe(
            || TraceKind::Contradiction,
            depth,
            || (state.cells, Some(state.masks)),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn solver_matches_generated_cases_and_enumerates_caps() {
        super::super::tests::assert_solver_contract(&ConstraintPropagation);
    }

    #[test]
    fn compiled_intersection_frontier_matches_geometry() {
        use rand::{Rng, SeedableRng, rngs::SmallRng};

        let all = (1 << 27) - 1;
        let check = |dirty| {
            let mut expected = 0;
            for (index, intersection) in INTERSECTIONS.iter().enumerate() {
                let first = intersection.shared[0];
                let line = if intersection.shared.iter().all(|cell| cell / 9 == first / 9) {
                    first / 9
                } else {
                    9 + first % 9
                };
                let box_unit = 18 + box_index(first);
                if dirty & ((1 << line) | (1 << box_unit)) != 0 {
                    expected |= 1 << index;
                }
            }
            assert_eq!(affected_intersections(dirty), expected, "dirty={dirty:#x}");
        };
        check(0);
        check(all);
        for first in 0..27 {
            check(1 << first);
            check(all & !(1 << first));
            for second in 0..27 {
                check((1 << first) | (1 << second));
            }
        }
        let mut rng = SmallRng::seed_from_u64(0xaffec7ed);
        for _ in 0..4096 {
            check(rng.random::<u32>() & all);
        }
    }

    // Preserve the previous scan scheduler as an oracle. Rule ordering,
    // candidates, forced-move counts, and dirty state must stay identical.
    fn scan_propagate(state: &mut Domains, collector: &mut Collector) -> Option<()> {
        loop {
            state.singles_observed(collector, 0)?;
            if !state.cells.contains(&0) {
                return Some(());
            }
            let dirty = state.take_dirty();
            let mut changed = false;
            for (index, unit) in UNITS.iter().enumerate() {
                if dirty & (1 << index) != 0 {
                    changed |= hidden_singles_observed(state, unit, collector, 0)?;
                }
            }
            if changed {
                continue;
            }
            for (index, unit) in UNITS.iter().enumerate() {
                if dirty & (1 << index) != 0 {
                    changed |= naked_pairs_observed(state, unit, collector, 0)?;
                }
            }
            for intersection in &INTERSECTIONS {
                if dirty & intersection.units != 0 {
                    changed |= locked_candidates_observed(state, intersection, collector, 0)?;
                }
            }
            if !changed {
                return Some(());
            }
        }
    }

    fn assert_same_propagation(mut state: Domains, branch_depth: usize) {
        let mut reference = state.clone();
        let mut actual = None;
        let mut expected = None;
        let actual_stats = collect(NonZeroUsize::MIN, |collector| {
            actual = propagate(&mut state, collector, 0);
        });
        let expected_stats = collect(NonZeroUsize::MIN, |collector| {
            expected = scan_propagate(&mut reference, collector);
        });
        assert_eq!(actual, expected);
        assert_eq!(actual_stats.stats, expected_stats.stats);
        assert_eq!(state.cells, reference.cells);
        assert_eq!(state.masks, reference.masks);
        assert_eq!(state.clone().take_dirty(), reference.take_dirty());
        if actual.is_none() || branch_depth == 0 {
            return;
        }
        if let Some((cell, mut choices)) = state.select() {
            while choices != 0 {
                let bit = choices & choices.wrapping_neg();
                choices &= choices - 1;
                let mut child = state.clone();
                if child.assign(cell, bit).is_some() {
                    assert_same_propagation(child, branch_depth - 1);
                }
            }
        }
    }

    #[test]
    fn frontier_preserves_scan_propagation_across_generated_branches() {
        for record in super::super::tests::generated_records() {
            assert_same_propagation(Domains::new(record.puzzle()).unwrap(), 3);
        }
        for floor in [17, 22, 28, 36, 48, 81] {
            for seed in 600_000..600_008 {
                let generated =
                    crate::generate(seed, crate::MinClues::try_from(floor).unwrap()).unwrap();
                assert_same_propagation(Domains::new(generated.puzzle()).unwrap(), 3);
            }
        }
        assert_same_propagation(Domains::new(&Board::default()).unwrap(), 2);
    }

    #[test]
    fn no_hidden_single_preserves_candidates_and_forced_moves() {
        let mut state = Domains::new(&Board::default()).unwrap();
        let before = state.masks;
        let mut forced = 0;
        assert_eq!(
            hidden_singles(&mut state, &UNITS[0], &mut forced),
            Some(false)
        );
        assert_eq!(state.masks, before);
        assert_eq!(state.cells, [0; 81]);
        assert_eq!(forced, 0);
    }

    #[test]
    fn hidden_single_is_found_among_multivalued_cells() {
        let mut state = Domains::new(&Board::default()).unwrap();
        for cell in 1..9 {
            state.masks[cell] &= !(1 << 9);
        }
        let mut forced = 0;
        assert_eq!(
            hidden_singles(&mut state, &UNITS[0], &mut forced),
            Some(true)
        );
        assert_eq!(state.cells[0], 9);
        assert_eq!(forced, 1);
    }

    #[test]
    fn hidden_singles_detect_missing_support_and_two_digits_in_one_cell() {
        let mut state = Domains::new(&Board::default()).unwrap();
        for cell in 0..9 {
            state.masks[cell] &= !(1 << 9);
        }
        assert_eq!(hidden_singles(&mut state, &UNITS[0], &mut 0), None);
        state.masks[0] = ALL_DIGITS;
        for cell in 1..9 {
            state.masks[cell] &= !(1 << 8);
        }
        assert_eq!(hidden_singles(&mut state, &UNITS[0], &mut 0), None);
    }

    #[test]
    fn naked_pair_eliminates_both_digits_from_the_rest_of_a_unit() {
        for unit in [&UNITS[0], &UNITS[9], &UNITS[18]] {
            let mut state = Domains::new(&Board::default()).unwrap();
            let pair = (1 << 2) | (1 << 7);
            state.masks[unit[0]] = pair;
            state.masks[unit[8]] = pair;
            assert_eq!(naked_pairs(&mut state, unit), Some(true));
            for (position, &cell) in unit.iter().enumerate() {
                assert_eq!(
                    state.masks[cell],
                    if position == 0 || position == 8 {
                        pair
                    } else {
                        ALL_DIGITS & !pair
                    }
                );
            }
            assert_eq!(naked_pairs(&mut state, unit), Some(false));
        }
    }

    #[test]
    fn three_identical_pairs_are_a_hall_contradiction() {
        let mut state = Domains::new(&Board::default()).unwrap();
        for cell in 0..3 {
            state.masks[cell] = (1 << 1) | (1 << 2);
        }
        assert_eq!(naked_pairs(&mut state, &UNITS[0]), None);
    }

    #[test]
    fn overlapping_pairs_and_a_lone_pair_do_not_justify_elimination() {
        for unit in [&UNITS[0], &UNITS[9], &UNITS[18]] {
            for second in [ALL_DIGITS, (1 << 2) | (1 << 8)] {
                let mut state = Domains::new(&Board::default()).unwrap();
                state.masks[unit[0]] = (1 << 2) | (1 << 7);
                state.masks[unit[8]] = second;
                let before = state.masks;
                assert_eq!(naked_pairs(&mut state, unit), Some(false));
                assert_eq!(state.masks, before);
                assert_eq!(state.cells, [0; 81]);
            }
        }
    }

    #[test]
    fn locked_candidates_require_all_support_to_be_confined_to_the_intersection() {
        for intersection in &INTERSECTIONS {
            for pointing in [true, false] {
                let mut state = Domains::new(&Board::default()).unwrap();
                let source = if pointing {
                    &intersection.box_rest
                } else {
                    &intersection.line_rest
                };
                for &cell in &source[1..] {
                    state.masks[cell] &= !(1 << 8);
                }
                // A single remaining support outside the intersection prevents the deduction.
                let before = state.masks;
                assert_eq!(locked_candidates(&mut state, intersection), Some(false));
                assert_eq!(state.masks, before);
            }
        }
    }

    #[test]
    fn a_digit_already_placed_in_a_unit_is_not_another_hidden_single() {
        for unit in [&UNITS[0], &UNITS[9], &UNITS[18]] {
            let mut state = Domains::new(&Board::default()).unwrap();
            state.assign(unit[8], 1 << 9).unwrap();
            let before = state.masks;
            let mut forced = 0;
            assert_eq!(hidden_singles(&mut state, unit, &mut forced), Some(false));
            assert_eq!(forced, 0);
            assert_eq!(state.masks, before);
            assert_eq!(state.cells[unit[8]], 9);
        }
    }

    #[test]
    fn every_box_line_intersection_supports_pointing_and_claiming() {
        for intersection in &INTERSECTIONS {
            for pointing in [true, false] {
                let mut state = Domains::new(&Board::default()).unwrap();
                let (source, target) = if pointing {
                    (&intersection.box_rest, &intersection.line_rest)
                } else {
                    (&intersection.line_rest, &intersection.box_rest)
                };
                for &cell in source {
                    state.masks[cell] &= !(1 << 8);
                }
                assert_eq!(locked_candidates(&mut state, intersection), Some(true));
                for &cell in target {
                    assert_eq!(state.masks[cell] & (1 << 8), 0);
                }
                for &cell in &intersection.shared {
                    assert_ne!(state.masks[cell] & (1 << 8), 0);
                }
            }
        }
    }

    #[test]
    fn intersections_partition_each_box_and_line() {
        for intersection in &INTERSECTIONS {
            let first = intersection.shared[0];
            let box_id = box_index(first);
            for &cell in intersection.shared.iter().chain(&intersection.box_rest) {
                assert_eq!(box_index(cell), box_id);
            }
            let row = intersection
                .shared
                .iter()
                .all(|&cell| cell / 9 == first / 9);
            for &cell in &intersection.line_rest {
                assert_ne!(box_index(cell), box_id);
                assert!(if row {
                    cell / 9 == first / 9
                } else {
                    cell % 9 == first % 9
                });
            }
        }
    }
}
