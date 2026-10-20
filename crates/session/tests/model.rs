//! Pure application transitions, protocol validation, and independent history views.

mod support;
use std::num::NonZeroUsize;

use ku_core::{
    Board, CellIndex, Digit, HistoryLimits, Record, SearchExtent, SearchResult, SearchStats,
    TraceEvent, TraceKind, TraceSnapshot,
};
use ku_session::{
    action::Action,
    app::{
        App, AppConfig, AppError, BaseOverlay, Mode, OverlayState, Playback, RunState, Severity,
        SolverCatalog, SolverDescriptor, StopReason, ThemeCatalog, ThemeDescriptor,
    },
    protocol::{
        Activation, ActivationId, CommandId, CommandKind, Completion, Failure, FailureKind, Job,
        LogicalTime, ProtocolError, RunId, SessionEndReason,
    },
};

const SOLUTION: &str =
    "534678912672195348198342567859761423426853791713924856961537284287419635345286179";

fn app() -> App {
    let config = AppConfig::new(
        vec![
            Record::from(Board::default()),
            Record::from(SOLUTION.parse::<Board>().unwrap()),
        ],
        0,
        "model".into(),
        "bitmask",
        support::catalog(),
        HistoryLimits::default(),
        Mode::Watch,
        NonZeroUsize::MIN,
        50,
    )
    .unwrap();
    App::from_config(config)
}
fn activate(app: &mut App, time: u64, jobs: Vec<Job>) -> Completion {
    let id = app
        .last_activation()
        .map_or(ActivationId::FIRST, |last| last.checked_next().unwrap());
    app.activate(&Activation::new(id, LogicalTime::from_millis(time), jobs).unwrap())
        .unwrap()
}
fn action(app: &mut App, time: u64, action: Action) -> Completion {
    activate(app, time, vec![action.into()])
}
fn start(app: &mut App) -> (RunId, CommandId, CommandId) {
    let completion = action(app, 0, Action::StepForward);
    assert_eq!(completion.commands().len(), 2);
    let CommandKind::StartSolver { run, .. } = completion.commands()[0].kind else {
        panic!("step starts a producer")
    };
    let start = completion.commands()[0].id;
    let timer = completion.commands()[1].id;
    assert!(matches!(
        completion.commands()[1].kind,
        CommandKind::ScheduleTimer { .. }
    ));
    assert!(
        activate(
            app,
            0,
            vec![Job::SolverStarted {
                run,
                command: start
            }]
        )
        .commands()
        .is_empty()
    );
    (run, start, timer)
}
fn trace_start(puzzle: &Board) -> TraceEvent {
    TraceEvent::Start {
        solver: "bitmask".into(),
        puzzle: puzzle.clone(),
    }
}
fn node() -> TraceEvent {
    TraceEvent::Step {
        kind: TraceKind::Node,
        snapshot: TraceSnapshot::new(
            Board::default(),
            None,
            SearchStats::default(),
            0,
            CellIndex::FIRST,
        ),
    }
}

#[test]
fn genesis_validates_complete_data_and_reconstructs_identical_initial_state() {
    let original = app();
    let reconstructed = App::from_config(original.config().clone());
    assert_eq!(original.snapshot(), reconstructed.snapshot());
    assert_eq!(original.config(), reconstructed.config());
    let descriptor = SolverDescriptor {
        id: "external".parse().unwrap(),
        tracing: false,
    };
    assert!(SolverCatalog::try_from(vec![]).is_err());
    assert!(SolverCatalog::try_from(vec![descriptor.clone(), descriptor]).is_err());
    for (records, solver, speed, tick) in [
        (vec![], "bitmask", 1, 50),
        (vec![Record::from(Board::default())], "missing", 1, 50),
        (vec![Record::from(Board::default())], "bitmask", 129, 50),
        (vec![Record::from(Board::default())], "bitmask", 1, 0),
    ] {
        assert!(
            AppConfig::new(
                records,
                0,
                "test".into(),
                solver,
                support::catalog(),
                HistoryLimits::default(),
                Mode::Play,
                NonZeroUsize::new(speed).unwrap(),
                tick
            )
            .is_err()
        );
    }
}

#[test]
fn out_of_order_envelopes_and_early_timers_are_rejected_before_any_mutation() {
    let mut app = app();
    let checkpoint = app.checkpoint();
    let skipped = Activation::new(
        ActivationId::try_from(2).unwrap(),
        LogicalTime::ZERO,
        vec![Action::Help.into()],
    )
    .unwrap();
    assert_eq!(app.activate(&skipped), Err(ProtocolError::ActivationOrder));
    assert!(checkpoint.matches(&app));
    let (_, _, timer) = start(&mut app);
    let checkpoint = app.checkpoint();
    let id = app.last_activation().unwrap().checked_next().unwrap();
    let early = Activation::new(
        id,
        LogicalTime::from_millis(49),
        vec![Job::TimerFired { command: timer }],
    )
    .unwrap();
    assert_eq!(app.activate(&early), Err(ProtocolError::EarlyTimer));
    assert!(checkpoint.matches(&app));
    // A foreign timer does not consume the identified pending timer.
    activate(
        &mut app,
        49,
        vec![Job::TimerFired {
            command: CommandId::try_from(99).unwrap(),
        }],
    );
    assert_eq!(
        app.pending_timer(),
        Some((timer, LogicalTime::from_millis(50)))
    );
    let checkpoint = app.checkpoint();
    let id = app.last_activation().unwrap().checked_next().unwrap();
    let backwards =
        Activation::new(id, LogicalTime::from_millis(48), vec![Action::Help.into()]).unwrap();
    assert_eq!(
        app.activate(&backwards),
        Err(ProtocolError::ActivationOrder)
    );
    assert!(checkpoint.matches(&app));
}

#[test]
fn elapsed_logical_time_renews_pending_single_step_without_transport_poll_jobs() {
    let mut app = app();
    let (run, command, timer) = start(&mut app);
    assert_eq!(app.requested_events(), 1);
    assert_eq!(app.watch().playback(), Playback::AwaitingStep);
    let first = activate(&mut app, 50, vec![Job::TimerFired { command: timer }]);
    assert_eq!(first.commands().len(), 1);
    assert_eq!(app.requested_events(), 1);
    let second_timer = first.commands()[0].id;
    assert_eq!(
        app.pending_timer(),
        Some((second_timer, LogicalTime::from_millis(100)))
    );
    let second = activate(
        &mut app,
        100,
        vec![Job::TimerFired {
            command: second_timer,
        }],
    );
    assert_eq!(
        second.commands()[0].id,
        second_timer.checked_next().unwrap()
    );
    assert_eq!(app.watch().playback(), Playback::AwaitingStep);
    let puzzle = app.game().puzzle().clone();
    activate(
        &mut app,
        100,
        vec![Job::SolverEvents {
            run,
            command,
            events: vec![trace_start(&puzzle)],
        }],
    );
    assert_eq!(app.trace_cursor(), 1);
    assert_eq!(app.watch().playback(), Playback::Paused);
    assert_eq!(app.watch().run(), &RunState::Active(run));
    assert_eq!(app.requested_events(), 0);
    assert_eq!(app.pending_timer(), None);
}

#[test]
fn active_run_wrong_correlation_produces_portable_failure_and_correlated_cancel_ack() {
    let mut app = app();
    let (run, start, _) = start(&mut app);
    let wrong = CommandId::try_from(99).unwrap();
    let completion = activate(
        &mut app,
        0,
        vec![Job::SolverEvents {
            run,
            command: wrong,
            events: vec![node()],
        }],
    );
    assert_eq!(completion.commands().len(), 1);
    let cancellation = completion.commands()[0].id;
    assert_eq!(
        completion.commands()[0].kind,
        CommandKind::CancelSolver { run }
    );
    assert!(
        matches!(app.watch().run(),RunState::Stopped(StopReason::Failed(failure)) if failure.kind==FailureKind::Protocol)
    );
    assert_eq!(app.notice().severity(), Severity::Error);
    assert!(app.trace_history().is_empty());
    assert_eq!(app.current_run(), None);
    assert_eq!(
        app.snapshot().pending_cancellations,
        vec![(run, cancellation)]
    );
    activate(
        &mut app,
        0,
        vec![Job::SolverCancelled {
            run,
            command: start,
        }],
    );
    assert_eq!(
        app.snapshot().pending_cancellations,
        vec![(run, cancellation)]
    );
    activate(
        &mut app,
        0,
        vec![Job::SolverCancelled {
            run,
            command: cancellation,
        }],
    );
    assert!(app.snapshot().pending_cancellations.is_empty());
}

#[test]
fn replaced_run_feedback_is_stale_while_cancel_ack_remains_independently_correlated() {
    let mut app = app();
    let (old_run, old_start, _) = start(&mut app);
    let replaced = action(&mut app, 0, Action::NextPuzzle);
    let cancel = replaced.commands()[0].id;
    let fresh = action(&mut app, 0, Action::StepForward);
    let CommandKind::StartSolver { run: fresh_run, .. } = fresh.commands()[0].kind else {
        panic!("new puzzle needs a new producer")
    };
    let fresh_command = fresh.commands()[0].id;
    activate(
        &mut app,
        0,
        vec![Job::SolverStarted {
            run: fresh_run,
            command: fresh_command,
        }],
    );
    assert!(
        activate(
            &mut app,
            0,
            vec![
                Job::SolverStarted {
                    run: old_run,
                    command: old_start
                },
                Job::SolverEvents {
                    run: old_run,
                    command: old_start,
                    events: vec![node()]
                }
            ]
        )
        .commands()
        .is_empty()
    );
    assert_eq!(app.current_run(), Some((fresh_run, fresh_command)));
    assert!(app.trace_history().is_empty());
    activate(
        &mut app,
        0,
        vec![Job::SolverCancelled {
            run: old_run,
            command: cancel,
        }],
    );
    assert!(app.snapshot().pending_cancellations.is_empty());
    assert_eq!(app.current_run(), Some((fresh_run, fresh_command)));
}

#[test]
fn malformed_event_batch_preserves_valid_prefix_and_error_notice() {
    let mut app = app();
    let (run, command, _) = start(&mut app);
    let puzzle = app.game().puzzle().clone();
    let invalid = TraceEvent::Step {
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
    let completion = activate(
        &mut app,
        0,
        vec![Job::SolverEvents {
            run,
            command,
            events: vec![trace_start(&puzzle), node(), invalid],
        }],
    );
    assert_eq!(completion.commands().len(), 1);
    assert_eq!(app.trace_history().len(), 2);
    assert_eq!(app.trace_cursor(), 0);
    assert!(matches!(
        app.watch().run(),
        RunState::Stopped(StopReason::InvalidTrace(_))
    ));
    assert_eq!(app.notice().severity(), Severity::Error);
    assert!(app.notice().message().contains("assignment"));
    assert!(
        action(&mut app, 0, Action::HistoryEnd)
            .commands()
            .is_empty()
    );
    assert_eq!(app.trace_cursor(), 2);
}

#[test]
fn completed_producer_can_remain_paused_and_replay_without_new_solver_commands() {
    let solution: Board = SOLUTION.parse().unwrap();
    let mut cells = *solution.cells();
    cells[0] = 0;
    let puzzle = Board::try_from(cells).unwrap();
    let mut app = support::app(
        vec![Record::from(puzzle.clone())],
        0,
        "test".into(),
        "bitmask",
    )
    .unwrap();
    app.set_initial_mode(Mode::Watch).unwrap();
    let (run, command, _) = start(&mut app);
    action(&mut app, 0, Action::Help);
    let stats = SearchStats {
        nodes: 1,
        forced_moves: 1,
        ..SearchStats::default()
    };
    let snapshot = TraceSnapshot::new(solution.clone(), None, stats, 0, CellIndex::FIRST);
    let result = SearchResult::new(
        &puzzle,
        Some(solution.clone()),
        1,
        SearchExtent::Capped(NonZeroUsize::MIN),
        stats,
    )
    .unwrap();
    activate(
        &mut app,
        0,
        vec![Job::SolverEvents {
            run,
            command,
            events: vec![
                trace_start(&puzzle),
                TraceEvent::Step {
                    kind: TraceKind::Assignment {
                        cell: CellIndex::FIRST,
                        digit: Digit::try_from(5).unwrap(),
                        forced: true,
                    },
                    snapshot: snapshot.clone(),
                },
            ],
        }],
    );
    let finished = activate(
        &mut app,
        0,
        vec![Job::SolverCompleted {
            run,
            command,
            result: result.clone(),
            snapshot,
        }],
    );
    assert!(matches!(
        finished.commands()[0].kind,
        CommandKind::CancelSolver { .. }
    ));
    assert_eq!(app.watch().run(), &RunState::Finished(result));
    let result = app.watch().run().clone();
    let cancel = finished.commands()[0].id;
    let failure = Failure {
        kind: FailureKind::Execution,
        message: "cleanup hook failed".into(),
    };
    assert!(
        activate(
            &mut app,
            0,
            vec![Job::CommandFailed {
                command: cancel,
                failure: failure.clone(),
            }],
        )
        .commands()
        .is_empty()
    );
    assert_eq!(app.watch().run(), &result);
    assert_eq!(app.snapshot().last_command_failure, Some((cancel, failure)));
    assert_eq!(app.snapshot().pending_cancellations, vec![(run, cancel)]);
    assert_eq!(app.trace_cursor(), 0);
    assert_eq!(app.watch().playback(), Playback::Paused);
    action(&mut app, 0, Action::Escape);
    let mut states = Vec::new();
    for _ in 0..3 {
        assert!(
            action(&mut app, 0, Action::StepForward)
                .commands()
                .is_empty()
        );
        states.push(app.trace().clone());
    }
    assert_eq!(app.board(), &solution);
    action(&mut app, 0, Action::HistoryStart);
    for expected in &states {
        assert!(
            action(&mut app, 0, Action::StepForward)
                .commands()
                .is_empty()
        );
        assert_eq!(app.trace(), expected);
    }
    for expected in states[..2].iter().rev() {
        action(&mut app, 0, Action::StepBack);
        assert_eq!(app.trace(), expected);
    }
}

#[test]
fn session_end_prevents_restart_within_the_same_batch_and_still_accepts_cancel_ack() {
    let mut app = app();
    let (run, _, timer) = start(&mut app);
    let completed = activate(
        &mut app,
        0,
        vec![
            Job::SessionEnded {
                reason: SessionEndReason::InputEnded,
            },
            Action::ToggleRun.into(),
            Action::StepForward.into(),
        ],
    );
    assert_eq!(completed.commands().len(), 1);
    let cancel = completed.commands()[0].id;
    assert_eq!(
        completed.commands()[0].kind,
        CommandKind::CancelSolver { run }
    );
    assert!(app.closed());
    assert_eq!(app.watch().run(), &RunState::Stopped(StopReason::Cancelled));
    assert_eq!(app.pending_timer(), None);
    assert_eq!(app.requested_events(), 0);
    assert!(
        activate(
            &mut app,
            50,
            vec![
                Job::TimerFired { command: timer },
                Action::NextPuzzle.into(),
                Action::StepForward.into()
            ]
        )
        .commands()
        .is_empty()
    );
    assert_eq!(app.record_index(), 0);
    assert!(
        activate(
            &mut app,
            50,
            vec![Job::SolverCancelled {
                run,
                command: cancel
            }]
        )
        .commands()
        .is_empty()
    );
    assert!(app.snapshot().pending_cancellations.is_empty());
}

#[test]
fn preactivation_configuration_changes_are_genesis_and_later_changes_are_semantic_actions() {
    let mut app = support::app(
        vec![Record::from(Board::default())],
        0,
        "test".into(),
        "bitmask",
    )
    .unwrap();
    app.set_initial_mode(Mode::Watch).unwrap();
    app.set_events_per_tick(NonZeroUsize::MIN).unwrap();
    assert_eq!(
        app.snapshot(),
        App::from_config(app.config().clone()).snapshot()
    );
    action(&mut app, 0, Action::Faster);
    assert_eq!(app.events_per_tick(), 2);
    assert_eq!(app.config().events_per_tick().get(), 1);
    let before = app.snapshot();
    assert_eq!(
        app.set_events_per_tick(NonZeroUsize::MIN),
        Err(AppError::AlreadyActivated)
    );
    assert_eq!(
        app.set_initial_mode(Mode::Play),
        Err(AppError::AlreadyActivated)
    );
    assert_eq!(app.snapshot(), before);
}

#[test]
fn command_failures_require_an_emitted_identity_and_stop_matching_start_or_timer() {
    let failure = Failure {
        kind: FailureKind::Execution,
        message: "adapter hook failed".into(),
    };
    let mut fresh = app();
    let before = fresh.checkpoint();
    let premature = Activation::new(
        ActivationId::FIRST,
        LogicalTime::ZERO,
        vec![Job::CommandFailed {
            command: CommandId::FIRST,
            failure: failure.clone(),
        }],
    )
    .unwrap();
    assert_eq!(
        fresh.activate(&premature),
        Err(ProtocolError::CommandReference)
    );
    assert!(before.matches(&fresh));

    for fail_timer in [false, true] {
        let mut app = app();
        let started = action(&mut app, 0, Action::StepForward);
        let (run, start) = app.current_run().unwrap();
        let command = started.commands()[usize::from(fail_timer)].id;
        let before = app.checkpoint();
        let next = app.last_activation().unwrap().checked_next().unwrap();
        let future = Activation::new(
            next,
            LogicalTime::ZERO,
            vec![
                Action::NextPuzzle.into(),
                Job::CommandFailed {
                    command: started.commands()[1].id.checked_next().unwrap(),
                    failure: failure.clone(),
                },
            ],
        )
        .unwrap();
        assert_eq!(app.activate(&future), Err(ProtocolError::CommandReference));
        assert!(before.matches(&app));
        let completion = activate(
            &mut app,
            0,
            vec![Job::CommandFailed {
                command,
                failure: failure.clone(),
            }],
        );
        assert_eq!(completion.commands().len(), 1);
        assert_eq!(
            completion.commands()[0].kind,
            CommandKind::CancelSolver { run }
        );
        assert_eq!(
            app.watch().run(),
            &RunState::Stopped(StopReason::Failed(failure.clone()))
        );
        assert_eq!(
            app.snapshot().last_command_failure,
            Some((command, failure.clone()))
        );
        assert_eq!(app.pending_timer(), None);
        assert_eq!(app.current_run(), None);
        assert_eq!(app.requested_events(), 0);
        assert_eq!(app.watch().playback(), Playback::Paused);
        assert!(start <= command);
    }
}

#[test]
fn failed_cancellation_after_closure_retains_terminal_state_until_acknowledged() {
    let mut app = app();
    let (run, _, _) = start(&mut app);
    action(&mut app, 0, Action::Quit);
    let cancel = app.snapshot().pending_cancellations[0].1;
    activate(
        &mut app,
        0,
        vec![Job::SessionEnded {
            reason: SessionEndReason::Quit,
        }],
    );
    let stopped = app.watch().run().clone();
    let failure = Failure {
        kind: FailureKind::Execution,
        message: "cancel interceptor failed".into(),
    };
    assert!(
        activate(
            &mut app,
            0,
            vec![Job::CommandFailed {
                command: cancel,
                failure: failure.clone(),
            }]
        )
        .commands()
        .is_empty()
    );
    assert!(app.closed());
    assert!(app.quit());
    assert_eq!(app.watch().run(), &stopped);
    assert_eq!(app.snapshot().last_command_failure, Some((cancel, failure)));
    assert_eq!(app.snapshot().pending_cancellations, vec![(run, cancel)]);
    activate(
        &mut app,
        0,
        vec![Job::SolverCancelled {
            run,
            command: cancel,
        }],
    );
    assert!(app.snapshot().pending_cancellations.is_empty());
    assert_eq!(app.watch().run(), &stopped);
}

#[test]
fn sidebar_toggle_preserves_producer_playback_and_timer_state() {
    let mut app = app();
    assert!(!app.sidebar_open());
    let checkpoint = app.checkpoint();
    let (run, _, _) = start(&mut app);
    let mut expected = app.snapshot();
    let completion = action(&mut app, 0, Action::ToggleSidebar);
    assert!(completion.commands().is_empty());
    expected.sidebar_open = true;
    expected.last_activation = Some(completion.activation());
    assert_eq!(app.snapshot(), expected);
    assert_eq!(app.watch().run(), &RunState::Active(run));
    assert!(app.sidebar_open());
    assert!(!checkpoint.matches(&app));
    assert!(
        action(&mut app, 0, Action::ToggleSidebar)
            .commands()
            .is_empty()
    );
    assert!(!app.sidebar_open());
}

#[test]
fn sidebar_preference_survives_session_navigation_and_escape_closes_overlays_first() {
    let mut app = app();
    action(&mut app, 0, Action::ToggleSidebar);
    for navigation in [
        Action::ToggleMode,
        Action::NextPuzzle,
        Action::Reset,
        Action::ToggleMode,
    ] {
        action(&mut app, 0, navigation);
        assert!(app.sidebar_open());
    }
    action(&mut app, 0, Action::ChooseSolver);
    let picker = app.snapshot().overlay;
    assert!(matches!(
        picker,
        OverlayState::Base(BaseOverlay::SolverPicker { .. })
    ));
    action(&mut app, 0, Action::Help);
    assert!(matches!(app.snapshot().overlay, OverlayState::Help { .. }));
    assert!(action(&mut app, 0, Action::Escape).commands().is_empty());
    assert_eq!(app.snapshot().overlay, picker);
    assert!(app.sidebar_open());
    assert!(action(&mut app, 0, Action::Escape).commands().is_empty());
    assert_eq!(
        app.snapshot().overlay,
        OverlayState::Base(BaseOverlay::None)
    );
    assert!(app.sidebar_open());
    assert!(action(&mut app, 0, Action::Escape).commands().is_empty());
    assert!(!app.sidebar_open());
}

fn themes() -> ThemeCatalog {
    ThemeCatalog::try_from(vec![
        ThemeDescriptor {
            id: "ku".into(),
            label: "Ku".into(),
        },
        ThemeDescriptor {
            id: "nord".into(),
            label: "Nord".into(),
        },
        ThemeDescriptor {
            id: "solarized-dark".into(),
            label: "Solarized Dark".into(),
        },
    ])
    .unwrap()
}

#[test]
fn theme_catalog_and_genesis_reject_invalid_metadata_and_unknown_initial_selection() {
    assert_eq!(ThemeCatalog::try_from(Vec::new()), Err(AppError::Themes));
    for id in ["", "has space", "slash/id", "☀"] {
        assert_eq!(
            ThemeCatalog::try_from(vec![ThemeDescriptor {
                id: id.into(),
                label: "Theme".into()
            }]),
            Err(AppError::Themes)
        );
    }
    for label in ["", " \t\n"] {
        assert_eq!(
            ThemeCatalog::try_from(vec![ThemeDescriptor {
                id: "theme".into(),
                label: label.into()
            }]),
            Err(AppError::Themes)
        );
    }
    assert_eq!(
        ThemeCatalog::try_from(vec![
            ThemeDescriptor {
                id: "same".into(),
                label: "First".into()
            },
            ThemeDescriptor {
                id: "same".into(),
                label: "Second".into()
            },
        ]),
        Err(AppError::Themes)
    );
    let base = app();
    assert_eq!(base.theme().id, "ku");
    assert_eq!(base.config().themes(), &ThemeCatalog::default());
    assert_eq!(
        base.config().clone().with_themes(themes(), "missing"),
        Err(AppError::UnknownTheme("missing".into()))
    );
    let config = base.config().clone().with_themes(themes(), "nord").unwrap();
    let themed = App::from_config(config.clone());
    assert_eq!(config.initial_theme(), "nord");
    assert_eq!(themed.theme().label, "Nord");
    assert_eq!(themed.snapshot().theme, "nord");
    assert_eq!(
        App::from_config(themed.config().clone()).snapshot(),
        themed.snapshot()
    );
}

#[test]
fn theme_picker_pauses_without_cancelling_and_commits_only_on_accept() {
    let mut app = App::from_config(
        app()
            .config()
            .clone()
            .with_themes(themes(), "nord")
            .unwrap(),
    );
    let (run, command, _) = start(&mut app);
    let before = app.snapshot();
    assert!(
        action(&mut app, 0, Action::ChooseTheme)
            .commands()
            .is_empty()
    );
    assert_eq!(app.theme_picker_index(), Some(1));
    assert_eq!(app.picker_index(), None);
    assert_eq!(app.current_run(), Some((run, command)));
    assert_eq!(app.watch().run(), &before.run);
    assert_eq!(app.watch().playback(), Playback::Paused);
    assert_eq!(app.pending_timer(), None);
    assert_eq!(app.snapshot().play_board, before.play_board);
    assert_eq!(app.snapshot().play_events, before.play_events);
    assert_eq!(app.snapshot().trace, before.trace);
    assert_eq!(app.snapshot().trace_events, before.trace_events);
    action(
        &mut app,
        0,
        Action::Move {
            rows: 127,
            columns: 0,
        },
    );
    assert_eq!(app.theme_picker_index(), Some(2));
    assert_eq!(app.theme().id, "nord");
    action(&mut app, 0, Action::Escape);
    assert_eq!(app.theme_picker_index(), None);
    assert_eq!(app.theme().id, "nord");
    action(&mut app, 0, Action::ChooseTheme);
    action(
        &mut app,
        0,
        Action::Move {
            rows: -128,
            columns: 0,
        },
    );
    assert_eq!(app.theme_picker_index(), Some(0));
    action(
        &mut app,
        0,
        Action::Move {
            rows: 2,
            columns: 0,
        },
    );
    action(&mut app, 0, Action::Help);
    assert!(app.help_open());
    assert_eq!(app.theme_picker_index(), Some(2));
    assert!(action(&mut app, 0, Action::Accept).commands().is_empty());
    assert_eq!(app.theme().id, "nord");
    action(&mut app, 0, Action::Escape);
    assert_eq!(app.theme_picker_index(), Some(2));
    assert!(action(&mut app, 0, Action::Accept).commands().is_empty());
    assert_eq!(app.theme().id, "solarized-dark");
    assert_eq!(app.theme_picker_index(), None);
    assert_eq!(app.current_run(), Some((run, command)));
    for navigation in [Action::ToggleMode, Action::NextPuzzle, Action::Reset] {
        action(&mut app, 0, navigation);
        assert_eq!(app.theme().id, "solarized-dark");
    }
}
