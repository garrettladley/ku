//! Application model re-exported from the terminal-independent session crate.
pub use ku_session::{
    app::*,
    protocol::{
        Activation, ActivationId, Command, CommandId, CommandKind, Completion, Failure,
        FailureKind, Job, LogicalTime, RunId, SessionEndReason, SolverId,
    },
};

/// Builds portable metadata for the application's built-in strategy registry.
pub fn solver_catalog() -> SolverCatalog {
    SolverCatalog::try_from(
        ku_solvers::solver_registry()
            .iter()
            .map(|entry| SolverDescriptor {
                id: entry
                    .name()
                    .parse()
                    .expect("built-in solver identities are valid"),
                tracing: entry.tracer().is_some(),
            })
            .collect::<Vec<_>>(),
    )
    .expect("built-in solver identities are unique and nonempty")
}

/// Constructs the ordinary application with its built-in strategy catalog.
///
/// # Errors
/// Rejects an empty bank, an out-of-range zero-based `index`, or an unknown solver.
pub fn new(
    records: Vec<ku_core::Record>,
    index: usize,
    label: String,
    solver: &str,
) -> Result<App, AppError> {
    App::new(records, index, label, solver, solver_catalog())
}
