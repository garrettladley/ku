//! Search performance on complete, sparse, ambiguous, and unsatisfiable boards.

use std::{hint::black_box, num::NonZeroUsize, time::Duration};

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use ku_core::Board;
use ku_solvers::built_in_solvers;

fn edge_benchmarks(criterion: &mut Criterion) {
    let solution: Board =
        "534678912672195348198342567859761423426853791713924856961537284287419635345286179"
            .parse()
            .unwrap();
    let mut ambiguous = *solution.cells();
    for cell in [3, 4, 30, 31] {
        ambiguous[cell] = 0;
    }
    let mut dense = *solution.cells();
    dense[..8].fill(0);
    let mut immediate = [0; 81];
    immediate[..8].copy_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8]);
    immediate[17] = 9;
    let mut deeper: [u8; 81] =
        "530070000600195000098000060800060003400803001700020006060000280000419005000080079"
            .parse::<Board>()
            .unwrap()
            .into();
    deeper[2] = 1;
    let cap = NonZeroUsize::new(2).unwrap();
    for (name, puzzle, expected_count) in [
        ("complete", solution, 1),
        ("empty", Board::default(), 2),
        ("ambiguous", Board::try_from(ambiguous).unwrap(), 2),
        ("dense", Board::try_from(dense).unwrap(), 1),
        ("immediate-unsat", Board::try_from(immediate).unwrap(), 0),
        ("deeper-unsat", Board::try_from(deeper).unwrap(), 0),
    ] {
        let mut group = criterion.benchmark_group(format!("edge/{name}"));
        group.throughput(Throughput::Elements(1));
        for &solver in built_in_solvers() {
            let result = solver.search(&puzzle, cap);
            assert_eq!(result.solutions_found(), expected_count);
            assert_eq!(result.limit_reached(), expected_count == 2);
            if let Some(solved) = result.solution() {
                assert!(
                    Board::try_from(*solved.cells())
                        .unwrap()
                        .is_solution_for(&puzzle)
                );
            }
            group.bench_function(BenchmarkId::from_parameter(solver.name()), |bencher| {
                bencher.iter(|| black_box(solver.search(black_box(&puzzle), cap)));
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
        targets = edge_benchmarks
    }
}
criterion_main!(harness::benches);
