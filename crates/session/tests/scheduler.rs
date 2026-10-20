//! Virtual deadlines, ordered feedback, cancellation, and scheduling failures.

use std::{cell::Cell, num::NonZeroUsize};

use ku_core::{Board, CellIndex, SearchExtent, SearchResult, TraceSnapshot};
use ku_session::{
    action::Action,
    protocol::{
        Activation, ActivationId, Command, CommandId, CommandKind, Job, LogicalTime, ProtocolError,
        RunId,
    },
    simulation::{FunctionExecutor, ScheduledJob, SimulationError, VirtualScheduler},
};

fn time(milliseconds: u64) -> LogicalTime {
    LogicalTime::from_millis(milliseconds)
}

fn command(id: u64, kind: CommandKind) -> Command {
    Command {
        id: CommandId::try_from(id).unwrap(),
        kind,
    }
}

fn start(id: u64, run: RunId) -> Command {
    command(
        id,
        CommandKind::StartSolver {
            run,
            solver: "scripted".parse().unwrap(),
            puzzle: Board::default(),
        },
    )
}

fn completed(run: RunId, command: CommandId) -> Job {
    let solution: Board =
        "534678912672195348198342567859761423426853791713924856961537284287419635345286179"
            .parse()
            .unwrap();
    let stats = Default::default();
    let result = SearchResult::new(
        &Board::default(),
        Some(solution.clone()),
        1,
        SearchExtent::Capped(NonZeroUsize::MIN),
        stats,
    )
    .unwrap();
    Job::SolverCompleted {
        run,
        command,
        result,
        snapshot: TraceSnapshot::new(solution, None, stats, 0, CellIndex::FIRST),
    }
}

#[test]
fn timers_skip_to_deadlines_and_equal_deadlines_preserve_command_order() {
    let mut scheduler = VirtualScheduler::new();
    let called = Cell::new(false);
    let mut executor = FunctionExecutor::new(|_: &Command| {
        called.set(true);
        Ok(Vec::new())
    });
    for (id, deadline) in [(1, 1_000_000), (2, 1_000_000), (3, 50)] {
        executor
            .execute(
                &mut scheduler,
                &command(
                    id,
                    CommandKind::ScheduleTimer {
                        deadline: time(deadline),
                    },
                ),
            )
            .unwrap();
    }
    assert!(!called.get(), "timers must not call the solver capability");
    assert_eq!(scheduler.now(), LogicalTime::ZERO);
    let first = scheduler.next_activation_auto().unwrap().unwrap();
    assert_eq!(first.time(), time(50));
    assert_eq!(
        first.jobs(),
        [Job::TimerFired {
            command: CommandId::try_from(3).unwrap()
        }]
    );
    let second = scheduler.next_activation_auto().unwrap().unwrap();
    assert_eq!(second.time(), time(1_000_000));
    assert_eq!(
        second.jobs(),
        [
            Job::TimerFired {
                command: CommandId::FIRST
            },
            Job::TimerFired {
                command: CommandId::try_from(2).unwrap()
            }
        ]
    );
    assert_eq!(second.id(), first.id().checked_next().unwrap());
    assert!(scheduler.next_activation_auto().unwrap().is_none());
    assert_eq!(scheduler.now(), time(1_000_000));
}

#[test]
fn jobs_at_the_current_time_form_a_new_batch_without_advancing_time() {
    let mut scheduler = VirtualScheduler::default();
    scheduler
        .schedule_at(time(30), Action::Help.into())
        .unwrap();
    assert_eq!(
        scheduler.next_jobs(),
        Some((time(30), vec![Action::Help.into()]))
    );
    scheduler
        .schedule_at(time(30), Action::Quit.into())
        .unwrap();
    assert_eq!(
        scheduler.next_jobs(),
        Some((time(30), vec![Action::Quit.into()]))
    );
    assert_eq!(scheduler.next_jobs(), None);
    assert_eq!(scheduler.now(), time(30));
}

#[test]
fn cleanup_discards_pending_jobs_without_resetting_time_or_identities() {
    let mut scheduler = VirtualScheduler::new();
    let timer = command(1, CommandKind::ScheduleTimer { deadline: time(30) });
    scheduler.execute(&timer).unwrap();
    scheduler.next_activation_auto().unwrap().unwrap();
    scheduler
        .schedule_at(time(40), Action::Help.into())
        .unwrap();
    scheduler.clear();
    scheduler.clear();
    assert!(scheduler.is_empty());
    assert_eq!(scheduler.now(), time(30));
    assert_eq!(
        scheduler.execute(&timer),
        Err(SimulationError::Protocol(ProtocolError::CommandOrder))
    );
    scheduler
        .schedule_at(time(30), Action::Quit.into())
        .unwrap();
    assert_eq!(
        scheduler
            .next_activation_auto()
            .unwrap()
            .unwrap()
            .id()
            .get(),
        2
    );
}

#[test]
fn cancellation_removes_ordinary_run_responses_and_acknowledges_automatically() {
    let mut scheduler = VirtualScheduler::new();
    let run = RunId::FIRST;
    let start = start(1, run);
    let mut called = Vec::new();
    scheduler
        .execute_with(&start, &mut |command| {
            called.push(command.id);
            Ok(vec![
                ScheduledJob::new(
                    time(20),
                    Job::SolverStarted {
                        run,
                        command: command.id,
                    },
                ),
                ScheduledJob::new(time(100), completed(run, command.id)),
            ])
        })
        .unwrap();
    let cancel = command(2, CommandKind::CancelSolver { run });
    scheduler
        .execute_with(&cancel, &mut |command| {
            called.push(command.id);
            Ok(Vec::new())
        })
        .unwrap();
    assert_eq!(called, [start.id, cancel.id]);
    assert_eq!(scheduler.len(), 1);
    assert_eq!(
        scheduler.next_jobs(),
        Some((
            LogicalTime::ZERO,
            vec![Job::SolverCancelled {
                run,
                command: cancel.id
            }]
        ))
    );
    assert!(scheduler.is_empty());
}

#[test]
fn injected_success_and_cancellation_races_can_choose_both_orders() {
    for cancellation_first in [false, true] {
        let mut scheduler = VirtualScheduler::new();
        let run = RunId::FIRST;
        let start = start(1, run);
        scheduler
            .execute_with(&start, &mut |command| {
                Ok(vec![
                    ScheduledJob::new(
                        time(10),
                        Job::SolverStarted {
                            run,
                            command: command.id,
                        },
                    ),
                    ScheduledJob::new(time(100), completed(run, command.id)),
                ])
            })
            .unwrap();
        assert_eq!(
            scheduler.next_activation_auto().unwrap().unwrap().time(),
            time(10)
        );
        let cancel = command(2, CommandKind::CancelSolver { run });
        let mut acknowledge = |command: &Command| {
            Ok(vec![ScheduledJob::new(
                time(100),
                Job::SolverCancelled {
                    run,
                    command: command.id,
                },
            )])
        };
        if cancellation_first {
            scheduler.execute_with(&cancel, &mut acknowledge).unwrap();
            scheduler
                .schedule_at(time(100), completed(run, start.id))
                .unwrap();
        } else {
            scheduler
                .schedule_at(time(100), completed(run, start.id))
                .unwrap();
            scheduler.execute_with(&cancel, &mut acknowledge).unwrap();
        }
        let activation = scheduler.next_activation_auto().unwrap().unwrap();
        let success = completed(run, start.id);
        let cancelled = Job::SolverCancelled {
            run,
            command: cancel.id,
        };
        let expected = if cancellation_first {
            vec![cancelled, success]
        } else {
            vec![success, cancelled]
        };
        assert_eq!(activation.time(), time(100));
        assert_eq!(activation.jobs(), expected);
        assert!(
            scheduler.is_empty(),
            "ordinary completion should have been removed"
        );
    }
}

#[test]
fn cancellation_keeps_other_runs_timers_and_explicit_stale_jobs() {
    let mut scheduler = VirtualScheduler::new();
    let first = RunId::FIRST;
    let second = first.checked_next().unwrap();
    for (id, run) in [(1, first), (2, second)] {
        scheduler
            .execute_with(&start(id, run), &mut |command| {
                Ok(vec![ScheduledJob::new(
                    time(50),
                    Job::SolverStarted {
                        run,
                        command: command.id,
                    },
                )])
            })
            .unwrap();
    }
    let timer = command(3, CommandKind::ScheduleTimer { deadline: time(50) });
    scheduler.execute(&timer).unwrap();
    let stale = Job::SolverStarted {
        run: first,
        command: CommandId::FIRST,
    };
    scheduler.schedule_at(time(50), stale.clone()).unwrap();
    let cancel = command(4, CommandKind::CancelSolver { run: first });
    scheduler.execute(&cancel).unwrap();
    assert_eq!(
        scheduler.next_jobs(),
        Some((
            LogicalTime::ZERO,
            vec![Job::SolverCancelled {
                run: first,
                command: cancel.id
            }]
        ))
    );
    assert_eq!(
        scheduler.next_jobs(),
        Some((
            time(50),
            vec![
                Job::SolverStarted {
                    run: second,
                    command: CommandId::try_from(2).unwrap()
                },
                Job::TimerFired { command: timer.id },
                stale,
            ]
        ))
    );
}

#[test]
fn backward_times_and_invalid_callback_batches_are_rejected_without_mutation() {
    let mut scheduler = VirtualScheduler::new();
    scheduler
        .schedule_at(time(20), Action::Help.into())
        .unwrap();
    scheduler.next_jobs().unwrap();
    let expected = SimulationError::BackwardTime {
        now: time(20),
        scheduled: time(19),
    };
    assert_eq!(
        scheduler.schedule_at(time(19), Action::Quit.into()),
        Err(expected.clone())
    );
    assert_eq!(
        scheduler.execute(&command(
            1,
            CommandKind::ScheduleTimer { deadline: time(19) }
        )),
        Err(expected)
    );
    let start = start(1, RunId::FIRST);
    let result = scheduler.execute_with(&start, &mut |command| {
        Ok(vec![
            ScheduledJob::new(
                time(21),
                Job::SolverStarted {
                    run: RunId::FIRST,
                    command: command.id,
                },
            ),
            ScheduledJob::new(
                time(22),
                Job::SolverStarted {
                    run: RunId::FIRST.checked_next().unwrap(),
                    command: command.id,
                },
            ),
        ])
    });
    assert_eq!(
        result,
        Err(SimulationError::ResponseMismatch { command: start.id })
    );
    assert!(scheduler.is_empty());
    assert_eq!(scheduler.now(), time(20));
    scheduler
        .execute_with(&start, &mut |command| {
            Ok(vec![ScheduledJob::new(
                time(21),
                Job::SolverStarted {
                    run: RunId::FIRST,
                    command: command.id,
                },
            )])
        })
        .unwrap();
    assert_eq!(
        scheduler.len(),
        1,
        "failed execution must not consume the command identity"
    );
}

#[test]
fn callback_failures_do_not_cancel_or_consume_a_command() {
    let mut scheduler = VirtualScheduler::new();
    let run = RunId::FIRST;
    scheduler
        .execute_with(&start(1, run), &mut |command| {
            Ok(vec![ScheduledJob::new(
                time(50),
                Job::SolverStarted {
                    run,
                    command: command.id,
                },
            )])
        })
        .unwrap();
    let cancel = command(2, CommandKind::CancelSolver { run });
    assert_eq!(
        scheduler.execute_with(&cancel, &mut |_| Err(SimulationError::Executor(
            "injected".into()
        ))),
        Err(SimulationError::Executor("injected".into()))
    );
    assert_eq!(scheduler.len(), 1);
    scheduler.execute(&cancel).unwrap();
    assert_eq!(
        scheduler.len(),
        1,
        "only cancellation acknowledgement should remain"
    );
    assert!(matches!(
        scheduler.next_jobs().unwrap().1.as_slice(),
        [Job::SolverCancelled { .. }]
    ));
}

#[test]
fn command_and_activation_identities_are_monotonic_and_rejection_preserves_jobs() {
    let mut scheduler = VirtualScheduler::new();
    let timer = command(3, CommandKind::ScheduleTimer { deadline: time(10) });
    scheduler.execute(&timer).unwrap();
    assert_eq!(
        scheduler.execute(&timer),
        Err(SimulationError::Protocol(ProtocolError::CommandOrder))
    );
    assert_eq!(
        scheduler.execute(&command(
            2,
            CommandKind::ScheduleTimer { deadline: time(20) }
        )),
        Err(SimulationError::Protocol(ProtocolError::CommandOrder))
    );
    scheduler
        .next_activation(ActivationId::try_from(4).unwrap())
        .unwrap()
        .unwrap();
    scheduler
        .schedule_at(time(20), Action::Help.into())
        .unwrap();
    assert_eq!(
        scheduler.next_activation(ActivationId::FIRST),
        Err(SimulationError::Protocol(ProtocolError::ActivationOrder))
    );
    assert_eq!(scheduler.len(), 1);
    assert_eq!(scheduler.now(), time(10));
    assert_eq!(
        scheduler
            .next_activation_auto()
            .unwrap()
            .unwrap()
            .id()
            .get(),
        5
    );
}

#[test]
fn empty_activations_and_exhausted_time_or_identities_are_explicit_errors() {
    assert_eq!(
        Activation::new(ActivationId::FIRST, LogicalTime::ZERO, Vec::new()),
        Err(ProtocolError::EmptyActivation)
    );
    assert_eq!(time(u64::MAX).checked_add(1), Err(ProtocolError::Exhausted));
    let mut scheduler = VirtualScheduler::new();
    assert!(scheduler.next_activation_auto().unwrap().is_none());
    scheduler
        .schedule_at(time(u64::MAX), Action::Help.into())
        .unwrap();
    scheduler
        .next_activation(ActivationId::try_from(u64::MAX).unwrap())
        .unwrap();
    scheduler
        .schedule_at(time(u64::MAX), Action::Quit.into())
        .unwrap();
    assert_eq!(
        scheduler.next_activation_auto(),
        Err(SimulationError::Protocol(ProtocolError::Exhausted))
    );
    assert_eq!(scheduler.len(), 1);
    assert_eq!(scheduler.now(), time(u64::MAX));
}
