//! Serial and reused-pool parallel throughput for independent puzzle batches.

#[path = "../tests/support/mod.rs"]
mod support;

use std::{hint::black_box, time::Duration};

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use ku_core::{Board, Solver};
use ku_solvers::Bitmask;
use rayon::{ThreadPoolBuilder, prelude::*};

fn batch_benchmarks(criterion: &mut Criterion) {
    let records: Vec<_> = support::groups()
        .iter()
        .flat_map(|(_, records)| records)
        .collect();
    // A long-lived pool matches batch processing in an application. Its startup
    // is deliberately excluded; collect output in both paths for equal work.
    let pools: Vec<_> = [2, 4, 8]
        .into_iter()
        .map(|threads| {
            (
                threads,
                ThreadPoolBuilder::new()
                    .num_threads(threads)
                    .build()
                    .expect("worker pool"),
            )
        })
        .collect();
    let mut contradictory = [0; 81];
    contradictory[..8].copy_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8]);
    contradictory[17] = 9;
    let check = [
        records[0].puzzle().clone(),
        Board::try_from(contradictory).unwrap(),
        records[1].puzzle().clone(),
    ];
    let expected: Vec<_> = check.iter().map(|puzzle| Bitmask.solve(puzzle)).collect();
    assert!(expected[1].is_none());
    for (_, pool) in &pools {
        let actual: Vec<_> = pool.install(|| {
            check
                .par_iter()
                .map(|puzzle| Bitmask.solve(puzzle))
                .collect()
        });
        assert_eq!(actual, expected);
    }
    for size in [1, 32, 128, 1024] {
        let puzzles: Vec<_> = records
            .iter()
            .cycle()
            .take(size)
            .map(|record| record.puzzle().clone())
            .collect();
        let expected: Vec<_> = puzzles.iter().map(|puzzle| Bitmask.solve(puzzle)).collect();
        let mut group = criterion.benchmark_group(format!("batch/{size}"));
        group.throughput(Throughput::Elements(size as u64));
        group.bench_function("serial", |bencher| {
            bencher.iter(|| {
                black_box(
                    black_box(&puzzles)
                        .iter()
                        .map(|puzzle| Bitmask.solve(puzzle))
                        .collect::<Vec<Option<Board>>>(),
                )
            });
        });
        for (threads, pool) in &pools {
            let actual: Vec<_> = pool.install(|| {
                puzzles
                    .par_iter()
                    .map(|puzzle| Bitmask.solve(puzzle))
                    .collect()
            });
            assert_eq!(actual, expected);
            group.bench_function(BenchmarkId::new("rayon", threads), |bencher| {
                bencher.iter(|| {
                    pool.install(|| {
                        black_box(
                            black_box(&puzzles)
                                .par_iter()
                                .map(|puzzle| Bitmask.solve(puzzle))
                                .collect::<Vec<Option<Board>>>(),
                        )
                    })
                });
            });
        }
        group.finish();
    }
}

mod harness {
    use super::*;

    criterion_group! {
        name = benches;
        config = Criterion::default().sample_size(30).warm_up_time(Duration::from_millis(300)).measurement_time(Duration::from_secs(2));
        targets = batch_benchmarks
    }
}
criterion_main!(harness::benches);
