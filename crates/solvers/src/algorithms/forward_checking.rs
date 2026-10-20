use std::num::NonZeroUsize;

use rand::Rng;

use super::{
    Collector, Observer, SearchResult, Solver, collect, collect_traced, domains::Domains,
    shuffled_digits,
};
use crate::{Board, GridGenerator, TraceKind, TraceSink};

/// MRV search with incremental peer elimination and queued naked singles.
#[derive(Clone, Copy, Debug, Default)]
pub struct ForwardChecking;

impl Solver for ForwardChecking {
    fn name(&self) -> &'static str {
        "forward-checking"
    }

    fn search(&self, puzzle: &Board, limit: NonZeroUsize) -> SearchResult {
        collect(limit, |collector| {
            if let Some(state) = Domains::new(puzzle).as_mut() {
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

impl GridGenerator for ForwardChecking {
    fn generate_grid(&self, rng: &mut (impl Rng + ?Sized)) -> Option<Board> {
        let state = Domains::new(&Board::default())?;
        let mut solution = None;
        let _ = collect(NonZeroUsize::MIN, |collector| {
            solution = random_visit(state, rng, collector);
        });
        solution
    }
}

fn random_visit(
    mut state: Domains,
    rng: &mut (impl Rng + ?Sized),
    collector: &mut Collector,
) -> Option<Board> {
    state.singles_observed(collector, 0)?;
    let Some((cell, mask)) = state.select() else {
        return Board::try_from(state.cells).ok();
    };
    for digit in shuffled_digits(mask, rng) {
        let mut child = state.clone();
        if child
            .assign_observed(cell, 1 << digit, false, collector, 0)
            .is_some()
            && let Some(solution) = random_visit(child, rng, collector)
        {
            return Some(solution);
        }
    }
    None
}

impl super::TraceSolver for ForwardChecking {
    fn trace_search(
        &self,
        puzzle: &Board,
        limit: NonZeroUsize,
        sink: &mut dyn TraceSink,
    ) -> super::TraceOutcome {
        collect_traced(limit, self.name(), puzzle, sink, |collector| {
            if let Some(state) = Domains::new(puzzle).as_mut() {
                visit(state, collector, 0);
            } else {
                collector.visit(0);
                collector.result.stats.backtracks += 1;
                collector.observe(|| TraceKind::Contradiction, 0, || (*puzzle.cells(), None));
            }
        })
    }
}

fn visit<O: Observer>(state: &mut Domains, collector: &mut Collector<O>, depth: usize) {
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
    if state.singles_observed(collector, depth).is_none() {
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
        // Borrow the isolated child so recursion does not copy it a second time.
        let mut child = state.clone();
        if child
            .assign_observed(cell, bit, false, collector, depth + 1)
            .is_some()
        {
            visit(&mut child, collector, depth + 1);
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
        super::super::tests::assert_solver_contract(&ForwardChecking);
    }

    #[test]
    fn borrowed_search_keeps_sibling_states_isolated() {
        let solution: Board =
            "534678912672195348198342567859761423426853791713924856961537284287419635345286179"
                .parse()
                .unwrap();
        let mut cells = *solution.cells();
        for cell in [3, 4, 30, 31] {
            cells[cell] = 0;
        }
        let puzzle = Board::try_from(cells).unwrap();
        let mut state = Domains::new(&puzzle).unwrap();
        let original = state.clone();
        let result = collect(NonZeroUsize::new(3).unwrap(), |collector| {
            visit(&mut state, collector, 0);
        });
        assert_eq!(result.solutions_found, 2);
        assert!(!result.limit_reached());
        assert_eq!(state.cells, original.cells);
        assert_eq!(state.masks, original.masks);
        assert_eq!(state.select(), original.select());
    }
}
