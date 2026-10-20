use std::num::NonZeroUsize;

use super::{
    Collector, Observer, SearchResult, Solver, cell_index, collect, collect_traced, search_digit,
};
use crate::{
    Board, TraceKind, TraceSink,
    algorithms::topology::{ALL_DIGITS, box_index},
};

const COLUMNS: usize = 324;

#[derive(Clone, Debug, PartialEq, Eq)]
struct Node {
    left: usize,
    right: usize,
    up: usize,
    down: usize,
    column: usize,
    assignment: usize,
}

struct Matrix {
    nodes: Vec<Node>,
    sizes: [usize; COLUMNS + 1],
}

impl Matrix {
    fn new(puzzle: &Board) -> Self {
        let mut rows = [0_u16; 9];
        let mut columns = [0_u16; 9];
        let mut boxes = [0_u16; 9];
        for (cell, &digit) in puzzle.cells().iter().enumerate() {
            if digit != 0 {
                let bit = 1 << digit;
                rows[cell / 9] |= bit;
                columns[cell % 9] |= bit;
                boxes[box_index(cell)] |= bit;
            }
        }
        let masks: [u16; 81] = std::array::from_fn(|cell| {
            let digit = puzzle.cells()[cell];
            if digit != 0 {
                1 << digit
            } else {
                ALL_DIGITS & !(rows[cell / 9] | columns[cell % 9] | boxes[box_index(cell)])
            }
        });
        let assignments: usize = masks.iter().map(|mask| mask.count_ones() as usize).sum();
        let mut matrix = Self {
            nodes: Vec::with_capacity(COLUMNS + 1 + 4 * assignments),
            sizes: [0; COLUMNS + 1],
        };
        for column in 0..=COLUMNS {
            matrix.nodes.push(Node {
                left: if column == 0 { COLUMNS } else { column - 1 },
                right: if column == COLUMNS { 0 } else { column + 1 },
                up: column,
                down: column,
                column,
                assignment: 0,
            });
        }
        for (cell, mut mask) in masks.into_iter().enumerate() {
            while mask != 0 {
                let digit = mask.trailing_zeros() as usize - 1;
                mask &= mask - 1;
                let constraints = [
                    cell,
                    81 + cell / 9 * 9 + digit,
                    162 + cell % 9 * 9 + digit,
                    243 + box_index(cell) * 9 + digit,
                ];
                matrix.add_assignment(cell * 9 + digit, constraints);
            }
        }
        matrix
    }

    fn add_assignment(&mut self, assignment: usize, columns: [usize; 4]) {
        let first = self.nodes.len();
        for (offset, column) in columns.into_iter().enumerate() {
            let column = column + 1;
            let index = first + offset;
            let up = self.nodes[column].up;
            self.nodes.push(Node {
                left: first + (offset + 3) % 4,
                right: first + (offset + 1) % 4,
                up,
                down: column,
                column,
                assignment,
            });
            self.nodes[up].down = index;
            self.nodes[column].up = index;
            self.sizes[column] += 1;
        }
    }

    #[inline(always)]
    fn cover(&mut self, column: usize) {
        let left = self.nodes[column].left;
        let right = self.nodes[column].right;
        self.nodes[left].right = right;
        self.nodes[right].left = left;
        let mut row = self.nodes[column].down;
        while row != column {
            let mut node = self.nodes[row].right;
            while node != row {
                let up = self.nodes[node].up;
                let down = self.nodes[node].down;
                self.nodes[up].down = down;
                self.nodes[down].up = up;
                self.sizes[self.nodes[node].column] -= 1;
                node = self.nodes[node].right;
            }
            row = self.nodes[row].down;
        }
    }

    #[inline(always)]
    fn uncover(&mut self, column: usize) {
        // Reverse both traversals: a removal must be undone in LIFO order.
        let mut row = self.nodes[column].up;
        while row != column {
            let mut node = self.nodes[row].left;
            while node != row {
                let up = self.nodes[node].up;
                let down = self.nodes[node].down;
                self.nodes[up].down = node;
                self.nodes[down].up = node;
                self.sizes[self.nodes[node].column] += 1;
                node = self.nodes[node].left;
            }
            row = self.nodes[row].up;
        }
        let left = self.nodes[column].left;
        let right = self.nodes[column].right;
        self.nodes[left].right = column;
        self.nodes[right].left = column;
    }

    fn smallest_column(&self) -> Option<usize> {
        let mut column = self.nodes[0].right;
        let mut best = None;
        let mut size = usize::MAX;
        while column != 0 {
            if self.sizes[column] < size {
                best = Some(column);
                size = self.sizes[column];
                // A singleton is already a forced move. A later empty column
                // is detected by the recursive search after this assignment.
                if size <= 1 {
                    break;
                }
            }
            column = self.nodes[column].right;
        }
        best
    }

    fn visit<O: Observer>(
        &mut self,
        path: &mut [usize; 81],
        givens: &[u8; 81],
        collector: &mut Collector<O>,
        depth: usize,
    ) {
        collector.visit(depth);
        collector.observe(
            || TraceKind::Node,
            depth,
            || (path_cells(path, depth, givens), None),
        );
        if collector.stopped() {
            return;
        }
        let before = collector.result.solutions_found;
        let Some(column) = self.smallest_column() else {
            let mut cells = [0; 81];
            for &assignment in &path[..depth] {
                cells[assignment / 9] = (assignment % 9 + 1) as u8;
            }
            collector.record(&cells);
            collector.observe(|| TraceKind::Solution, depth, || (cells, None));
            return;
        };
        match self.sizes[column] {
            0 => {
                collector.result.stats.backtracks += 1;
                collector.observe(
                    || TraceKind::Contradiction,
                    depth,
                    || (path_cells(path, depth, givens), None),
                );
                return;
            }
            1 => collector.result.stats.forced_moves += 1,
            _ => collector.result.stats.guesses += 1,
        }
        self.cover(column);
        let mut row = self.nodes[column].down;
        while row != column {
            path[depth] = self.nodes[row].assignment;
            let assignment = path[depth];
            collector.observe(
                || TraceKind::Assignment {
                    cell: cell_index(assignment / 9),
                    digit: search_digit((assignment % 9 + 1) as u8),
                    forced: self.sizes[column] == 1,
                },
                depth + 1,
                || (path_cells(path, depth + 1, givens), None),
            );
            if collector.stopped() {
                break;
            }
            let mut node = self.nodes[row].right;
            while node != row {
                self.cover(self.nodes[node].column);
                node = self.nodes[node].right;
            }
            self.visit(path, givens, collector, depth + 1);
            node = self.nodes[row].left;
            while node != row {
                self.uncover(self.nodes[node].column);
                node = self.nodes[node].left;
            }
            collector.observe(
                || TraceKind::Restore,
                depth,
                || (path_cells(path, depth, givens), None),
            );
            if collector.stopped() || collector.result.limit_reached() {
                break;
            }
            row = self.nodes[row].down;
        }
        self.uncover(column);
        if collector.stopped() {
            return;
        }
        if before == collector.result.solutions_found {
            collector.result.stats.backtracks += 1;
            collector.observe(
                || TraceKind::Contradiction,
                depth,
                || (path_cells(path, depth, givens), None),
            );
        }
    }
}

fn path_cells(path: &[usize; 81], depth: usize, givens: &[u8; 81]) -> [u8; 81] {
    let mut cells = *givens;
    for &assignment in &path[..depth] {
        cells[assignment / 9] = (assignment % 9 + 1) as u8;
    }
    cells
}

/// Algorithm X over Sudoku's exact-cover constraints in a safe index arena.
#[derive(Clone, Copy, Debug, Default)]
pub struct DancingLinks;

impl Solver for DancingLinks {
    fn name(&self) -> &'static str {
        "dlx"
    }

    fn search(&self, puzzle: &Board, limit: NonZeroUsize) -> SearchResult {
        collect(limit, |collector| {
            Matrix::new(puzzle).visit(&mut [0; 81], puzzle.cells(), collector, 0)
        })
        .finish(puzzle)
    }
}

impl super::TraceSolver for DancingLinks {
    fn trace_search(
        &self,
        puzzle: &Board,
        limit: NonZeroUsize,
        sink: &mut dyn TraceSink,
    ) -> super::TraceOutcome {
        collect_traced(limit, self.name(), puzzle, sink, |collector| {
            Matrix::new(puzzle).visit(&mut [0; 81], puzzle.cells(), collector, 0)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn solver_matches_generated_cases_and_enumerates_caps() {
        super::super::tests::assert_solver_contract(&DancingLinks);
    }

    #[test]
    fn cancellation_at_each_event_restores_the_entire_arena() {
        use std::ops::ControlFlow;

        let puzzle = Board::default();
        let mut matrix = Matrix::new(&puzzle);
        let nodes = matrix.nodes.clone();
        let sizes = matrix.sizes;
        // Include cancellations before recursion, after each nested removal,
        // and while unwinding a branch. Reuse the arena to expose stale links.
        for stop_after in 0..1024 {
            let mut remaining = stop_after;
            let outcome = collect_traced(
                NonZeroUsize::new(2).unwrap(),
                "dlx",
                &puzzle,
                &mut |_| {
                    if remaining == 0 {
                        ControlFlow::Break(())
                    } else {
                        remaining -= 1;
                        ControlFlow::Continue(())
                    }
                },
                |collector| matrix.visit(&mut [0; 81], puzzle.cells(), collector, 0),
            );
            assert_eq!(matrix.nodes, nodes, "stop_after={stop_after}");
            assert_eq!(matrix.sizes, sizes, "stop_after={stop_after}");
            if matches!(outcome, super::super::TraceOutcome::Completed(_)) {
                return;
            }
        }
        panic!("empty-board search should complete within the event budget");
    }

    #[test]
    fn cover_then_uncover_restores_the_matrix_for_each_constraint() {
        let mut matrix = Matrix::new(&Board::default());
        let nodes = matrix.nodes.clone();
        let sizes = matrix.sizes;
        for column in 1..=COLUMNS {
            matrix.cover(column);
            matrix.uncover(column);
            assert_eq!(matrix.nodes, nodes);
            assert_eq!(matrix.sizes, sizes);
        }
    }

    #[test]
    fn covering_all_four_constraints_restores_every_possible_assignment() {
        let mut matrix = Matrix::new(&Board::default());
        let nodes = matrix.nodes.clone();
        let sizes = matrix.sizes;
        for assignment in 0..729 {
            let cell = assignment / 9;
            let digit = assignment % 9;
            let constraints = [
                cell + 1,
                81 + cell / 9 * 9 + digit + 1,
                162 + cell % 9 * 9 + digit + 1,
                243 + box_index(cell) * 9 + digit + 1,
            ];
            for column in constraints {
                matrix.cover(column);
            }
            for column in constraints.into_iter().rev() {
                matrix.uncover(column);
            }
            assert_eq!(matrix.nodes, nodes, "assignment={assignment}");
            assert_eq!(matrix.sizes, sizes, "assignment={assignment}");
        }
    }

    #[test]
    fn deferred_empty_constraints_restore_every_link_on_unsatisfiable_searches() {
        let mut immediate = [0; 81];
        immediate[..8].copy_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8]);
        immediate[17] = 9;
        let mut deeper: [u8; 81] =
            "530070000600195000098000060800060003400803001700020006060000280000419005000080079"
                .parse::<Board>()
                .unwrap()
                .into();
        deeper[2] = 1;
        for (cells, empty_column) in [(immediate, 9), (deeper, 226)] {
            let puzzle = Board::try_from(cells).unwrap();
            let mut matrix = Matrix::new(&puzzle);
            let nodes = matrix.nodes.clone();
            let sizes = matrix.sizes;
            // Column one represents a given and is encountered before the
            // contradiction. The early exit must defer that later empty
            // constraint rather than rejecting the root without descending.
            assert_eq!(matrix.smallest_column(), Some(1));
            assert_eq!(matrix.sizes[1], 1);
            assert_eq!(matrix.sizes[empty_column], 0);
            if empty_column == 226 {
                // This puzzle has no empty cell domain at initialization;
                // its contradiction lies in a column/digit constraint.
                assert!(matrix.sizes[1..=81].iter().all(|&size| size > 0));
            }
            for cap in [1, 2, 3] {
                let mut collector = Collector {
                    result: super::super::SearchState::empty(),
                    limit: NonZeroUsize::new(cap).unwrap(),
                    observer: super::super::NoObserver,
                };
                matrix.visit(&mut [0; 81], puzzle.cells(), &mut collector, 0);
                assert_eq!(collector.result.solutions_found, 0, "cap={cap}");
                assert_eq!(collector.result.solution, None);
                assert!(!collector.result.limit_reached());
                assert!(collector.result.stats.nodes > 1);
                assert!(collector.result.stats.forced_moves > 0);
                assert!(collector.result.stats.backtracks > 0);
                assert!(collector.result.stats.max_depth > 0);
                assert_eq!(matrix.nodes, nodes, "cap={cap} {puzzle}");
                assert_eq!(matrix.sizes, sizes, "cap={cap} {puzzle}");
            }
        }
    }

    #[test]
    fn capped_search_restores_every_link_and_column_size() {
        let puzzle = Board::default();
        let mut matrix = Matrix::new(&Board::default());
        let nodes = matrix.nodes.clone();
        let sizes = matrix.sizes;
        for cap in [1, 2, 3] {
            let mut collector = Collector {
                result: super::super::SearchState::empty(),
                limit: NonZeroUsize::new(cap).unwrap(),
                observer: super::super::NoObserver,
            };
            matrix.visit(&mut [0; 81], puzzle.cells(), &mut collector, 0);
            assert_eq!(collector.result.solutions_found, cap);
            assert!(collector.result.limit_reached());
            assert_eq!(matrix.nodes, nodes, "cap={cap}");
            assert_eq!(matrix.sizes, sizes, "cap={cap}");
        }
    }
}
