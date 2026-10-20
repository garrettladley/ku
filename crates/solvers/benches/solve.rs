//! Solver searches and checked result conversion, with setup outside timing loops.

#[path = "../tests/support/mod.rs"]
mod support;

use std::{hint::black_box, num::NonZeroUsize, time::Duration};

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use ku_core::{Board, SearchExtent, SearchResult, SearchStats};
use ku_solvers::built_in_solvers;

fn result_boundary_benchmarks(criterion: &mut Criterion) {
    let solution: Board =
        "534678912672195348198342567859761423426853791713924856961537284287419635345286179"
            .parse()
            .unwrap();
    let partial = support::groups()[0].1[0].puzzle();
    let empty = Board::default();
    let mut group = criterion.benchmark_group("boundary/checked-result");
    group.throughput(Throughput::Elements(1));
    for (name, puzzle, cells, count) in [
        ("complete", &solution, Some(*solution.cells()), 1),
        // Use the matching generated solution to include preservation of givens.
        (
            "partial",
            partial,
            Some(*support::groups()[0].1[0].solution().unwrap().cells()),
            1,
        ),
        ("empty", &empty, Some(*solution.cells()), 1),
        ("no-solution", &empty, None, 0),
    ] {
        assert!(
            SearchResult::try_from_cells(
                puzzle,
                cells,
                count,
                SearchExtent::Exhausted,
                SearchStats::default(),
            )
            .is_ok()
        );
        group.bench_function(name, |bencher| {
            bencher.iter(|| {
                black_box(SearchResult::try_from_cells(
                    black_box(puzzle),
                    black_box(cells),
                    black_box(count),
                    black_box(SearchExtent::Exhausted),
                    black_box(SearchStats::default()),
                ))
            });
        });
    }
    group.finish();
}

fn solve_benchmarks(criterion: &mut Criterion) {
    let seed_offset = std::env::var("KU_BENCH_SEED_OFFSET")
        .map(|value| value.parse().expect("KU_BENCH_SEED_OFFSET must be a u64"))
        .unwrap_or(0);
    let count = std::env::var("KU_BENCH_PUZZLES")
        .map(|value| {
            value
                .parse()
                .expect("KU_BENCH_PUZZLES must be a positive usize")
        })
        .unwrap_or(32);
    assert!(count > 0, "KU_BENCH_PUZZLES must be positive");
    let groups = support::generated_groups(seed_offset, count);
    for (min_clues, records) in &groups {
        for (mode, cap) in [
            ("solve", NonZeroUsize::MIN),
            ("unique", NonZeroUsize::new(2).unwrap()),
        ] {
            let mut group = criterion.benchmark_group(format!("{mode}/min-clues-{min_clues}"));
            group.throughput(Throughput::Elements(records.len() as u64));
            for &solver in built_in_solvers() {
                // Catch broken implementations before collecting misleading timings.
                for record in records {
                    let result = solver.search(record.puzzle(), cap);
                    assert_eq!(result.solutions_found(), 1);
                    assert_eq!(result.solution(), record.solution());
                    assert_eq!(result.limit_reached(), cap == NonZeroUsize::MIN);
                }
                group.bench_function(BenchmarkId::from_parameter(solver.name()), |bencher| {
                    bencher.iter(|| {
                        for record in records {
                            let _ = black_box(solver.search(black_box(record.puzzle()), cap));
                        }
                    });
                });
            }
            group.finish();
        }
    }
}

mod harness {
    use super::*;

    criterion_group! {
        name = benches;
        config = Criterion::default().sample_size(30).warm_up_time(Duration::from_millis(300)).measurement_time(Duration::from_secs(2));
        targets = solve_benchmarks, result_boundary_benchmarks
    }
}
criterion_main!(harness::benches);
