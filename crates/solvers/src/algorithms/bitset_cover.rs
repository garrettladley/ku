use std::num::NonZeroUsize;

use super::{
    Collector, Observer, SearchResult, Solver, cell_index, collect, collect_traced, search_digit,
    topology::{MEMBERSHIPS, PEERS},
};
use crate::{
    Board, TraceKind, TraceSink,
    algorithms::topology::{UNITS, box_index},
};

const ALL_CHOICES: u16 = (1 << 9) - 1;
const COVERED: u16 = 1 << 15;

struct Assignment {
    columns: [u16; 4],
    bits: [u16; 4],
}

const ASSIGNMENTS: [Assignment; 729] = make_assignments();

const fn make_assignments() -> [Assignment; 729] {
    let mut assignments = [const {
        Assignment {
            columns: [0; 4],
            bits: [0; 4],
        }
    }; 729];
    let mut assignment = 0;
    while assignment < 729 {
        let cell = assignment / 9;
        let digit = assignment % 9;
        assignments[assignment].columns[0] = cell as u16;
        assignments[assignment].bits[0] = 1 << digit;
        let mut member = 0;
        while member < 3 {
            let (unit, position) = MEMBERSHIPS[cell][member];
            assignments[assignment].columns[member + 1] = (81 + unit * 9 + digit) as u16;
            assignments[assignment].bits[member + 1] = 1 << position;
            member += 1;
        }
        assignment += 1;
    }
    assignments
}

// A live peer choice cannot survive in a covered unit-digit column. Once an
// assignment covers its units, only the peer's nonshared units need updating.
struct PeerConflict {
    cell: u8,
    // Nine low bits encode the unit's column base; four high bits encode
    // the peer's position. Zero terminates the one-support case.
    supports: [u16; 2],
}

const PEER_CONFLICTS: [[PeerConflict; 20]; 81] = make_peer_conflicts();

const fn make_peer_conflicts() -> [[PeerConflict; 20]; 81] {
    let mut conflicts = [const {
        [const {
            PeerConflict {
                cell: 0,
                supports: [0; 2],
            }
        }; 20]
    }; 81];
    let mut cell = 0;
    while cell < 81 {
        let mut peer_index = 0;
        while peer_index < 20 {
            let peer = PEERS[cell][peer_index];
            conflicts[cell][peer_index].cell = peer as u8;
            let mut member = 0;
            let mut count = 0;
            while member < 3 {
                let (unit, position) = MEMBERSHIPS[peer][member];
                if unit != MEMBERSHIPS[cell][0].0
                    && unit != MEMBERSHIPS[cell][1].0
                    && unit != MEMBERSHIPS[cell][2].0
                {
                    conflicts[cell][peer_index].supports[count] =
                        (81 + unit * 9) as u16 | ((position as u16) << 9);
                    count += 1;
                }
                member += 1;
            }
            assert!(count >= 1 && count <= 2);
            peer_index += 1;
        }
        cell += 1;
    }
    conflicts
}

fn decode(column: usize, choice: usize) -> usize {
    if column < 81 {
        column * 9 + choice
    } else {
        let unit_digit = column - 81;
        UNITS[unit_digit / 9][choice] * 9 + unit_digit % 9
    }
}

#[derive(Clone)]
struct Matrix {
    // Each of Sudoku's 324 exact-cover columns has exactly nine possible rows.
    // Local nine-bit supports avoid 729-bit row sets and linked-list traversal.
    options: [u16; 324],
    cells: [u8; 81],
    pending: [u64; 6],
}

impl Matrix {
    fn new(puzzle: &Board) -> Option<Self> {
        let mut matrix = Self {
            options: [0; 324],
            cells: *puzzle.cells(),
            pending: [0; 6],
        };
        let mut rows = [0_u16; 9];
        let mut columns = [0_u16; 9];
        let mut boxes = [0_u16; 9];
        for (cell, &digit) in puzzle.cells().iter().enumerate() {
            if digit == 0 {
                continue;
            }
            let digit = usize::from(digit - 1);
            let bit = 1 << digit;
            rows[cell / 9] |= bit;
            columns[cell % 9] |= bit;
            boxes[box_index(cell)] |= bit;
            for &column in &ASSIGNMENTS[cell * 9 + digit].columns {
                let options = &mut matrix.options[usize::from(column)];
                if *options == COVERED {
                    return None;
                }
                *options = COVERED;
            }
        }
        // Construct each surviving support once rather than repeatedly removing
        // rows while replaying givens. Legal assignments cannot touch a column
        // covered by a given, because occupancy excludes that unit's digit.
        for (cell, &digit) in puzzle.cells().iter().enumerate() {
            if digit != 0 {
                continue;
            }
            let mut choices =
                ALL_CHOICES & !(rows[cell / 9] | columns[cell % 9] | boxes[box_index(cell)]);
            if choices == 0 {
                return None;
            }
            // The cell support is already complete; only unit supports need
            // accumulating once per surviving assignment.
            matrix.options[cell] = choices;
            while choices != 0 {
                let digit = choices.trailing_zeros() as usize;
                choices &= choices - 1;
                let assignment = &ASSIGNMENTS[cell * 9 + digit];
                for (&column, &bit) in assignment.columns[1..].iter().zip(&assignment.bits[1..]) {
                    matrix.options[usize::from(column)] |= bit;
                }
            }
        }
        for (column, &options) in matrix.options.iter().enumerate() {
            if options == COVERED {
                continue;
            }
            if options == 0 {
                return None;
            }
            if options.is_power_of_two() {
                matrix.pending[column / 64] |= 1 << (column % 64);
            }
        }
        Some(matrix)
    }

    #[cfg(test)]
    fn eliminate(&mut self, assignment: usize) -> Option<()> {
        let mut output = None;
        let _ = collect(NonZeroUsize::MIN, |collector| {
            output = Some(self.eliminate_observed(assignment, collector, 0));
        });
        output.unwrap()
    }
    #[inline(always)]
    fn eliminate_observed<O: Observer>(
        &mut self,
        assignment: usize,
        collector: &mut Collector<O>,
        depth: usize,
    ) -> Option<()> {
        let row = &ASSIGNMENTS[assignment];
        for (&column, &bit) in row.columns.iter().zip(&row.bits) {
            let column = usize::from(column);
            if self.options[column] & bit != 0 {
                self.remove_support_observed(column, bit, collector, depth)?;
            }
        }
        Some(())
    }

    #[inline(always)]
    fn remove_support_observed<O: Observer>(
        &mut self,
        column: usize,
        bit: u16,
        collector: &mut Collector<O>,
        depth: usize,
    ) -> Option<()> {
        debug_assert_ne!(self.options[column] & bit, 0);
        let options = self.options[column] & !bit;
        self.options[column] = options;
        if options == 0 {
            collector.observe(
                || TraceKind::Contradiction,
                depth,
                || (self.cells, Some(self.candidate_masks())),
            );
            return None;
        }
        if options.is_power_of_two() {
            self.pending[column / 64] |= 1 << (column % 64);
        }
        collector.observe(
            || TraceKind::Candidates,
            depth,
            || (self.cells, Some(self.candidate_masks())),
        );
        if collector.stopped() {
            return None;
        }
        Some(())
    }

    #[cfg(test)]
    fn cover(&mut self, assignment: usize) -> Option<()> {
        let mut output = None;
        let _ = collect(NonZeroUsize::MIN, |collector| {
            output = Some(self.cover_observed(assignment, false, collector, 0));
        });
        output.unwrap()
    }
    fn cover_observed<O: Observer>(
        &mut self,
        assignment: usize,
        forced: bool,
        collector: &mut Collector<O>,
        depth: usize,
    ) -> Option<()> {
        let cell = assignment / 9;
        let digit = assignment % 9;
        let bit = 1 << digit;
        let mut alternatives = self.options[cell];
        if alternatives & bit == 0 {
            return None;
        }
        for &column in &ASSIGNMENTS[assignment].columns {
            let column = usize::from(column);
            self.options[column] = COVERED;
            self.pending[column / 64] &= !(1 << (column % 64));
        }
        self.cells[cell] = (digit + 1) as u8;
        collector.observe(
            || TraceKind::Assignment {
                cell: cell_index(cell),
                digit: search_digit(self.cells[cell]),
                forced,
            },
            depth,
            || (self.cells, Some(self.candidate_masks())),
        );
        if collector.stopped() {
            return None;
        }
        alternatives &= !bit;
        while alternatives != 0 {
            let other = alternatives.trailing_zeros() as usize;
            alternatives &= alternatives - 1;
            self.eliminate_observed(cell * 9 + other, collector, depth)?;
        }
        for conflict in &PEER_CONFLICTS[cell] {
            let peer = usize::from(conflict.cell);
            if self.options[peer] & bit == 0 {
                continue;
            }
            self.remove_support_observed(peer, bit, collector, depth)?;
            for &support in &conflict.supports {
                if support == 0 {
                    break;
                }
                let column = usize::from(support & ALL_CHOICES) + digit;
                let position = support >> 9;
                self.remove_support_observed(column, 1 << position, collector, depth)?;
            }
        }
        Some(())
    }

    fn candidate_masks(&self) -> [u16; 81] {
        std::array::from_fn(|cell| {
            if self.options[cell] == COVERED {
                0
            } else {
                self.options[cell] << 1
            }
        })
    }

    fn select(&self) -> Option<(usize, u16)> {
        let mut best = None;
        let mut size = 10;
        for (column, &options) in self.options.iter().enumerate() {
            if options == COVERED {
                continue;
            }
            let count = options.count_ones();
            if count < size {
                best = Some((column, options));
                size = count;
                if count <= 2 {
                    break;
                }
            }
        }
        best
    }

    fn pop_single(&mut self) -> Option<usize> {
        for (word, pending) in self.pending.iter_mut().enumerate() {
            if *pending != 0 {
                let bit = pending.trailing_zeros() as usize;
                *pending &= *pending - 1;
                return Some(word * 64 + bit);
            }
        }
        None
    }

    fn visit<O: Observer>(mut self, collector: &mut Collector<O>, depth: usize) {
        collector.visit(depth);
        collector.observe(
            || TraceKind::Node,
            depth,
            || (self.cells, Some(self.candidate_masks())),
        );
        if collector.stopped() {
            return;
        }
        let before = collector.result.solutions_found;
        // Supports shrink monotonically: process each new singleton directly
        // instead of scanning 324 columns after every forced assignment.
        while let Some(column) = self.pop_single() {
            let options = self.options[column];
            collector.result.stats.forced_moves += 1;
            if self
                .cover_observed(
                    decode(column, options.trailing_zeros() as usize),
                    true,
                    collector,
                    depth,
                )
                .is_none()
            {
                if collector.stopped() {
                    return;
                }
                collector.result.stats.backtracks += 1;
                collector.observe(
                    || TraceKind::Contradiction,
                    depth,
                    || (self.cells, Some(self.candidate_masks())),
                );
                return;
            }
        }
        let Some((column, mut options)) = self.select() else {
            collector.record(&self.cells);
            collector.observe(
                || TraceKind::Solution,
                depth,
                || (self.cells, Some(self.candidate_masks())),
            );
            return;
        };
        collector.result.stats.guesses += 1;
        while options != 0 {
            let choice = options.trailing_zeros() as usize;
            options &= options - 1;
            let mut child = self.clone();
            if child
                .cover_observed(decode(column, choice), false, collector, depth + 1)
                .is_some()
            {
                child.visit(collector, depth + 1);
            }
            collector.observe(
                || TraceKind::Restore,
                depth,
                || (self.cells, Some(self.candidate_masks())),
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
                || (self.cells, Some(self.candidate_masks())),
            );
        }
    }
}

/// Algorithm X with nine-bit support masks for all 324 Sudoku constraints.
#[derive(Clone, Copy, Debug, Default)]
pub struct BitsetCover;

impl Solver for BitsetCover {
    fn name(&self) -> &'static str {
        "bitset-cover"
    }

    fn search(&self, puzzle: &Board, limit: NonZeroUsize) -> SearchResult {
        collect(limit, |collector| {
            if let Some(matrix) = Matrix::new(puzzle) {
                matrix.visit(collector, 0);
            } else {
                collector.visit(0);
                collector.result.stats.backtracks += 1;
                collector.observe(|| TraceKind::Contradiction, 0, || (*puzzle.cells(), None));
            }
        })
        .finish(puzzle)
    }
}

impl super::TraceSolver for BitsetCover {
    fn trace_search(
        &self,
        puzzle: &Board,
        limit: NonZeroUsize,
        sink: &mut dyn TraceSink,
    ) -> super::TraceOutcome {
        collect_traced(limit, self.name(), puzzle, sink, |collector| {
            if let Some(matrix) = Matrix::new(puzzle) {
                matrix.visit(collector, 0);
            } else {
                collector.visit(0);
                collector.result.stats.backtracks += 1;
                collector.observe(|| TraceKind::Contradiction, 0, || (*puzzle.cells(), None));
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn solver_matches_generated_cases_and_enumerates_caps() {
        super::super::tests::assert_solver_contract(&BitsetCover);
    }

    fn scalar_initialization(puzzle: &Board) -> Option<([u16; 324], [u64; 6])> {
        let mut supports = [0; 324];
        let mut covered = [false; 324];
        // Scan all 729 assignments independently of the compiled assignment
        // table and occupancy masks used by production initialization.
        for assignment in 0..729 {
            let cell = assignment / 9;
            let digit = assignment % 9;
            let row = cell / 9;
            let column = cell % 9;
            let box_number = row / 3 * 3 + column / 3;
            let constraints = [
                cell,
                81 + row * 9 + digit,
                162 + column * 9 + digit,
                243 + box_number * 9 + digit,
            ];
            let bits = [
                1 << digit,
                1 << column,
                1 << row,
                1 << (row % 3 * 3 + column % 3),
            ];
            if puzzle.cells()[cell] == (digit + 1) as u8 {
                for constraint in constraints {
                    if covered[constraint] {
                        return None;
                    }
                    covered[constraint] = true;
                }
            }
            if puzzle.cells()[cell] != 0 {
                continue;
            }
            let legal = puzzle.cells().iter().enumerate().all(|(other, &given)| {
                given != (digit + 1) as u8
                    || (other / 9 != row
                        && other % 9 != column
                        && (other / 9 / 3 * 3 + other % 9 / 3) != box_number)
            });
            if legal {
                for (&constraint, &bit) in constraints.iter().zip(&bits) {
                    supports[constraint] |= bit;
                }
            }
        }
        let mut pending = [0; 6];
        for (column, support) in supports.iter_mut().enumerate() {
            if covered[column] {
                *support = COVERED;
            } else if *support == 0 {
                return None;
            } else if support.is_power_of_two() {
                pending[column / 64] |= 1 << (column % 64);
            }
        }
        Some((supports, pending))
    }

    fn replayed_initialization(puzzle: &Board) -> Option<Matrix> {
        let mut matrix = Matrix {
            options: [ALL_CHOICES; 324],
            cells: [0; 81],
            pending: [0; 6],
        };
        for (cell, &digit) in puzzle.cells().iter().enumerate() {
            if digit != 0 {
                matrix.cover(cell * 9 + usize::from(digit - 1))?;
            }
        }
        Some(matrix)
    }

    fn assert_initialization(puzzle: &Board) {
        let scalar = scalar_initialization(puzzle);
        let replayed = replayed_initialization(puzzle);
        let direct = Matrix::new(puzzle);
        assert_eq!(direct.is_some(), scalar.is_some(), "{puzzle}");
        assert_eq!(direct.is_some(), replayed.is_some(), "{puzzle}");
        if let Some(matrix) = direct {
            let (options, pending) = scalar.unwrap();
            let replayed = replayed.unwrap();
            assert_eq!(matrix.cells, *puzzle.cells());
            assert_eq!(matrix.options, options, "{puzzle}");
            assert_eq!(matrix.pending, pending, "{puzzle}");
            assert_eq!(matrix.cells, replayed.cells);
            assert_eq!(matrix.options, replayed.options, "{puzzle}");
            assert_eq!(matrix.pending, replayed.pending, "{puzzle}");
            assert_eq!(matrix.pending[5] & !0b1111, 0);
            for (column, &options) in matrix.options.iter().enumerate() {
                let queued = matrix.pending[column / 64] & (1 << (column % 64)) != 0;
                assert_eq!(queued, options != COVERED && options.is_power_of_two());
            }
        }
    }

    #[test]
    fn direct_initialization_matches_scalar_and_replayed_supports_for_generated_cases() {
        for record in super::super::tests::generated_records() {
            assert_initialization(record.puzzle());
        }
    }

    #[test]
    fn direct_initialization_handles_empty_complete_single_gap_and_contradictory_grids() {
        let solution: Board =
            "534678912672195348198342567859761423426853791713924856961537284287419635345286179"
                .parse()
                .unwrap();
        assert_initialization(&Board::default());
        assert_initialization(&solution);
        for cell in 0..81 {
            let mut cells = *solution.cells();
            cells[cell] = 0;
            let puzzle = Board::try_from(cells).unwrap();
            assert_initialization(&puzzle);
            let matrix = Matrix::new(&puzzle).unwrap();
            assert_eq!(
                matrix
                    .pending
                    .iter()
                    .map(|word| word.count_ones())
                    .sum::<u32>(),
                4
            );
        }
        let mut contradictory = [0; 81];
        contradictory[..8].copy_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8]);
        contradictory[17] = 9;
        let puzzle = Board::try_from(contradictory).unwrap();
        assert_initialization(&puzzle);
        assert!(Matrix::new(&puzzle).is_none());
    }

    #[test]
    fn direct_initialization_rejects_missing_unit_support_with_nonempty_cell_domains() {
        let mut cells = [0; 81];
        cells[4..9].copy_from_slice(&[2, 3, 4, 5, 6]);
        cells[9] = 1;
        cells[39] = 1;
        let puzzle = Board::try_from(cells).unwrap();
        for (cell, &given) in puzzle.cells().iter().enumerate() {
            if given == 0 {
                assert!((1..=9).any(|digit| {
                    PEERS[cell]
                        .iter()
                        .all(|&peer| puzzle.cells()[peer] != digit)
                }));
            }
        }
        assert_initialization(&puzzle);
        assert!(Matrix::new(&puzzle).is_none());
    }

    #[test]
    fn all_four_constraints_decode_to_the_same_assignment() {
        for (assignment, row) in ASSIGNMENTS.iter().enumerate() {
            for (&column, &bit) in row.columns.iter().zip(&row.bits) {
                assert_eq!(
                    decode(usize::from(column), bit.trailing_zeros() as usize),
                    assignment
                );
            }
        }
    }

    #[test]
    fn cover_updates_every_active_constraint_support_consistently() {
        let mut matrix = Matrix::new(&Board::default()).unwrap();
        matrix.cover(40 * 9 + 4).unwrap();
        for (assignment, row) in ASSIGNMENTS.iter().enumerate() {
            let cell = assignment / 9;
            let digit = assignment % 9;
            let legal = cell != 40 && (digit != 4 || !PEERS[40].contains(&cell));
            for (&column, &bit) in row.columns.iter().zip(&row.bits) {
                let options = matrix.options[usize::from(column)];
                if options != COVERED {
                    assert_eq!(
                        options & bit != 0,
                        legal,
                        "assignment={assignment} column={column}"
                    );
                }
            }
        }
        assert_eq!(matrix.cells[40], 5);
        assert_eq!(
            matrix
                .options
                .iter()
                .filter(|&&mask| mask == COVERED)
                .count(),
            4
        );
    }

    #[test]
    fn elimination_detects_a_constraint_with_no_remaining_support() {
        let mut matrix = Matrix::new(&Board::default()).unwrap();
        for digit in 0..8 {
            matrix.eliminate(digit).unwrap();
        }
        assert_eq!(matrix.eliminate(8), None);
    }

    #[test]
    fn selection_can_branch_on_unit_digit_support_instead_of_a_cell() {
        let mut matrix = Matrix::new(&Board::default()).unwrap();
        for cell in 2..9 {
            matrix.eliminate(cell * 9).unwrap();
        }
        assert_eq!(matrix.select(), Some((81, 0b11)));
    }

    #[test]
    fn singleton_queue_spans_all_words_and_cover_removes_stale_entries() {
        let mut matrix = Matrix::new(&Board::default()).unwrap();
        for column in [0, 63, 64, 127, 128, 255, 256, 323] {
            matrix.pending[column / 64] |= 1 << (column % 64);
        }
        matrix.cover(0).unwrap();
        let mut queued = Vec::new();
        while let Some(column) = matrix.pop_single() {
            queued.push(column);
        }
        assert_eq!(queued, [63, 64, 127, 128, 255, 256, 323]);
    }

    #[test]
    fn eliminating_an_assignment_queues_all_new_singleton_supports_once() {
        let mut matrix = Matrix::new(&Board::default()).unwrap();
        for digit in 0..8 {
            matrix.eliminate(digit).unwrap();
        }
        assert_eq!(matrix.pop_single(), Some(0));
        assert_eq!(matrix.pop_single(), None);
        matrix.eliminate(0).unwrap();
        assert_eq!(matrix.pop_single(), None);
    }
    #[test]
    fn packed_peer_supports_are_exactly_the_constraints_not_covered_by_the_assignment() {
        assert_eq!(std::mem::size_of::<PeerConflict>(), 6);
        for (cell, conflicts) in PEER_CONFLICTS.iter().enumerate() {
            for (peer_index, conflict) in conflicts.iter().enumerate() {
                let peer = usize::from(conflict.cell);
                assert_eq!(peer, PEERS[cell][peer_index]);
                for digit in 0..9 {
                    let covered = &ASSIGNMENTS[cell * 9 + digit].columns;
                    let row = &ASSIGNMENTS[peer * 9 + digit];
                    let expected: Vec<_> = row
                        .columns
                        .iter()
                        .zip(&row.bits)
                        .filter(|(column, _)| !covered.contains(column))
                        .map(|(&column, &bit)| (usize::from(column), bit))
                        .collect();
                    let mut actual = vec![(peer, 1 << digit)];
                    for &support in &conflict.supports {
                        if support == 0 {
                            break;
                        }
                        actual.push((
                            usize::from(support & ALL_CHOICES) + digit,
                            1 << (support >> 9),
                        ));
                    }
                    assert_eq!(actual, expected, "cell={cell} peer={peer} digit={digit}");
                }
            }
        }
    }

    fn scalar_cover(matrix: &mut Matrix, assignment: usize) -> Option<()> {
        let cell = assignment / 9;
        let digit = assignment % 9;
        let bit = 1 << digit;
        let mut alternatives = matrix.options[cell];
        if alternatives & bit == 0 {
            return None;
        }
        for &column in &ASSIGNMENTS[assignment].columns {
            let column = usize::from(column);
            matrix.options[column] = COVERED;
            matrix.pending[column / 64] &= !(1 << (column % 64));
        }
        matrix.cells[cell] = (digit + 1) as u8;
        alternatives &= !bit;
        while alternatives != 0 {
            let other = alternatives.trailing_zeros() as usize;
            alternatives &= alternatives - 1;
            matrix.eliminate(cell * 9 + other)?;
        }
        for &peer in &PEERS[cell] {
            if matrix.options[peer] & bit != 0 {
                matrix.eliminate(peer * 9 + digit)?;
            }
        }
        Some(())
    }

    fn assert_cover_matches_scalar(matrix: &Matrix, assignment: usize) {
        let mut expected = matrix.clone();
        let mut actual = matrix.clone();
        assert_eq!(
            actual.cover(assignment),
            scalar_cover(&mut expected, assignment)
        );
        // The same order also preserves the partial matrix on contradiction.
        assert_eq!(actual.options, expected.options, "assignment={assignment}");
        assert_eq!(actual.pending, expected.pending, "assignment={assignment}");
        assert_eq!(actual.cells, expected.cells, "assignment={assignment}");
    }

    #[test]
    fn specialized_peer_elimination_matches_scalar_covers_on_generated_and_propagated_states() {
        for record in super::super::tests::generated_records() {
            let matrix = Matrix::new(record.puzzle()).unwrap();
            for assignment in 0..729 {
                assert_cover_matches_scalar(&matrix, assignment);
            }
            let mut matrix = matrix;
            for (cell, &digit) in record.solution().unwrap().cells().iter().enumerate() {
                if matrix.cells[cell] == 0 {
                    let assignment = cell * 9 + usize::from(digit - 1);
                    assert_cover_matches_scalar(&matrix, assignment);
                    matrix.cover(assignment).unwrap();
                }
            }
            assert_eq!(matrix.cells, *record.solution().unwrap().cells());
        }
        let mut matrix = Matrix::new(&Board::default()).unwrap();
        // Leave one support in cell0; assigning the same digit to its peer
        // must report the same contradiction and partial state as the scalar path.
        for digit in 1..9 {
            matrix.eliminate(digit).unwrap();
        }
        assert_cover_matches_scalar(&matrix, 9);
        assert_eq!(matrix.cover(9), None);
    }
}
