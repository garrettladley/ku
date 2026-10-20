//! Built-in strategy registration and capability discovery.
use crate::{
    Backtracking, Bitmask, BitsetCover, ConstraintPropagation, DancingLinks, ForwardChecking,
};
use ku_core::{Solver, TraceSolver};

/// An explicitly registered strategy and its optional tracing capability.
#[derive(Clone, Copy)]
pub struct SolverEntry {
    solver: &'static dyn Solver,
    tracer: Option<&'static dyn TraceSolver>,
}
impl SolverEntry {
    /// Registers a strategy without tracing support.
    pub const fn new(solver: &'static dyn Solver) -> Self {
        Self {
            solver,
            tracer: None,
        }
    }
    /// Registers a strategy with tracing support.
    ///
    /// # Examples
    ///
    /// ```
    /// use ku_solvers::{Bitmask, SolverEntry};
    ///
    /// let searchable = SolverEntry::new(&Bitmask);
    /// assert_eq!(searchable.name(), "bitmask");
    /// assert!(searchable.tracer().is_none());
    /// let traceable = SolverEntry::traced(&Bitmask);
    /// assert!(traceable.tracer().is_some());
    /// ```
    pub const fn traced<T: TraceSolver>(solver: &'static T) -> Self {
        Self {
            solver,
            tracer: Some(solver),
        }
    }
    /// Borrows its search capability.
    pub fn solver(self) -> &'static dyn Solver {
        self.solver
    }
    /// Borrows its tracing capability when supported.
    pub fn tracer(self) -> Option<&'static dyn TraceSolver> {
        self.tracer
    }
    /// Returns the strategy's stable name.
    pub fn name(self) -> &'static str {
        self.solver.name()
    }
}
impl std::fmt::Debug for SolverEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SolverEntry")
            .field("name", &self.name())
            .field("tracing", &self.tracer.is_some())
            .finish()
    }
}
macro_rules! registry {
    ($($solver:ident),+ $(,)?) => {
        /// Built-in strategies and their capabilities, shared by every consumer.
        pub fn solver_registry() -> &'static [SolverEntry] { static ENTRIES: &[SolverEntry] = &[$(SolverEntry::traced(&$solver)),+]; ENTRIES }
        /// Built-in strategies, in stable registry order.
        pub fn built_in_solvers() -> &'static [&'static dyn Solver] { &[$(&$solver),+] }
        /// Built-in tracing strategies, in stable registry order.
        pub fn built_in_trace_solvers() -> &'static [&'static dyn TraceSolver] { &[$(&$solver),+] }
    };
}
registry!(
    Backtracking,
    Bitmask,
    DancingLinks,
    ForwardChecking,
    ConstraintPropagation,
    BitsetCover
);
