//! Discovery of independent search and tracing capabilities.

use ku_core::{Board, SearchResult, Solver};
use std::num::NonZeroUsize;

use ku_solvers::{Bitmask, SolverEntry, solver_registry};

#[test]
fn registry_distinguishes_search_only_strategies_from_tracing_capabilities() {
    struct SearchOnly;
    impl Solver for SearchOnly {
        fn name(&self) -> &'static str {
            "search-only"
        }
        fn search(&self, puzzle: &Board, limit: NonZeroUsize) -> SearchResult {
            Bitmask.search(puzzle, limit)
        }
    }
    static SEARCH_ONLY: SearchOnly = SearchOnly;
    let entry = SolverEntry::new(&SEARCH_ONLY);
    assert!(entry.tracer().is_none());
    assert_eq!(entry.name(), "search-only");
    for entry in solver_registry() {
        assert!(entry.tracer().is_some());
        assert_eq!(entry.solver().name(), entry.name());
    }
}
