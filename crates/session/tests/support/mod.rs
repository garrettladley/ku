use ku_core::Record;
use ku_session::app::{App, AppError, SolverCatalog, SolverDescriptor};

pub fn catalog() -> SolverCatalog {
    SolverCatalog::try_from(
        [
            "backtracking",
            "bitmask",
            "dlx",
            "forward-checking",
            "constraint-propagation",
            "bitset-cover",
        ]
        .into_iter()
        .map(|name| SolverDescriptor {
            id: name.parse().unwrap(),
            tracing: true,
        })
        .collect::<Vec<_>>(),
    )
    .unwrap()
}

pub fn app(
    records: Vec<Record>,
    index: usize,
    label: String,
    solver: &str,
) -> Result<App, AppError> {
    App::new(records, index, label, solver, catalog())
}
