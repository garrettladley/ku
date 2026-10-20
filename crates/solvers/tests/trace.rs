//! Actual solver traces, deterministic navigation, and cancellation boundaries.

mod support;

use std::{num::NonZeroUsize, ops::ControlFlow};

use ku_core::{
    Board, CandidateMask, CellIndex, Digit, HistoryError, HistoryLimits, SearchResult, SearchStats,
    Solver, TraceEvent, TraceHistory, TraceKind, TraceOutcome, TraceReplay, TraceSnapshot,
    TraceSolver,
};
use ku_solvers::built_in_trace_solvers;

const PUZZLE: &str =
    "530070000600195000098000060800060003400803001700020006060000280000419005000080079";
const SOLUTION: &str =
    "534678912672195348198342567859761423426853791713924856961537284287419635345286179";

fn traced(solver: &dyn TraceSolver, puzzle: &Board) -> (Vec<TraceEvent>, SearchResult) {
    let mut events = Vec::new();
    let outcome = solver.trace_search(puzzle, NonZeroUsize::new(2).unwrap(), &mut |event| {
        events.push(event);
        ControlFlow::Continue(())
    });
    let TraceOutcome::Completed(result) = outcome else {
        panic!("collecting cannot cancel")
    };
    (events, result)
}

#[test]
fn traced_search_preserves_results_statistics_and_valid_protocol_across_generated_puzzles() {
    for (_, records) in support::groups() {
        for record in records {
            for &solver in built_in_trace_solvers() {
                let (events, actual) = traced(solver, record.puzzle());
                assert_eq!(
                    actual,
                    solver.search(record.puzzle(), NonZeroUsize::new(2).unwrap()),
                    "{} {}",
                    solver.name(),
                    record.puzzle()
                );
                let mut history = TraceHistory::new(
                    record.puzzle().clone(),
                    solver.name(),
                    HistoryLimits::default(),
                );
                let mut assignments = 0;
                for event in events {
                    if let TraceEvent::Step {
                        kind: TraceKind::Assignment { cell, digit, .. },
                        snapshot,
                    } = &event
                    {
                        assert_eq!(snapshot.cells()[cell.get()], digit.get());
                        assert_eq!(snapshot.focus(), *cell);
                        assignments += 1;
                    }
                    history
                        .push(event)
                        .unwrap_or_else(|error| panic!("{}: {error}", solver.name()));
                }
                assert!(assignments > 0);
                assert!(history.is_complete());
                assert_eq!(history.cursor(), 0);
                assert!(history.capacity_bytes() <= history.limits().max_bytes().get());
                history.seek(history.len()).unwrap();
                assert_eq!(history.state().finished(), Some(&actual));
                assert_eq!(history.state().cells(), actual.solution().unwrap().cells());
            }
        }
    }
}

#[test]
fn direct_forward_and_backward_navigation_restore_identical_complete_states() {
    let mut ambiguous = *SOLUTION.parse::<Board>().unwrap().cells();
    for cell in [3, 4, 30, 31] {
        ambiguous[cell] = 0;
    }
    for puzzle in [
        PUZZLE.parse().unwrap(),
        Board::try_from(ambiguous).unwrap(),
        Board::default(),
    ] {
        for &solver in built_in_trace_solvers() {
            let (events, result) = traced(solver, &puzzle);
            assert_eq!(traced(solver, &puzzle), (events.clone(), result));
            let mut history =
                TraceHistory::new(puzzle.clone(), solver.name(), HistoryLimits::default());
            for event in &events {
                history.push(event.clone()).unwrap();
            }
            let mut sequential = TraceReplay::new(&puzzle);
            let mut expected = vec![sequential.state().clone()];
            for event in &events {
                sequential.apply(event);
                expected.push(sequential.state().clone());
            }
            for (cursor, state) in expected.iter().enumerate() {
                history.seek(cursor).unwrap();
                assert_eq!(history.state(), state, "{} cursor={cursor}", solver.name());
            }
            history.seek(0).unwrap();
            for state in &expected[1..] {
                assert!(history.step_forward());
                assert_eq!(history.state(), state);
            }
            assert!(!history.step_forward());
            for state in expected[..events.len()].iter().rev() {
                assert!(history.step_back());
                assert_eq!(history.state(), state);
            }
            assert!(!history.step_back());
        }
    }
}

#[test]
fn cancellation_never_exposes_a_completed_result_or_finish_event() {
    let puzzle: Board = PUZZLE.parse().unwrap();
    for &solver in built_in_trace_solvers() {
        let mut events = Vec::new();
        let outcome = solver.trace_search(&puzzle, NonZeroUsize::MIN, &mut |event| {
            let cancel = matches!(
                event,
                TraceEvent::Step {
                    kind: TraceKind::Assignment { .. },
                    ..
                }
            );
            events.push(event);
            if cancel {
                ControlFlow::Break(())
            } else {
                ControlFlow::Continue(())
            }
        });
        let TraceOutcome::Cancelled(progress) = outcome else {
            panic!("assignment must cancel")
        };
        assert_eq!(progress.solution(), None);
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, TraceEvent::Finished { .. }))
        );
        assert!(matches!(
            events.last(),
            Some(TraceEvent::Step {
                kind: TraceKind::Assignment { .. },
                ..
            })
        ));
        let mut count = 0;
        let outcome = solver.trace_search(&puzzle, NonZeroUsize::MIN, &mut |_| {
            count += 1;
            ControlFlow::Break(())
        });
        let TraceOutcome::Cancelled(progress) = outcome else {
            panic!("start must cancel")
        };
        assert_eq!(count, 1);
        assert_eq!(progress.stats(), SearchStats::default());
    }
}

#[test]
fn cancellation_after_finding_one_solution_still_reports_only_progress() {
    for &solver in built_in_trace_solvers() {
        let outcome = solver.trace_search(
            &Board::default(),
            NonZeroUsize::new(2).unwrap(),
            &mut |event| {
                if matches!(
                    event,
                    TraceEvent::Step {
                        kind: TraceKind::Solution,
                        ..
                    }
                ) {
                    ControlFlow::Break(())
                } else {
                    ControlFlow::Continue(())
                }
            },
        );
        let TraceOutcome::Cancelled(progress) = outcome else {
            panic!("solution sink must cancel")
        };
        assert!(progress.solutions_found() <= 1);
        if let Some(solution) = progress.solution() {
            assert!(solution.is_solved());
        }
    }
}

#[test]
fn every_trace_event_is_a_cancellation_boundary_and_terminal_break_means_completion() {
    // Two completions require branching and restoration, unlike a singles-only puzzle.
    let mut cells = *SOLUTION.parse::<Board>().unwrap().cells();
    for cell in [3, 4, 30, 31] {
        cells[cell] = 0;
    }
    let puzzle = Board::try_from(cells).unwrap();
    for &solver in built_in_trace_solvers() {
        let (expected, result) = traced(solver, &puzzle);
        assert_eq!(result.solutions_found(), 2);
        for stop in 0..expected.len() {
            let mut received = Vec::new();
            let outcome =
                solver.trace_search(&puzzle, NonZeroUsize::new(2).unwrap(), &mut |event| {
                    received.push(event);
                    if received.len() == stop + 1 {
                        ControlFlow::Break(())
                    } else {
                        ControlFlow::Continue(())
                    }
                });
            assert_eq!(received, expected[..=stop], "{} stop={stop}", solver.name());
            if stop == expected.len() - 1 {
                assert_eq!(outcome, TraceOutcome::Completed(result.clone()));
            } else {
                let TraceOutcome::Cancelled(progress) = outcome else {
                    panic!("{} stop={stop}: premature completion", solver.name());
                };
                let found = received
                    .iter()
                    .filter(|event| {
                        matches!(
                            event,
                            TraceEvent::Step {
                                kind: TraceKind::Solution,
                                ..
                            }
                        )
                    })
                    .count();
                assert_eq!(
                    progress.solutions_found(),
                    found,
                    "{} stop={stop}",
                    solver.name()
                );
                assert_eq!(
                    progress.solution(),
                    (found > 0).then(|| result.solution().unwrap())
                );
            }
        }
        // Cancellation must leave the strategy reusable with identical results and work.
        assert_eq!(traced(solver, &puzzle), (expected, result));
        assert_eq!(puzzle.cells(), &cells);
    }
}

#[test]
fn unsatisfiable_finish_is_self_contained_and_exposes_real_contradictions() {
    let mut immediate = [0; 81];
    immediate[..8].copy_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8]);
    immediate[17] = 9;
    let mut deeper = *PUZZLE.parse::<Board>().unwrap().cells();
    deeper[2] = 1;
    for cells in [immediate, deeper] {
        let puzzle = Board::try_from(cells).unwrap();
        for &solver in built_in_trace_solvers() {
            let (events, result) = traced(solver, &puzzle);
            assert!(result.solution().is_none());
            assert!(events.iter().any(|event| matches!(
                event,
                TraceEvent::Step {
                    kind: TraceKind::Contradiction,
                    ..
                }
            )));
            let mut sequential = TraceReplay::new(&puzzle);
            for event in &events {
                sequential.apply(event);
            }
            let mut direct = TraceReplay::new(&puzzle);
            direct.apply(events.last().unwrap());
            assert_eq!(direct, sequential, "{} finish snapshot", solver.name());
            direct.apply(&events[0]);
            assert_eq!(direct, TraceReplay::new(&puzzle));
        }
    }
}

#[test]
fn intermediate_snapshots_report_accumulated_work_and_typed_candidates() {
    let puzzle: Board = PUZZLE.parse().unwrap();
    for &solver in built_in_trace_solvers() {
        let (events, result) = traced(solver, &puzzle);
        let mut counts = Vec::new();
        for event in events {
            if let TraceEvent::Step { snapshot, .. } = event {
                counts.push(snapshot.stats().forced_moves);
                if let Some(masks) = snapshot.candidates() {
                    assert!(masks.iter().all(|mask| mask.bits() & !0x03fe == 0));
                }
            }
        }
        assert!(
            counts
                .iter()
                .any(|&count| count > 0 && count < result.stats().forced_moves),
            "{}",
            solver.name()
        );
        assert!(counts.windows(2).all(|pair| pair[0] <= pair[1]));
    }
}

fn empty_start() -> TraceEvent {
    TraceEvent::Start {
        solver: "test".to_owned(),
        puzzle: Board::default(),
    }
}
fn empty_step(focus: usize, candidates: Option<[CandidateMask; 81]>) -> TraceEvent {
    TraceEvent::Step {
        kind: TraceKind::Node,
        snapshot: TraceSnapshot::new(
            Board::default(),
            candidates,
            SearchStats::default(),
            0,
            CellIndex::try_from(focus).unwrap(),
        ),
    }
}

#[test]
fn history_rejects_protocol_errors_transactionally() {
    let mut history = TraceHistory::new(Board::default(), "test", HistoryLimits::default());
    assert!(matches!(
        history.push(empty_step(0, None)),
        Err(HistoryError::Protocol(_))
    ));
    assert!(history.is_empty());
    assert!(matches!(
        history.push(TraceEvent::Start {
            solver: "other".to_owned(),
            puzzle: Board::default()
        }),
        Err(HistoryError::Protocol(_))
    ));
    history.push(empty_start()).unwrap();
    assert!(matches!(
        history.push(empty_start()),
        Err(HistoryError::Protocol(_))
    ));
    let assignment = TraceEvent::Step {
        kind: TraceKind::Assignment {
            cell: CellIndex::FIRST,
            digit: Digit::try_from(1).unwrap(),
            forced: false,
        },
        snapshot: TraceSnapshot::new(
            Board::default(),
            None,
            SearchStats::default(),
            0,
            CellIndex::FIRST,
        ),
    };
    let state = history.state().clone();
    let capacity = history.capacity_bytes();
    assert!(matches!(
        history.push(assignment),
        Err(HistoryError::Protocol(_))
    ));
    assert_eq!(history.len(), 1);
    assert_eq!(history.state(), &state);
    assert_eq!(history.capacity_bytes(), capacity);
    assert!(matches!(history.seek(2), Err(HistoryError::Cursor { .. })));
    assert_eq!(history.cursor(), 0);
}

#[test]
fn history_preserves_givens_and_rejects_events_after_completion() {
    let puzzle: Board = PUZZLE.parse().unwrap();
    let mut history = TraceHistory::new(puzzle.clone(), "test", HistoryLimits::default());
    history
        .push(TraceEvent::Start {
            solver: "test".to_owned(),
            puzzle,
        })
        .unwrap();
    assert!(matches!(
        history.push(empty_step(0, None)),
        Err(HistoryError::Protocol(_))
    ));
    let solved: Board = SOLUTION.parse().unwrap();
    let result = ku_solvers::Bitmask.search(&solved, NonZeroUsize::new(2).unwrap());
    let mut complete = TraceHistory::new(solved.clone(), "test", HistoryLimits::default());
    complete
        .push(TraceEvent::Start {
            solver: "test".to_owned(),
            puzzle: solved.clone(),
        })
        .unwrap();
    complete
        .push(TraceEvent::Finished {
            snapshot: TraceSnapshot::new(solved, None, result.stats(), 0, CellIndex::FIRST),
            result,
        })
        .unwrap();
    assert!(matches!(
        complete.push(empty_step(0, None)),
        Err(HistoryError::Protocol(_))
    ));
}

#[test]
fn event_and_capacity_limits_are_enforced_before_cursor_or_history_changes() {
    let limits = HistoryLimits::new(NonZeroUsize::MIN, NonZeroUsize::new(1024 * 1024).unwrap());
    let mut history = TraceHistory::new(Board::default(), "test", limits);
    history.push(empty_start()).unwrap();
    assert_eq!(
        history.push(empty_step(0, None)),
        Err(HistoryError::EventLimit)
    );
    assert_eq!(history.len(), 1);
    assert_eq!(history.cursor(), 0);
    let mut tiny = TraceHistory::new(
        Board::default(),
        "test",
        HistoryLimits::new(NonZeroUsize::new(100).unwrap(), NonZeroUsize::MIN),
    );
    assert_eq!(tiny.push(empty_start()), Err(HistoryError::ByteLimit));
    assert_eq!(tiny.capacity_bytes(), 0);
    let limits = HistoryLimits::new(
        NonZeroUsize::new(100).unwrap(),
        NonZeroUsize::new(4096).unwrap(),
    );
    let mut bounded = TraceHistory::new(Board::default(), "test", limits);
    bounded.push(empty_start()).unwrap();
    while bounded.push(empty_step(0, None)).is_ok() {
        assert!(bounded.capacity_bytes() <= 4096);
    }
    assert!(bounded.capacity_bytes() <= 4096);
    assert!(bounded.len() < 100);
}

#[test]
fn candidate_predecessor_metadata_and_focus_survive_bidirectional_seeking() {
    let first = [CandidateMask::try_from(2).unwrap(); 81];
    let second = [CandidateMask::try_from(4).unwrap(); 81];
    let mut history = TraceHistory::new(Board::default(), "test", HistoryLimits::default());
    for event in [
        empty_start(),
        empty_step(1, Some(first)),
        empty_step(2, None),
        empty_step(3, Some(second)),
        empty_step(4, None),
    ] {
        history.push(event).unwrap();
    }
    for cursor in [5, 2, 4, 3, 5, 0] {
        history.seek(cursor).unwrap();
        let expected = match cursor {
            5 => Some(&second),
            3 | 4 => Some(&first),
            _ => None,
        };
        assert_eq!(history.previous_candidates(), expected);
        assert_eq!(history.state().focus().get(), cursor.saturating_sub(1));
    }
}

#[test]
fn history_rejects_decreasing_work_and_inconsistent_terminal_snapshots() {
    let puzzle = Board::default();
    let mut history = TraceHistory::new(puzzle.clone(), "test", HistoryLimits::default());
    history.push(empty_start()).unwrap();
    let stats = SearchStats {
        nodes: 3,
        ..SearchStats::default()
    };
    history
        .push(TraceEvent::Step {
            kind: TraceKind::Node,
            snapshot: TraceSnapshot::new(puzzle.clone(), None, stats, 0, CellIndex::FIRST),
        })
        .unwrap();
    history.seek(2).unwrap();
    let state = history.state().clone();
    let bytes = history.capacity_bytes();
    assert!(matches!(
        history.push(empty_step(0, None)),
        Err(HistoryError::Protocol(_))
    ));
    let result =
        SearchResult::new(&puzzle, None, 0, ku_core::SearchExtent::Exhausted, stats).unwrap();
    let inconsistent = TraceEvent::Finished {
        result,
        snapshot: TraceSnapshot::new(
            puzzle,
            None,
            SearchStats { nodes: 4, ..stats },
            0,
            CellIndex::FIRST,
        ),
    };
    assert!(matches!(
        history.push(inconsistent),
        Err(HistoryError::Protocol(_))
    ));
    assert_eq!(history.len(), 2);
    assert_eq!(history.cursor(), 2);
    assert_eq!(history.state(), &state);
    assert_eq!(history.capacity_bytes(), bytes);
}
