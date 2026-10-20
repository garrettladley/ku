//! Locally generated workloads shared by solver tests and benchmarks.

use std::sync::OnceLock;

use ku_core::Record;
use ku_solvers::{MinClues, generate};

/// Fixed seeds and clue floors, deliberately not human difficulty ratings.
/// Generation happens once per process, outside benchmark timing loops.
pub fn groups() -> &'static [(usize, Vec<Record>)] {
    static GROUPS: OnceLock<[(usize, Vec<Record>); 4]> = OnceLock::new();
    GROUPS.get_or_init(|| generated_groups(0, 32))
}

/// Vary seeds and sample size without changing the timed solver workload.
pub fn generated_groups(seed_offset: u64, count: usize) -> [(usize, Vec<Record>); 4] {
    [48, 36, 28, 22].map(|min_clues| {
        let floor = MinClues::try_from(min_clues).expect("valid workload clue floor");
        let records = (0..count)
            .map(|offset| {
                let seed = seed_offset + min_clues as u64 * 1000 + offset as u64;
                let (puzzle, solution) = generate(seed, floor)
                    .expect("valid generation parameters")
                    .into_parts();
                Record::new(puzzle, Some(solution)).expect("generated solution preserves givens")
            })
            .collect();
        (min_clues, records)
    })
}
