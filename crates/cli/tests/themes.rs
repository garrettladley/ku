//! Theme selection, rendering, and replay without changing puzzle state.

use std::{cell::Cell, io};

use ku_cli::tui::{
    app::{
        App, AppSnapshot, BaseOverlay, Command, CommandKind, Job, LogicalTime, OverlayState,
        RunState,
    },
    capability::{FunctionCapability, HookEvent},
    input::{Action, map_event},
    render,
    session::{Session, SessionExit},
    simulation::{FunctionCommands, ScriptedInput, SessionRecording, simulate},
    source::{FunctionSolverSource, SourcePoll},
    theme::{self, Palette},
};
use ku_core::{
    Board, CellIndex, Digit, SearchExtent, SearchResult, SearchStats, TraceEvent, TraceSnapshot,
    TraceSolver, dataset::Record,
};

use ku_session::{
    history::{History, ReplayMode},
    simulation::{ScheduledJob, SimulationError},
};
use ratatui::{
    Terminal,
    backend::TestBackend,
    buffer::Buffer,
    crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers},
};

const SOLUTION: &str =
    "534678912672195348198342567859761423426853791713924856961537284287419635345286179";
const MOCHA: &str = "catppuccin-mocha";

fn app() -> App {
    let solution: Board = SOLUTION.parse().unwrap();
    let mut cells = *solution.cells();
    cells[0] = 0;
    let app = ku_cli::tui::app::new(
        vec![Record::new(Board::try_from(cells).unwrap(), Some(solution)).unwrap()],
        0,
        "theme simulation".into(),
        "bitmask",
    )
    .unwrap();
    App::from_config(theme::configure(app.config().clone(), "ku").unwrap())
}

fn mocha_index(app: &App) -> usize {
    app.theme_catalog()
        .iter()
        .position(|theme| theme.id == MOCHA)
        .unwrap()
}

fn assert_domain_unchanged(before: &AppSnapshot, after: &AppSnapshot) {
    assert_eq!(after.board, before.board);
    assert_eq!(after.play_board, before.play_board);
    assert_eq!(after.play_selected, before.play_selected);
    assert_eq!(after.play_cursor, before.play_cursor);
    assert_eq!(after.play_events, before.play_events);
    assert_eq!(after.trace, before.trace);
    assert_eq!(after.trace_cursor, before.trace_cursor);
    assert_eq!(after.trace_events, before.trace_events);
    assert_eq!(after.run, before.run);
    assert_eq!(after.solver, before.solver);
}

fn assert_replay(recording: &SessionRecording, app: &App) {
    let decoded = History::from_json(&recording.history().unwrap().to_json().unwrap()).unwrap();
    assert_eq!(decoded.config().initial_theme(), "ku");
    assert_eq!(
        decoded.replay(ReplayMode::Strict).unwrap().snapshot(),
        app.snapshot()
    );
}

fn screen(buffer: &Buffer) -> String {
    buffer
        .content
        .chunks(usize::from(buffer.area.width))
        .map(|row| row.iter().map(|cell| cell.symbol()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn theme_picker_runs_through_real_session_and_renders_at_compact_and_wide_sizes() {
    for (width, height) in [(37, 22), (90, 40)] {
        let mut app = app();
        assert_eq!(app.theme().id, "ku");
        let target = mocha_index(&app);
        let calls = Cell::new(0);
        let commands = FunctionCommands::new(
            |_: &Command, _: LogicalTime| -> Result<Vec<ScheduledJob>, SimulationError> {
                calls.set(calls.get() + 1);
                panic!("theme browsing must not execute solver commands");
            },
        );
        let scheduler = commands.scheduler();
        let theme_key = map_event(Event::Key(KeyEvent::new(
            KeyCode::Char('t'),
            KeyModifiers::NONE,
        )))
        .unwrap();
        assert_eq!(theme_key, Action::ChooseTheme);
        let mut actions = vec![Action::Digit(Digit::try_from(5).unwrap()), theme_key];
        actions.extend(std::iter::repeat_n(
            Action::Move {
                rows: 1,
                columns: 0,
            },
            target,
        ));
        actions.extend([
            Action::Help,
            Action::Move {
                rows: -1,
                columns: 0,
            },
            Action::Escape,
            Action::Accept,
            Action::ToggleSidebar,
            Action::Help,
            Action::Escape,
            Action::Quit,
        ]);
        for (time, action) in actions.into_iter().enumerate() {
            scheduler
                .borrow_mut()
                .schedule_at(
                    LogicalTime::from_millis(u64::try_from(time).unwrap()),
                    action.into(),
                )
                .unwrap();
        }
        let mut baseline = None;
        let mut picker_help = 0;
        let mut restored_picker = 0;
        let mut committed = 0;
        let observer = FunctionCapability::new(|event: HookEvent<'_>| {
            if let HookEvent::AfterActivation {
                app, activation, ..
            } = event
            {
                if activation
                    .jobs()
                    .iter()
                    .any(|job| matches!(job, Job::Action(Action::Digit(_))))
                {
                    assert!(
                        app.play()
                            .board()
                            .is_solution_for(app.play().game().puzzle())
                    );
                    baseline = Some(app.snapshot());
                } else if let Some(before) = &baseline {
                    assert_domain_unchanged(before, &app.snapshot());
                }
                if app.help_open() && app.theme_picker_index().is_some() {
                    picker_help += 1;
                    assert_eq!(app.theme_picker_index(), Some(target));
                    assert_eq!(app.theme().id, "ku");
                }
                if activation.jobs().contains(&Job::Action(Action::Escape))
                    && app.theme_picker_index().is_some()
                {
                    restored_picker += 1;
                    assert!(!app.help_open());
                    assert_eq!(app.theme_picker_index(), Some(target));
                }
                if activation.jobs().contains(&Job::Action(Action::Accept)) {
                    committed += 1;
                    assert_eq!(app.theme().id, MOCHA);
                    assert_eq!(app.theme_picker_index(), None);
                }
            }
            Ok(())
        });
        let mut capabilities = (observer, (commands, SessionRecording::default()));
        let mut source = FunctionSolverSource::new(
            |_: &'static dyn TraceSolver, _: &Board| -> io::Result<Vec<SourcePoll>> {
                panic!("theme browsing must not start a solver source");
            },
        );
        let mut input = ScriptedInput::default();
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        let mut frames = Vec::new();
        {
            let mut output = |app: &App| {
                terminal
                    .draw(|frame| render::render(frame, app))
                    .map_err(io::Error::other)?;
                frames.push((app.snapshot(), terminal.backend().buffer().clone()));
                Ok(())
            };
            let mut session = Session::new(
                &mut app,
                &mut input,
                &mut source,
                &mut output,
                &mut capabilities,
            );
            simulate(&mut session, &scheduler, 64).unwrap();
            session.close(SessionExit::Quit).unwrap();
        }
        assert_eq!(calls.get(), 0);
        assert_eq!(source.starts(), 0);
        assert!(scheduler.borrow().is_empty());
        assert_replay(&capabilities.1.1, &app);
        assert!(
            capabilities
                .1
                .1
                .history()
                .unwrap()
                .records()
                .iter()
                .all(|record| record.completion().commands().is_empty())
        );
        // Release the observer's borrowed counters before inspecting them.
        drop(capabilities);
        assert!(picker_help >= 2);
        assert_eq!(restored_picker, 1);
        assert_eq!(committed, 1);
        let ku = Palette::for_id("ku");
        let mocha = Palette::for_id(MOCHA);
        assert_ne!(ku.table, mocha.table);
        assert_ne!(ku.accent, mocha.accent);
        for (id, palette) in [("ku", ku), (MOCHA, mocha)] {
            let (_, buffer) = frames
                .iter()
                .find(|(snapshot, _)| {
                    snapshot.theme == id
                        && snapshot.play_events == 1
                        && !snapshot.sidebar_open
                        && snapshot.overlay == OverlayState::Base(BaseOverlay::None)
                })
                .unwrap();
            assert_eq!(buffer[(0, 0)].bg, palette.table, "{id} at {width}x{height}");
            assert!(
                buffer.content.iter().any(|cell| cell.symbol() == "5"
                    && cell.bg == palette.selection_bg
                    && cell.fg == palette.selection_fg),
                "selected solved cell uses {id} at {width}x{height}"
            );
        }
        let (_, help) = frames
            .iter()
            .find(|(snapshot, _)| {
                snapshot.theme == MOCHA && matches!(snapshot.overlay, OverlayState::Help { .. })
            })
            .unwrap();
        assert!(screen(help).contains("KU / CONTROLS"));
        assert_eq!(help[(width / 2, height / 2)].bg, mocha.table);
        let (_, details) = frames
            .iter()
            .find(|(snapshot, _)| {
                snapshot.theme == MOCHA
                    && snapshot.sidebar_open
                    && snapshot.overlay == OverlayState::Base(BaseOverlay::None)
            })
            .unwrap();
        assert!(screen(details).contains("YOUR PUZZLE"));
        assert!(screen(details).contains("Complete · well played."));
        let panel_x = if width >= 89 { width - 17 } else { width / 2 };
        assert_eq!(details[(panel_x, height / 2)].bg, mocha.table);
    }
}

#[test]
fn changing_theme_preserves_completed_authored_solver_history_and_play_history() {
    let mut app = app();
    let target = mocha_index(&app);
    let starts = Cell::new(0);
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
            starts.set(starts.get() + 1);
            let solution: Board = SOLUTION.parse().unwrap();
            let result = SearchResult::new(
                puzzle,
                Some(solution.clone()),
                1,
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
                    now.checked_add(1)?,
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
                    now.checked_add(2)?,
                    Job::SolverCompleted {
                        run: *run,
                        command: command.id,
                        result,
                        snapshot: TraceSnapshot::new(
                            solution,
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
    for (time, action) in [
        (0, Action::Digit(Digit::try_from(5).unwrap())),
        (1, Action::ToggleMode),
        (2, Action::StepForward),
        (8, Action::HistoryEnd),
    ] {
        scheduler
            .borrow_mut()
            .schedule_at(LogicalTime::from_millis(time), action.into())
            .unwrap();
    }
    let mut actions = vec![Action::ChooseTheme];
    actions.extend(std::iter::repeat_n(
        Action::Move {
            rows: 1,
            columns: 0,
        },
        target,
    ));
    actions.extend([Action::Accept, Action::Quit]);
    for (offset, action) in actions.into_iter().enumerate() {
        scheduler
            .borrow_mut()
            .schedule_at(
                LogicalTime::from_millis(10 + u64::try_from(offset).unwrap()),
                action.into(),
            )
            .unwrap();
    }
    let mut retained = None;
    let observer = FunctionCapability::new(|event: HookEvent<'_>| {
        if let HookEvent::AfterActivation {
            app, activation, ..
        } = event
        {
            if activation
                .jobs()
                .contains(&Job::Action(Action::ChooseTheme))
            {
                assert!(matches!(app.watch().run(), RunState::Finished(_)));
                assert_eq!(app.trace_history().len(), 2);
                assert!(app.trace().finished().is_some());
                retained = Some(app.snapshot());
            } else if let Some(before) = &retained {
                assert_domain_unchanged(before, &app.snapshot());
            }
        }
        Ok(())
    });
    let mut capabilities = (observer, (commands, SessionRecording::default()));
    let mut source = FunctionSolverSource::new(
        |_: &'static dyn TraceSolver, _: &Board| -> io::Result<Vec<SourcePoll>> {
            panic!("authored solver commands must bypass solver infrastructure");
        },
    );
    let mut input = ScriptedInput::default();
    let mut terminal = Terminal::new(TestBackend::new(90, 40)).unwrap();
    {
        let mut output = |app: &App| {
            terminal
                .draw(|frame| render::render(frame, app))
                .map(|_| ())
                .map_err(io::Error::other)
        };
        let mut session = Session::new(
            &mut app,
            &mut input,
            &mut source,
            &mut output,
            &mut capabilities,
        );
        simulate(&mut session, &scheduler, 64).unwrap();
        session.close(SessionExit::Quit).unwrap();
    }
    assert_eq!(starts.get(), 1);
    assert_eq!(source.starts(), 0);
    assert_eq!(app.theme().id, MOCHA);
    assert!(
        app.play()
            .board()
            .is_solution_for(app.play().game().puzzle())
    );
    assert_eq!(app.play().game().events().len(), 1);
    assert_eq!(app.trace_history().len(), 2);
    assert!(app.trace().finished().is_some());
    assert_replay(&capabilities.1.1, &app);
}
