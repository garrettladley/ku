//! Virtual command execution through the live driver, rendering, and portable replay.

use std::{
    cell::{Cell, RefCell},
    io,
    panic::{AssertUnwindSafe, catch_unwind},
};

use ku_cli::tui::{
    app::{Activation, ActivationId, App, Command, CommandKind, Job, LogicalTime, Playback},
    capability::{FunctionCapability, HookEvent, SessionCapability},
    input::Action,
    session::{Session, SessionExit},
    simulation::{FunctionCommands, RecordingOutput, ScriptedInput, SessionRecording, simulate},
    source::{SolverEventSource, SourcePoll},
};
use ku_core::{Board, TraceEvent, TraceSolver};

use ku_session::{
    history::{History, ReplayMode},
    simulation::{ScheduledJob, SimulationError},
};

fn app() -> App {
    ku_cli::tui::app::new(
        vec![Board::default().into()],
        0,
        "virtual protocol".into(),
        "bitmask",
    )
    .unwrap()
}

#[test]
fn board_first_presentation_and_optional_details_run_through_the_recorded_session() {
    use ku_cli::tui::{
        input::map_event,
        session::{SessionOutput, TerminalOutput},
    };
    use ratatui::{
        Terminal,
        backend::TestBackend,
        crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers},
    };

    let puzzle: Board =
        "530070000600195000098000060800060003400803001700020006060000280000419005000080079"
            .parse()
            .unwrap();
    let mut app = ku_cli::tui::app::new(
        vec![puzzle.clone().into()],
        0,
        "board focus".into(),
        "bitmask",
    )
    .unwrap();
    let commands = FunctionCommands::new(|_: &Command, _: LogicalTime| Ok(Vec::new()));
    let scheduler = commands.scheduler();
    for (index, code) in [
        KeyCode::Char('i'),
        KeyCode::Left,
        KeyCode::Left,
        KeyCode::Char('?'),
        KeyCode::Esc,
        KeyCode::Esc,
        KeyCode::Tab,
        KeyCode::Char('i'),
        KeyCode::Esc,
        KeyCode::Char('q'),
    ]
    .into_iter()
    .enumerate()
    {
        let action = map_event(Event::Key(KeyEvent::new(code, KeyModifiers::NONE))).unwrap();
        scheduler
            .borrow_mut()
            .schedule_at(
                LogicalTime::from_millis((index as u64 + 1) * 10),
                action.into(),
            )
            .unwrap();
    }
    let mut source = Source::default();
    let mut input = ScriptedInput::default();
    let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
    let mut screens = Vec::new();
    let mut hooks = (commands, SessionRecording::default());
    {
        let mut output = |app: &App| -> io::Result<()> {
            TerminalOutput::new(&mut terminal).present(app)?;
            let buffer = terminal.backend().buffer();
            let text = (0..30)
                .map(|y| {
                    (0..100)
                        .map(|x| buffer[(x, y)].symbol())
                        .collect::<String>()
                })
                .collect::<Vec<_>>()
                .join("\n");
            screens.push((app.sidebar_open(), text));
            Ok(())
        };
        let mut session = Session::new(&mut app, &mut input, &mut source, &mut output, &mut hooks);
        simulate(&mut session, &scheduler, 20).unwrap();
        session.close(SessionExit::Quit).unwrap();
    }
    assert!(!screens[0].0);
    assert!(!screens[0].1.contains("DETAILS ·"));
    assert!(screens[1].0 && screens[1].1.contains("DETAILS ·"));
    assert!(screens[3].1.contains("[5]") && screens[3].1.contains("LOCKED"));
    assert!(screens[4].1.contains("KU / CONTROLS"));
    assert!(screens[5].0 && screens[5].1.contains("DETAILS ·"));
    assert!(!screens[6].0 && !screens[6].1.contains("DETAILS ·"));
    assert!(!screens[7].0 && !screens[7].1.contains("SEARCH OBSERVATORY"));
    assert!(screens[8].0 && screens[8].1.contains("SEARCH OBSERVATORY"));
    assert!(!screens[9].0);
    assert_eq!(app.game().board(), &puzzle);
    assert!(app.game().events().is_empty());
    assert_eq!(source.starts, 0);
    assert_eq!(source.polls, 0);
    assert_portable_replay(hooks.1.history().unwrap(), &app);
}

#[derive(Default)]
struct Source {
    starts: usize,
    polls: usize,
    cancels: usize,
}

impl SolverEventSource for Source {
    fn start(&mut self, _: &'static dyn TraceSolver, _: &Board) -> io::Result<()> {
        self.starts += 1;
        Ok(())
    }

    fn poll_event(&mut self) -> io::Result<SourcePoll> {
        self.polls += 1;
        Ok(SourcePoll::Pending)
    }

    fn cancel(&mut self) {
        self.cancels += 1;
    }
}

fn assert_portable_replay(history: &History, app: &App) {
    let decoded = History::from_json(&history.to_json().unwrap()).unwrap();
    assert_eq!(
        decoded.replay(ReplayMode::Strict).unwrap().snapshot(),
        app.snapshot()
    );
    assert!(app.snapshot().pending_cancellations.is_empty());
    assert!(app.current_run().is_none());
    assert_eq!(app.watch().playback(), Playback::Paused);
}

#[derive(Default)]
struct SuppressFirst(bool);

impl SessionCapability for SuppressFirst {
    fn before_activation(
        &mut self,
        _: &App,
        activation: Activation,
    ) -> io::Result<Option<Activation>> {
        if !self.0 {
            self.0 = true;
            Ok(None)
        } else {
            Ok(Some(activation))
        }
    }
}

#[test]
fn suppressed_virtual_activation_does_not_consume_a_reducer_identity() {
    let mut app = app();
    let commands = FunctionCommands::new(|_: &Command, _: LogicalTime| Ok(Vec::new()));
    let scheduler = commands.scheduler();
    scheduler
        .borrow_mut()
        .schedule_at(LogicalTime::ZERO, Action::ToggleMode.into())
        .unwrap();
    scheduler
        .borrow_mut()
        .schedule_at(LogicalTime::from_millis(10), Action::Quit.into())
        .unwrap();
    let mut capabilities = (
        SuppressFirst::default(),
        (commands, SessionRecording::default()),
    );
    let mut source = Source::default();
    let mut input = ScriptedInput::default();
    let mut output = RecordingOutput::default();
    {
        let mut session = Session::new(
            &mut app,
            &mut input,
            &mut source,
            &mut output,
            &mut capabilities,
        );
        assert_eq!(simulate(&mut session, &scheduler, 10).unwrap(), 2);
        session.close(SessionExit::Quit).unwrap();
    }
    let history = capabilities.1.1.history().unwrap();
    assert_eq!(history.records()[0].activation().id(), ActivationId::FIRST);
    assert_eq!(
        history.records()[0].activation().time(),
        LogicalTime::from_millis(10)
    );
    assert_eq!(
        history.records()[0].activation().jobs(),
        &[Job::Action(Action::Quit)]
    );
    assert!(
        !history
            .records()
            .iter()
            .flat_map(|record| record.activation().jobs())
            .any(|job| matches!(job, Job::Action(Action::ToggleMode)))
    );
    assert!(scheduler.borrow().is_empty());
    assert_portable_replay(history, &app);
}

#[derive(Default)]
struct AdvanceFirst(bool);

impl SessionCapability for AdvanceFirst {
    fn before_activation(
        &mut self,
        _: &App,
        activation: Activation,
    ) -> io::Result<Option<Activation>> {
        if self.0 {
            return Ok(Some(activation));
        }
        self.0 = true;
        Ok(Some(
            Activation::new(
                activation.id(),
                LogicalTime::from_millis(100),
                activation.jobs().to_vec(),
            )
            .map_err(io::Error::other)?,
        ))
    }
}

#[test]
fn virtual_callbacks_use_accepted_time_after_transforms_and_manual_dispatch() {
    for manual in [false, true] {
        let mut app = app();
        let starts = RefCell::new(Vec::new());
        let commands = FunctionCommands::new(
            |command: &Command, now: LogicalTime| -> Result<Vec<ScheduledJob>, SimulationError> {
                if let CommandKind::StartSolver { run, .. } = command.kind {
                    starts.borrow_mut().push(now);
                    Ok(vec![ScheduledJob::new(
                        now,
                        Job::SolverStarted {
                            run,
                            command: command.id,
                        },
                    )])
                } else {
                    Ok(Vec::new())
                }
            },
        );
        let scheduler = commands.scheduler();
        if !manual {
            for action in [Action::ToggleMode, Action::StepForward] {
                scheduler
                    .borrow_mut()
                    .schedule_at(LogicalTime::ZERO, action.into())
                    .unwrap();
            }
        }
        // This input becomes overdue when the accepted first activation advances time.
        scheduler
            .borrow_mut()
            .schedule_at(LogicalTime::from_millis(25), Action::Help.into())
            .unwrap();
        scheduler
            .borrow_mut()
            .schedule_at(LogicalTime::from_millis(200), Action::Quit.into())
            .unwrap();
        let mut capabilities = (
            AdvanceFirst::default(),
            (commands, SessionRecording::default()),
        );
        let mut source = Source::default();
        let mut input = ScriptedInput::default();
        let mut output = RecordingOutput::default();
        {
            let mut session = Session::new(
                &mut app,
                &mut input,
                &mut source,
                &mut output,
                &mut capabilities,
            );
            if manual {
                session
                    .dispatch_activation(
                        Activation::new(
                            ActivationId::FIRST,
                            LogicalTime::from_millis(100),
                            vec![Action::ToggleMode.into(), Action::StepForward.into()],
                        )
                        .unwrap(),
                    )
                    .unwrap();
            }
            simulate(&mut session, &scheduler, 20).unwrap();
            session.close(SessionExit::Quit).unwrap();
        }
        assert_eq!(
            *starts.borrow(),
            [LogicalTime::from_millis(100)],
            "manual={manual}"
        );
        assert_eq!(source.starts, 0);
        assert_eq!(source.polls, 0);
        assert_eq!(scheduler.borrow().now(), LogicalTime::from_millis(200));
        let history = capabilities.1.1.history().unwrap();
        assert!(
            history
                .records()
                .iter()
                .all(|record| record.activation().time() >= LogicalTime::from_millis(100))
        );
        assert!(
            history
                .records()
                .iter()
                .flat_map(|record| record.activation().jobs())
                .any(|job| matches!(job, Job::SolverStarted { .. }))
        );
        assert_portable_replay(history, &app);
    }
}

#[test]
fn dropping_or_unwinding_virtual_sessions_clears_infrastructure_without_final_observers() {
    for unwind in [false, true] {
        let mut app = app();
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
                Ok(vec![
                    ScheduledJob::new(
                        now,
                        Job::SolverStarted {
                            run: *run,
                            command: command.id,
                        },
                    ),
                    ScheduledJob::new(
                        now.checked_add(1_000)?,
                        Job::SolverEvents {
                            run: *run,
                            command: command.id,
                            events: vec![TraceEvent::Start {
                                solver: solver.to_string(),
                                puzzle: puzzle.clone(),
                            }],
                        },
                    ),
                ])
            },
        );
        let scheduler = commands.scheduler();
        let finals = Cell::new(0);
        let observer = FunctionCapability::new(|event: HookEvent<'_>| {
            if matches!(event, HookEvent::SessionEnd { .. }) {
                finals.set(finals.get() + 1);
            }
            Ok(())
        });
        let mut capabilities = (observer, commands);
        let mut source = Source::default();
        let mut input = ScriptedInput::default();
        let mut output = RecordingOutput::default();
        let result = catch_unwind(AssertUnwindSafe(|| {
            let mut session = Session::new(
                &mut app,
                &mut input,
                &mut source,
                &mut output,
                &mut capabilities,
            );
            session.dispatch(Action::ToggleMode.into()).unwrap();
            session.dispatch(Action::StepForward.into()).unwrap();
            assert!(session.app().current_run().is_some());
            assert!(!scheduler.borrow().is_empty());
            assert!(!unwind, "deliberate simulated session unwind");
        }));
        assert_eq!(result.is_err(), unwind);
        assert!(scheduler.borrow().is_empty());
        assert_eq!(finals.get(), 0);
        assert_eq!(source.starts, 0);
        assert_eq!(source.polls, 0);
        assert_eq!(source.cancels, 1);
        assert!(app.current_run().is_none());
        assert!(app.snapshot().pending_cancellations.is_empty());
        assert_eq!(app.watch().playback(), Playback::Paused);
    }
}

#[test]
fn cancellation_hook_errors_leave_no_pending_acknowledgments_and_replay_portably() {
    for boundary in ["after quit", "before cancel", "after cancel"] {
        let mut app = app();
        let mut source = Source::default();
        let mut input = ScriptedInput::default();
        let mut output = RecordingOutput::default();
        let failed = Cell::new(false);
        let finals = Cell::new(0);
        let observer = FunctionCapability::new(|event: HookEvent<'_>| {
            let target = match &event {
                HookEvent::AfterActivation { activation, .. } if boundary == "after quit" => {
                    activation
                        .jobs()
                        .iter()
                        .any(|job| matches!(job, Job::Action(Action::Quit)))
                }
                HookEvent::BeforeCommand { command, .. } if boundary == "before cancel" => {
                    matches!(command.kind, CommandKind::CancelSolver { .. })
                }
                HookEvent::AfterCommand { command, .. } if boundary == "after cancel" => {
                    matches!(command.kind, CommandKind::CancelSolver { .. })
                }
                HookEvent::SessionEnd { app, result } => {
                    finals.set(finals.get() + 1);
                    assert!(result.is_err());
                    assert!(app.snapshot().pending_cancellations.is_empty());
                    false
                }
                _ => false,
            };
            if target && !failed.replace(true) {
                return Err(io::Error::other(format!("original {boundary} error")));
            }
            Ok(())
        });
        let mut capabilities = (observer, SessionRecording::default());
        {
            let mut session = Session::new(
                &mut app,
                &mut input,
                &mut source,
                &mut output,
                &mut capabilities,
            );
            session.dispatch(Action::ToggleMode.into()).unwrap();
            session.dispatch(Action::StepForward.into()).unwrap();
            assert!(session.app().current_run().is_some());
            assert_eq!(
                session
                    .dispatch(Action::Quit.into())
                    .unwrap_err()
                    .to_string(),
                format!("original {boundary} error")
            );
        }
        assert!(failed.get());
        assert_eq!(finals.get(), 1);
        assert_eq!(source.starts, 1);
        assert!(source.cancels >= 1);
        let history = capabilities.1.history().unwrap();
        let cancellations: Vec<_> = history
            .records()
            .iter()
            .flat_map(|record| record.completion().commands())
            .filter(|command| matches!(command.kind, CommandKind::CancelSolver { .. }))
            .collect();
        assert_eq!(cancellations.len(), 1);
        let cancel_id = cancellations[0].id;
        let jobs: Vec<_> = history
            .records()
            .iter()
            .flat_map(|record| record.activation().jobs())
            .collect();
        assert!(jobs.iter().any(
            |job| matches!(job, Job::SolverCancelled { command, .. } if *command == cancel_id)
        ));
        let failures: Vec<_> = jobs
            .iter()
            .filter_map(|job| match job {
                Job::CommandFailed { command, failure } => Some((*command, failure)),
                _ => None,
            })
            .collect();
        if boundary == "after quit" {
            assert!(failures.is_empty());
        } else {
            assert_eq!(failures.len(), 1);
            assert_eq!(failures[0].0, cancel_id);
            assert_eq!(failures[0].1.message, format!("original {boundary} error"));
        }
        assert_portable_replay(history, &app);
    }
}
