//! Complement Criterion with per-puzzle latency, search work, and allocations.

use std::{error::Error, fs, hint::black_box, num::NonZeroUsize, time::Instant};

use ku_core::dataset;
use ku_solvers::built_in_solvers;

fn main() -> Result<(), Box<dyn Error>> {
    let mut args = std::env::args().skip(1);
    let path = args.next().ok_or("usage: compare DATASET [REPEATS]")?;
    let repeats: NonZeroUsize = args.next().as_deref().unwrap_or("100").parse()?;
    if args.next().is_some() {
        return Err("usage: compare DATASET [REPEATS]".into());
    }
    let records = dataset::parse(&fs::read_to_string(path)?)?;
    if records.is_empty() {
        return Err("dataset contains no puzzles".into());
    }
    // Validate outside the timed section and use identical first-solution semantics.
    for &solver in built_in_solvers() {
        for record in &records {
            let result = solver.search(record.puzzle(), NonZeroUsize::MIN);
            let solution = result
                .into_solution()
                .ok_or("unsatisfiable benchmark puzzle")?;
            if !solution.is_solution_for(record.puzzle())
                || record
                    .solution()
                    .is_some_and(|expected| *expected != solution)
            {
                return Err("incorrect benchmark solution".into());
            }
        }
    }
    println!(
        "solver,puzzles_per_second,p50_us,p95_us,p99_us,nodes_per_puzzle,allocations_per_puzzle,allocated_bytes_per_puzzle,peak_heap_bytes"
    );
    for &solver in built_in_solvers() {
        let mut times = Vec::with_capacity(records.len() * repeats.get());
        let mut nodes = 0_u64;
        for _ in 0..repeats.get() {
            for record in &records {
                let start = Instant::now();
                let result =
                    black_box(solver.search(black_box(record.puzzle()), NonZeroUsize::MIN));
                let elapsed = start.elapsed();
                nodes += result.stats().nodes;
                times.push(elapsed.as_secs_f64());
            }
        }
        let elapsed: f64 = times.iter().sum();
        times.sort_unstable_by(f64::total_cmp);
        let quantile =
            |percent: usize| times[(times.len() * percent).div_ceil(100).saturating_sub(1)] * 1e6;
        // Allocation accounting is a separate pass so measurement does not affect timings.
        let mut allocations = 0;
        let mut bytes = 0;
        let mut peak = 0;
        for record in &records {
            let info = allocation_counter::measure(|| {
                let _ = black_box(solver.search(black_box(record.puzzle()), NonZeroUsize::MIN));
            });
            allocations += info.count_total;
            bytes += info.bytes_total;
            peak = peak.max(info.bytes_max);
        }
        let count = times.len() as f64;
        let puzzles = records.len() as f64;
        println!(
            "{},{:.1},{:.3},{:.3},{:.3},{:.2},{:.2},{:.1},{peak}",
            solver.name(),
            count / elapsed,
            quantile(50),
            quantile(95),
            quantile(99),
            nodes as f64 / count,
            allocations as f64 / puzzles,
            bytes as f64 / puzzles
        );
    }
    Ok(())
}
