//! Runtime failure propagation, adapter cleanup, and session lifecycle contracts.

use std::{
    cell::{Cell, RefCell},
    io,
    panic::{AssertUnwindSafe, catch_unwind},
    rc::Rc,
};

use ku_cli::tui::{
    app::{
        App, Command, CommandKind, FailureKind, Job, LogicalTime, Playback, RunState, StopReason,
    },
    capability::{FunctionCapability, HookEvent, SessionCapability},
    input::Action,
    session::{Session, SessionExit},
    simulation::{FunctionCommands, RecordingOutput, ScriptedInput, SessionRecording, simulate},
    source::{SolverEventSource, SourcePoll},
};
use ku_core::{
    Board, CellIndex, SearchExtent, SearchResult, SearchStats, TraceEvent, TraceSnapshot,
    TraceSolver,
};

use ku_session::{
    history::{History, ReplayMode},
    simulation::{ScheduledJob, SimulationError},
};
fn app() -> App {
    ku_cli::tui::app::new(
        vec![Board::default().into()],
        0,
        "protocol".into(),
        "bitmask",
    )
    .unwrap()
}
#[derive(Default)]
struct Counts {
    starts: usize,
    polls: usize,
    cancels: usize,
}
struct Source {
    counts: Rc<RefCell<Counts>>,
    fail: &'static str,
}
impl SolverEventSource for Source {
    fn start(&mut self, _: &'static dyn TraceSolver, _: &Board) -> io::Result<()> {
        self.counts.borrow_mut().starts += 1;
        if self.fail == "start" {
            return Err(io::Error::other("original start failure"));
        }
        Ok(())
    }
    fn poll_event(&mut self) -> io::Result<SourcePoll> {
        self.counts.borrow_mut().polls += 1;
        if self.fail == "poll" {
            return Err(io::Error::other("original poll failure"));
        }
        Ok(SourcePoll::Pending)
    }
    fn cancel(&mut self) {
        self.counts.borrow_mut().cancels += 1;
    }
}
fn name(hook: &HookEvent<'_>) -> &'static str {
    match hook {
        HookEvent::SessionStart { .. } => "start session",
        HookEvent::BeforeActivation { .. } => "before activation",
        HookEvent::AfterActivation { .. } => "after activation",
        HookEvent::BeforeCommand { .. } => "before command",
        HookEvent::AfterCommand { .. } => "after command",
        HookEvent::BeforeRender { .. } => "before render",
        HookEvent::AfterRender { .. } => "after render",
        HookEvent::SessionEnd { .. } => "end session",
        HookEvent::Error { .. } => "error",
    }
}
#[test]
fn every_fallible_boundary_cleans_up_and_preserves_the_primary_error() {
    for boundary in [
        "start session",
        "before activation",
        "after activation",
        "before command",
        "after command",
        "before render",
        "after render",
        "end session",
        "input",
        "output",
        "start",
        "poll",
    ] {
        let mut app = app();
        let counts = Rc::new(RefCell::new(Counts::default()));
        let mut source = Source {
            counts: counts.clone(),
            fail: boundary,
        };
        let mut actions = [Action::ToggleMode, Action::StepForward, Action::Quit].into_iter();
        let mut input = || {
            if boundary == "input" {
                Err(io::Error::other("original input failure"))
            } else {
                Ok(actions.next())
            }
        };
        let mut output = |_: &App| {
            if boundary == "output" {
                Err(io::Error::other("original output failure"))
            } else {
                Ok(())
            }
        };
        let mut raised = false;
        let mut finals = 0;
        let mut errors = Vec::new();
        let mut hooks = FunctionCapability::new(|hook: HookEvent<'_>| {
            if let HookEvent::SessionEnd { app, .. } = &hook {
                finals += 1;
                assert_eq!(app.watch().playback(), Playback::Paused);
                assert!(app.current_run().is_none());
                if boundary != "end session" {
                    return Err(io::Error::other("secondary cleanup failure"));
                }
            }
            if let HookEvent::Error { error, .. } = &hook {
                errors.push(error.to_string());
            }
            if name(&hook) == boundary && !raised {
                raised = true;
                return Err(io::Error::other(format!("original {boundary} failure")));
            }
            Ok(())
        });
        let mut session = Session::new(&mut app, &mut input, &mut source, &mut output, &mut hooks);
        let error = session.run().unwrap_err();
        assert!(
            error
                .to_string()
                .contains(&format!("original {boundary} failure")),
            "{boundary}: {error}"
        );
        drop(session);
        assert_eq!(finals, 1, "{boundary}");
        assert!(counts.borrow().cancels >= 1, "{boundary}");
        assert_eq!(errors.len(), 1, "{boundary}");
    }
}
#[test]
fn pending_transport_polls_do_not_enter_recorded_activations() {
    let mut app = app();
    let counts = Rc::new(RefCell::new(Counts::default()));
    let mut source = Source {
        counts: counts.clone(),
        fail: "",
    };
    let mut input = ScriptedInput::new([
        Action::ToggleMode,
        Action::StepForward,
        Action::Tick,
        Action::Tick,
        Action::Quit,
    ]);
    let mut output = RecordingOutput::default();
    let mut recording = SessionRecording::default();
    Session::new(
        &mut app,
        &mut input,
        &mut source,
        &mut output,
        &mut recording,
    )
    .run()
    .unwrap();
    assert_eq!(counts.borrow().polls, 3);
    let history = recording.history().unwrap();
    assert_eq!(
        history
            .records()
            .iter()
            .flat_map(|record| record.activation().jobs())
            .filter(|job| matches!(job, Job::SolverEvents { .. }))
            .count(),
        0
    );
    assert_eq!(
        history.replay(ReplayMode::Strict).unwrap().snapshot(),
        app.snapshot()
    );
}
#[test]
fn solver_io_failures_are_portable_typed_history_outcomes() {
    for (failure, kind) in [
        ("start", FailureKind::Start),
        ("poll", FailureKind::Execution),
    ] {
        let mut app = app();
        let mut input = ScriptedInput::new([Action::ToggleMode, Action::StepForward]);
        let mut source = Source {
            counts: Default::default(),
            fail: failure,
        };
        let mut output = RecordingOutput::default();
        let mut recording = SessionRecording::default();
        assert!(
            Session::new(
                &mut app,
                &mut input,
                &mut source,
                &mut output,
                &mut recording
            )
            .run()
            .is_err()
        );
        assert!(
            matches!(app.watch().run(), RunState::Stopped(StopReason::Failed(data)) if data.kind == kind)
        );
        let json = recording.history().unwrap().to_json().unwrap();
        let decoded = History::from_json(&json).unwrap();
        assert_eq!(
            decoded.replay(ReplayMode::Strict).unwrap().snapshot(),
            app.snapshot()
        );
    }
}
#[test]
fn authored_commands_drive_the_actual_session_and_portable_replay_with_virtual_time() {
    let mut app = app();
    let calls = Cell::new(0);
    let commands = FunctionCommands::new(
        |command: &Command, now: LogicalTime| -> Result<Vec<ScheduledJob>, SimulationError> {
            let CommandKind::StartSolver {
                run,
                solver,
                puzzle,
            } = &command.kind
            else {
                return Ok(Vec::new());
            };
            calls.set(calls.get() + 1);
            let result = SearchResult::new(
                puzzle,
                None,
                0,
                SearchExtent::Exhausted,
                SearchStats::default(),
            )
            .unwrap();
            Ok(vec![
                ScheduledJob::new(
                    now,
                    Job::SolverStarted {
                        run: *run,
                        command: command.id,
                    },
                ),
                ScheduledJob::new(
                    now.checked_add(10)?,
                    Job::SolverEvents {
                        run: *run,
                        command: command.id,
                        events: vec![TraceEvent::Start {
                            solver: solver.to_string(),
                            puzzle: puzzle.clone(),
                        }],
                    },
                ),
                ScheduledJob::new(
                    now.checked_add(20)?,
                    Job::SolverCompleted {
                        run: *run,
                        command: command.id,
                        result,
                        snapshot: TraceSnapshot::new(
                            puzzle.clone(),
                            None,
                            SearchStats::default(),
                            0,
                            CellIndex::FIRST,
                        ),
                    },
                ),
            ])
        },
    );
    let scheduler = commands.scheduler();
    scheduler
        .borrow_mut()
        .schedule_at(LogicalTime::ZERO, Action::ToggleMode.into())
        .unwrap();
    scheduler
        .borrow_mut()
        .schedule_at(LogicalTime::ZERO, Action::ToggleRun.into())
        .unwrap();
    scheduler
        .borrow_mut()
        .schedule_at(LogicalTime::from_millis(100), Action::Quit.into())
        .unwrap();
    let counts = Rc::new(RefCell::new(Counts::default()));
    let mut source = Source {
        counts: counts.clone(),
        fail: "",
    };
    let mut input = ScriptedInput::default();
    let mut output = RecordingOutput::default();
    let mut capabilities = (commands, SessionRecording::default());
    {
        let mut session = Session::new(
            &mut app,
            &mut input,
            &mut source,
            &mut output,
            &mut capabilities,
        );
        simulate(&mut session, &scheduler, 50).unwrap();
        session.close(SessionExit::Quit).unwrap();
    }
    assert_eq!(calls.get(), 1);
    assert_eq!(counts.borrow().starts, 0);
    assert_eq!(counts.borrow().polls, 0);
    assert!(scheduler.borrow().is_empty());
    assert!(matches!(app.watch().run(), RunState::Finished(_)));
    assert_eq!(app.logical_time(), LogicalTime::from_millis(100));
    let json = capabilities.1.history().unwrap().to_json().unwrap();
    assert_eq!(
        History::from_json(&json)
            .unwrap()
            .replay(ReplayMode::Strict)
            .unwrap()
            .snapshot(),
        app.snapshot()
    );
    assert!(output.frames().iter().any(|frame| frame.trace_events == 2));
}
#[test]
fn drop_and_unwinding_cancel_without_running_final_hooks() {
    for panic in [false, true] {
        let mut app = app();
        let mut input = ScriptedInput::default();
        let counts = Rc::new(RefCell::new(Counts::default()));
        let mut source = Source {
            counts: counts.clone(),
            fail: "",
        };
        let mut output = RecordingOutput::default();
        let finalized = Cell::new(0);
        let mut hooks = FunctionCapability::new(|hook: HookEvent<'_>| {
            if matches!(hook, HookEvent::SessionEnd { .. }) {
                finalized.set(finalized.get() + 1);
            }
            Ok(())
        });
        let result = catch_unwind(AssertUnwindSafe(|| {
            let mut session =
                Session::new(&mut app, &mut input, &mut source, &mut output, &mut hooks);
            session.dispatch(Action::ToggleMode.into()).unwrap();
            session.dispatch(Action::StepForward.into()).unwrap();
            assert!(session.app().current_run().is_some());
            assert!(!panic, "deliberate unwind");
        }));
        assert_eq!(result.is_err(), panic);
        assert_eq!(counts.borrow().cancels, 1);
        assert_eq!(finalized.get(), 0);
        assert!(app.current_run().is_none());
        assert_eq!(app.watch().playback(), Playback::Paused);
    }
}
#[test]
fn nested_capabilities_preserve_order_and_primary_command_failures() {
    let app = app();
    let log = Rc::new(RefCell::new(Vec::new()));
    let observer = |label| {
        let log = log.clone();
        FunctionCapability::new(move |hook: HookEvent<'_>| {
            log.borrow_mut().push((label, name(&hook)));
            Ok(())
        })
    };
    let mut hooks = (observer("outer"), observer("inner"));
    let command = Command {
        id: ku_session::protocol::CommandId::FIRST,
        kind: CommandKind::CancelSolver {
            run: ku_session::protocol::RunId::FIRST,
        },
    };
    hooks
        .wrap_command(&app, &command, &mut |_| {
            log.borrow_mut().push(("handler", "execute"));
            Ok(Vec::new())
        })
        .unwrap();
    assert_eq!(
        *log.borrow(),
        [
            ("outer", "before command"),
            ("inner", "before command"),
            ("handler", "execute"),
            ("inner", "after command"),
            ("outer", "after command")
        ]
    );
    let mut broken = FunctionCapability::new(|hook: HookEvent<'_>| {
        if matches!(hook, HookEvent::AfterCommand { .. }) {
            Err(io::Error::other("secondary"))
        } else {
            Ok(())
        }
    });
    assert_eq!(
        broken
            .wrap_command(&app, &command, &mut |_| Err(io::Error::other("primary")))
            .unwrap_err()
            .to_string(),
        "primary"
    );
}

#[test]
fn malformed_worker_traces_become_replayable_protocol_failures() {
    use ku_cli::tui::source::FunctionSolverSource;
    use ku_core::TraceKind;
    let mut app = app();
    let mut source = FunctionSolverSource::new(|_: &'static dyn TraceSolver, puzzle: &Board| {
        // A node before the required trace start is a producer protocol failure.
        Ok(vec![SourcePoll::Event(TraceEvent::Step {
            kind: TraceKind::Node,
            snapshot: TraceSnapshot::new(
                puzzle.clone(),
                None,
                SearchStats::default(),
                0,
                CellIndex::FIRST,
            ),
        })])
    });
    let mut input = ScriptedInput::new([Action::ToggleMode, Action::StepForward, Action::Quit]);
    let mut output = RecordingOutput::default();
    let mut recording = SessionRecording::default();
    Session::new(
        &mut app,
        &mut input,
        &mut source,
        &mut output,
        &mut recording,
    )
    .run()
    .unwrap();
    assert!(
        matches!(app.watch().run(), RunState::Stopped(StopReason::Failed(failure)) if failure.kind == FailureKind::Protocol)
    );
    assert!(app.trace_history().is_empty());
    let history = recording.history().unwrap();
    assert!(history.records().iter().flat_map(|record| record.activation().jobs()).any(|job| matches!(job, Job::SolverFailed { failure, .. } if failure.kind == FailureKind::Protocol)));
    let decoded = History::from_json(&history.to_json().unwrap()).unwrap();
    assert_eq!(
        decoded.replay(ReplayMode::Strict).unwrap().snapshot(),
        app.snapshot()
    );
}

#[test]
fn live_clock_timestamps_only_fire_due_timers_and_record_delayed_delivery() {
    use std::{collections::VecDeque, time::Duration};

    use ku_cli::tui::input::InputSource;
    struct TimedInput {
        actions: VecDeque<(u64, Action)>,
        now: u64,
    }
    impl InputSource for TimedInput {
        fn next_action(&mut self) -> io::Result<Option<Action>> {
            Ok(self.actions.pop_front().map(|(time, action)| {
                self.now = time;
                action
            }))
        }
        fn elapsed(&self) -> Option<Duration> {
            Some(Duration::from_millis(self.now))
        }
    }
    let mut input = TimedInput {
        actions: [
            (10, Action::ToggleMode),
            (25, Action::StepForward),
            (50, Action::Tick),
            (100, Action::Tick),
            (120, Action::Quit),
        ]
        .into(),
        now: 0,
    };
    let mut app = app();
    let counts = Rc::new(RefCell::new(Counts::default()));
    let mut source = Source {
        counts: counts.clone(),
        fail: "",
    };
    let mut output = RecordingOutput::default();
    let mut recording = SessionRecording::default();
    Session::new(
        &mut app,
        &mut input,
        &mut source,
        &mut output,
        &mut recording,
    )
    .run()
    .unwrap();
    assert_eq!(
        counts.borrow().polls,
        2,
        "the early transport tick must not wake the model or poll its source"
    );
    let history = recording.history().unwrap();
    let timers: Vec<_> = history
        .records()
        .iter()
        .filter(|record| {
            record
                .activation()
                .jobs()
                .iter()
                .any(|job| matches!(job, Job::TimerFired { .. }))
        })
        .collect();
    assert_eq!(timers.len(), 1);
    assert_eq!(timers[0].activation().time(), LogicalTime::from_millis(100));
    assert!(history.records().iter().flat_map(|record| record.completion().commands()).any(|command| matches!(command.kind, CommandKind::ScheduleTimer { deadline } if deadline == LogicalTime::from_millis(75))));
    assert_eq!(
        History::from_json(&history.to_json().unwrap())
            .unwrap()
            .replay(ReplayMode::Strict)
            .unwrap()
            .snapshot(),
        app.snapshot()
    );
}
