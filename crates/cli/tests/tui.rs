//! Terminal input, rendering, and deterministic session integration.

use std::{
    collections::VecDeque,
    io,
    num::NonZeroUsize,
    thread,
    time::{Duration, Instant},
};

use ku_cli::tui::{
    app::{App, Mode},
    input::{Action, InputSource, map_event},
    run_session,
    source::{FunctionSolverSource, SolverEventSource, SourcePoll, WorkerSource},
    theme::Palette,
};
use ku_core::{
    Board, CandidateMask, CellIndex, Digit, SearchExtent, SearchResult, SearchStats, TraceEvent,
    TraceKind, TraceSnapshot, TraceSolver, dataset::Record,
};
use ku_solvers::{built_in_solvers, built_in_trace_solvers};
use ratatui::{
    Terminal,
    backend::{Backend, ClearType, TestBackend, WindowSize},
    buffer::{Buffer, Cell},
    crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers},
    layout::{Position, Size},
    style::Modifier,
};

const SOLUTION: &str =
    "534678912672195348198342567859761423426853791713924856961537284287419635345286179";

fn records() -> Vec<Record> {
    let solution: Board = SOLUTION.parse().unwrap();
    let mut cells = *solution.cells();
    cells[0] = 0;
    cells[1] = 0;
    vec![
        Record::new(Board::try_from(cells).unwrap(), Some(solution)).unwrap(),
        Record::new(Board::default(), None).unwrap(),
    ]
}

fn app() -> App {
    ku_cli::tui::app::new(
        records(),
        0,
        "scripted puzzles".into(),
        built_in_solvers()[0].name(),
    )
    .unwrap()
}

fn key(code: KeyCode) -> Action {
    map_event(Event::Key(KeyEvent::new(code, KeyModifiers::NONE))).unwrap()
}

fn character(character: char) -> Action {
    key(KeyCode::Char(character))
}

struct ScriptedInput {
    actions: VecDeque<Action>,
    fail: bool,
}

impl ScriptedInput {
    fn new(actions: impl IntoIterator<Item = Action>) -> Self {
        Self {
            actions: actions.into_iter().collect(),
            fail: false,
        }
    }
}

impl InputSource for ScriptedInput {
    fn next_action(&mut self) -> io::Result<Option<Action>> {
        if self.fail {
            return Err(io::Error::other("scripted input failure"));
        }
        Ok(self.actions.pop_front())
    }
}

#[derive(Default)]
struct ScriptedSolver {
    script: Vec<TraceEvent>,
    pending: VecDeque<TraceEvent>,
    starts: Vec<(&'static str, Board)>,
    polls: usize,
    unavailable_polls: usize,
    cancellations: usize,
    fail_start: bool,
    fail_poll: bool,
}

impl SolverEventSource for ScriptedSolver {
    fn start(&mut self, solver: &'static dyn TraceSolver, puzzle: &Board) -> io::Result<()> {
        if self.fail_start {
            return Err(io::Error::other("scripted start failure"));
        }
        self.starts.push((solver.name(), puzzle.clone()));
        self.pending = self.script.clone().into();
        if let Some(TraceEvent::Start {
            solver: selected,
            puzzle: original,
        }) = self.pending.front_mut()
        {
            *selected = solver.name().to_owned();
            *original = puzzle.clone();
        }
        Ok(())
    }

    fn poll_event(&mut self) -> io::Result<SourcePoll> {
        self.polls += 1;
        if self.fail_poll {
            return Err(io::Error::other("scripted poll failure"));
        }
        if self.unavailable_polls != 0 {
            self.unavailable_polls -= 1;
            return Ok(SourcePoll::Pending);
        }
        Ok(self
            .pending
            .pop_front()
            .map_or(SourcePoll::Closed, SourcePoll::Event))
    }

    fn cancel(&mut self) {
        self.cancellations += 1;
        self.pending.clear();
    }
}

// A TestBackend wrapper captures every production-loop frame and can inject a
// draw failure without raw mode, a terminal device or another crate dependency.
struct RecordingBackend {
    inner: TestBackend,
    frames: Vec<Buffer>,
    fail_draw: bool,
}

impl RecordingBackend {
    fn new(width: u16, height: u16) -> Self {
        Self {
            inner: TestBackend::new(width, height),
            frames: Vec::new(),
            fail_draw: false,
        }
    }
}

impl Backend for RecordingBackend {
    type Error = io::Error;

    fn draw<'a, I>(&mut self, content: I) -> io::Result<()>
    where
        I: Iterator<Item = (u16, u16, &'a Cell)>,
    {
        if self.fail_draw {
            return Err(io::Error::other("scripted rendering failure"));
        }
        self.inner.draw(content).unwrap();
        Ok(())
    }

    fn hide_cursor(&mut self) -> io::Result<()> {
        self.inner.hide_cursor().unwrap();
        Ok(())
    }
    fn show_cursor(&mut self) -> io::Result<()> {
        self.inner.show_cursor().unwrap();
        Ok(())
    }
    fn get_cursor_position(&mut self) -> io::Result<Position> {
        Ok(self.inner.get_cursor_position().unwrap())
    }
    fn set_cursor_position<P: Into<Position>>(&mut self, position: P) -> io::Result<()> {
        self.inner.set_cursor_position(position).unwrap();
        Ok(())
    }
    fn clear(&mut self) -> io::Result<()> {
        self.inner.clear().unwrap();
        Ok(())
    }
    fn clear_region(&mut self, clear_type: ClearType) -> io::Result<()> {
        self.inner.clear_region(clear_type).unwrap();
        Ok(())
    }
    fn size(&self) -> io::Result<Size> {
        Ok(self.inner.size().unwrap())
    }
    fn window_size(&mut self) -> io::Result<WindowSize> {
        Ok(self.inner.window_size().unwrap())
    }
    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush().unwrap();
        self.frames.push(self.inner.buffer().clone());
        Ok(())
    }
}

fn text(buffer: &Buffer) -> String {
    let width = usize::from(buffer.area.width);
    buffer
        .content()
        .chunks(width)
        .map(|row| row.iter().map(Cell::symbol).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}

fn assert_frame(terminal: &Terminal<RecordingBackend>, phrase: &str) {
    assert!(
        terminal
            .backend()
            .frames
            .iter()
            .any(|frame| text(frame).contains(phrase)),
        "no frame contains {phrase:?}"
    );
}

#[test]
fn scripted_play_session_validates_completes_replays_and_opens_help() {
    let mut app = app();
    let mut terminal = Terminal::new(RecordingBackend::new(100, 30)).unwrap();
    let mut source = ScriptedSolver::default();
    let mut input = ScriptedInput::new([
        key(KeyCode::Right),
        key(KeyCode::Right),
        character('0'),
        key(KeyCode::Left),
        key(KeyCode::Left),
        character('4'),
        character('5'),
        key(KeyCode::Backspace),
        character('5'),
        key(KeyCode::Right),
        character('3'),
        character('['),
        character('9'),
        key(KeyCode::End),
        character('?'),
        character('9'),
        key(KeyCode::Esc),
        character('q'),
    ]);
    run_session(&mut terminal, &mut app, &mut input, &mut source).unwrap();

    assert!(app.quit());
    assert_eq!(app.game().board().to_string(), SOLUTION);
    assert!(app.game().is_complete());
    assert_eq!(app.game().events().len(), 4);
    assert_eq!(app.game_cursor(), 4);
    assert_eq!(app.board_cells(), *app.game().board().cells());
    assert!(!app.help_open());
    assert!(source.starts.is_empty());
    assert!(source.cancellations > 0);
    for phrase in [
        "given clue",
        "conflicts in row 1",
        "move 2/2",
        "Reviewing history.",
        "KU / CONTROLS",
        "WATCH & REPLAY",
        "SOLVED",
    ] {
        assert_frame(&terminal, phrase);
    }
    assert!(terminal.backend().frames.iter().any(|frame| {
        frame
            .content()
            .iter()
            .any(|cell| cell.bg == Palette::default().alert)
    }));
    assert!(
        terminal
            .backend()
            .frames
            .iter()
            .any(
                |frame| frame.content().iter().any(|cell| cell.symbol() == "4"
                    && cell.bg != Palette::default().selection_bg
                    && cell.modifier.contains(Modifier::BOLD))
            )
    );
}

fn scripted_trace(puzzle: &Board) -> Vec<TraceEvent> {
    let solution: Board = SOLUTION.parse().unwrap();
    let mut filled = *puzzle.cells();
    filled[0] = 5;
    let filled = Board::try_from(filled).unwrap();
    let mut masks = [CandidateMask::default(); 81];
    masks[0] = CandidateMask::try_from(1 << 5).unwrap();
    masks[1] = CandidateMask::try_from(1 << 3).unwrap();
    let stats = SearchStats {
        nodes: 2,
        guesses: 1,
        forced_moves: 1,
        max_depth: 1,
        backtracks: 1,
    };
    let cell = |index| CellIndex::try_from(index).unwrap();
    let digit = |value| Digit::try_from(value).unwrap();
    let snapshot = |board: Board, candidates, focus| {
        TraceSnapshot::new(board, candidates, stats, 1, cell(focus))
    };
    let step = |kind, board, candidates, focus| TraceEvent::Step {
        kind,
        snapshot: snapshot(board, candidates, focus),
    };
    vec![
        TraceEvent::Start {
            solver: built_in_solvers()[0].name().to_owned(),
            puzzle: puzzle.clone(),
        },
        step(TraceKind::Candidates, puzzle.clone(), Some(masks), 0),
        step(
            TraceKind::Assignment {
                cell: cell(0),
                digit: digit(5),
                forced: true,
            },
            filled.clone(),
            None,
            0,
        ),
        step(TraceKind::Restore, puzzle.clone(), None, 0),
        step(
            TraceKind::Assignment {
                cell: cell(0),
                digit: digit(5),
                forced: true,
            },
            filled,
            None,
            0,
        ),
        step(
            TraceKind::Assignment {
                cell: cell(1),
                digit: digit(3),
                forced: false,
            },
            solution.clone(),
            None,
            1,
        ),
        step(TraceKind::Solution, solution.clone(), None, 1),
        TraceEvent::Finished {
            result: SearchResult::new(
                puzzle,
                Some(solution.clone()),
                1,
                SearchExtent::Capped(NonZeroUsize::MIN),
                stats,
            )
            .unwrap(),
            snapshot: snapshot(solution, None, 1),
        },
    ]
}

#[test]
fn scripted_solver_session_steps_pauses_resets_and_replays_without_searching_again() {
    let mut app = app();
    app.set_events_per_tick(NonZeroUsize::MIN).unwrap();
    let script = scripted_trace(app.game().puzzle());
    let mut source = ScriptedSolver {
        script: script.clone(),
        ..Default::default()
    };
    let mut terminal = Terminal::new(RecordingBackend::new(100, 30)).unwrap();
    let mut actions = vec![
        key(KeyCode::Tab),
        character('i'),
        character(']'),
        character(']'),
        character(' '),
        Action::Tick,
        character(' '),
        Action::Tick,
        character(']'),
        character(']'),
        character(']'),
        character(']'),
        character(']'),
        character('['),
        key(KeyCode::End),
        character('r'),
        character(' '),
    ];
    actions.extend([Action::Tick; 8]);
    // Space at the completed event should replay without another solver run.
    actions.push(character(' '));
    actions.extend([Action::Tick; 8]);
    actions.extend([character('?'), key(KeyCode::Esc), character('q')]);
    let mut input = ScriptedInput::new(actions);
    run_session(&mut terminal, &mut app, &mut input, &mut source).unwrap();

    assert_eq!(app.mode(), Mode::Watch);
    assert_eq!(
        source.starts,
        [(built_in_solvers()[0].name(), app.game().puzzle().clone())]
    );
    assert_eq!(source.polls, script.len());
    assert_eq!(
        app.trace_history().iter().cloned().collect::<Vec<_>>(),
        script
    );
    assert_eq!(app.trace_cursor(), script.len());
    assert_eq!(
        app.board_cells(),
        *SOLUTION.parse::<Board>().unwrap().cells()
    );
    assert!(app.trace().finished().is_some());
    assert!(!app.running());
    assert!(app.game().events().is_empty());
    for phrase in [
        "SEARCH OBSERVATORY",
        "Candidates 5",
        "Forced r1 c1 = 5",
        "Restore parent state",
        "Guess r1 c2 = 3",
        "finished",
        "event 0/8",
        "KU / CONTROLS",
    ] {
        assert_frame(&terminal, phrase);
    }
    assert!(terminal.backend().frames.iter().any(|frame| {
        frame.content().iter().any(|cell| {
            cell.symbol() == "3"
                && cell.bg == Palette::default().selection_bg
                && cell.fg != Palette::default().selection_bg
        })
    }));
    let paused = &terminal.backend().frames[7];
    let paused_tick = &terminal.backend().frames[8];
    assert_eq!(
        paused, paused_tick,
        "paused Tick must not consume a solver event"
    );
    assert!(text(&terminal.backend().frames[13]).contains("Depth      1"));
    assert!(
        text(&terminal.backend().frames[15]).contains("Depth      1"),
        "End must preserve the recorded Finished depth"
    );
}

#[test]
fn a_step_requested_before_the_worker_is_ready_advances_once_on_a_later_tick() {
    let mut app = app();
    let script = scripted_trace(app.game().puzzle());
    let mut source = ScriptedSolver {
        script: script.clone(),
        unavailable_polls: 1,
        ..Default::default()
    };
    let mut terminal = Terminal::new(RecordingBackend::new(100, 30)).unwrap();
    let mut input = ScriptedInput::new([
        key(KeyCode::Tab),
        character(']'),
        Action::Tick,
        Action::Tick,
        character('q'),
    ]);
    run_session(&mut terminal, &mut app, &mut input, &mut source).unwrap();
    assert_eq!(source.starts.len(), 1);
    assert_eq!(source.polls, 2);
    assert_eq!(app.trace_cursor(), 1);
    assert_eq!(
        app.trace_history().iter().cloned().collect::<Vec<_>>(),
        script[..1]
    );
    assert!(!app.running());
    assert_eq!(
        terminal.backend().frames[3],
        terminal.backend().frames[4],
        "a pending single step must not keep running after its event arrives"
    );
}

#[test]
fn solver_picker_changes_source_and_next_puzzle_resets_both_sessions() {
    use ku_cli::tui::{
        app::{Job, Playback},
        capability::{FunctionCapability, HookEvent},
        session::{Session, TerminalOutput},
    };

    let mut app = app();
    let puzzle = app.game().puzzle().clone();
    let mut source = ScriptedSolver {
        script: scripted_trace(&puzzle),
        ..Default::default()
    };
    let mut terminal = Terminal::new(RecordingBackend::new(100, 30)).unwrap();
    let mut input = ScriptedInput::new([
        character('5'),
        key(KeyCode::Tab),
        character(']'),
        character('s'),
        key(KeyCode::Down),
        key(KeyCode::Enter),
        character(']'),
        character('n'),
        key(KeyCode::Tab),
        character('q'),
    ]);
    let mut checked_switched_run = false;
    let mut capability = FunctionCapability::new(|hook: HookEvent<'_>| {
        if let HookEvent::BeforeActivation { app, activation } = hook
            && activation
                .jobs()
                .iter()
                .any(|job| matches!(job, Job::Action(Action::NextPuzzle)))
        {
            assert_eq!(app.solver_name(), built_in_trace_solvers()[1].name());
            assert!(app.current_run().is_some());
            assert_eq!(app.watch().playback(), Playback::Paused);
            assert_eq!(app.trace_cursor(), 1);
            assert_eq!(app.trace_history().len(), 1);
            assert_eq!(
                app.trace_history().event(0),
                Some(&TraceEvent::Start {
                    solver: built_in_trace_solvers()[1].name().to_owned(),
                    puzzle: puzzle.clone(),
                })
            );
            checked_switched_run = true;
        }
        Ok(())
    });
    let mut output = TerminalOutput::new(&mut terminal);
    Session::new(
        &mut app,
        &mut input,
        &mut source,
        &mut output,
        &mut capability,
    )
    .run()
    .unwrap();
    assert!(checked_switched_run);
    assert_eq!(
        source.starts,
        [
            (built_in_solvers()[0].name(), puzzle.clone()),
            (built_in_solvers()[1].name(), puzzle)
        ]
    );
    assert_eq!(app.solver_name(), built_in_solvers()[1].name());
    assert_eq!(app.record_index(), 1);
    assert_eq!(app.mode(), Mode::Play);
    assert_eq!(app.game().board(), &Board::default());
    assert_eq!(app.game_cursor(), 0);
    assert!(app.game().events().is_empty());
    assert_eq!(app.trace_cursor(), 0);
    assert!(app.trace_history().is_empty());
    assert!(matches!(
        app.watch().run(),
        ku_cli::tui::app::RunState::NotStarted
    ));
    assert_frame(&terminal, "SOLVER REGISTRY");
    assert_frame(&terminal, "2/2");
    assert!(source.cancellations >= 3);
}

#[test]
fn normal_narrow_and_tiny_sessions_render_without_a_terminal_device() {
    for (width, height, phrase) in [
        (100, 30, "i details"),
        (37, 22, "i details"),
        (37, 36, "i details"),
        (30, 10, "Resize to at least"),
        (1, 1, "K"),
    ] {
        let mut app = app();
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        let mut input = ScriptedInput::new([character('q')]);
        let mut source = ScriptedSolver::default();
        run_session(&mut terminal, &mut app, &mut input, &mut source).unwrap();
        assert!(
            text(terminal.backend().buffer()).contains(phrase),
            "missing {phrase:?} at {width}x{height}"
        );
        assert!(app.quit());
        assert!(source.cancellations > 0);
    }
}

#[test]
fn errors_in_input_rendering_or_solver_source_cancel_the_session() {
    use ku_cli::tui::app::{FailureKind, RunState, StopReason};

    for failure in ["input", "render", "start", "poll"] {
        let mut app = app();
        let mut backend = RecordingBackend::new(100, 30);
        backend.fail_draw = failure == "render";
        let mut terminal = Terminal::new(backend).unwrap();
        let mut input = ScriptedInput::new([key(KeyCode::Tab), character(']')]);
        input.fail = failure == "input";
        let mut source = ScriptedSolver {
            fail_start: failure == "start",
            fail_poll: failure == "poll",
            ..Default::default()
        };
        let error = run_session(&mut terminal, &mut app, &mut input, &mut source).unwrap_err();
        assert!(error.to_string().contains("scripted"));
        assert!(
            source.cancellations > 0,
            "{failure} failure did not cancel producer"
        );
        assert!(
            source.pending.is_empty(),
            "{failure} failure retained source responses"
        );
        assert!(
            app.current_run().is_none(),
            "{failure} failure retained an active producer"
        );
        if matches!(failure, "start" | "poll") {
            let RunState::Stopped(StopReason::Failed(cause)) = app.watch().run() else {
                panic!("{failure} failure was not recorded in the application state");
            };
            let expected = if failure == "start" {
                FailureKind::Start
            } else {
                FailureKind::Execution
            };
            assert_eq!(cause.kind, expected);
            assert!(cause.message.contains("scripted"));
        }
    }
}

#[test]
fn worker_source_streams_real_progress_from_every_registered_solver() {
    let puzzle = records().remove(0).puzzle().clone();
    for &solver in built_in_trace_solvers() {
        let mut source = WorkerSource::default();
        source.start(solver, &puzzle).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut events = Vec::new();
        loop {
            assert!(
                Instant::now() < deadline,
                "{} worker did not finish",
                solver.name()
            );
            match source.poll_event().unwrap() {
                SourcePoll::Event(event) => {
                    let finished = matches!(event, TraceEvent::Finished { .. });
                    events.push(event);
                    if finished {
                        break;
                    }
                }
                SourcePoll::Pending => thread::yield_now(),
                SourcePoll::Closed => panic!("solver closed before finish"),
            }
        }
        source.cancel();
        assert_eq!(
            events.first(),
            Some(&TraceEvent::Start {
                solver: solver.name().to_owned(),
                puzzle: puzzle.clone()
            })
        );
        assert!(events.iter().any(|event| matches!(
            event,
            TraceEvent::Step {
                kind: TraceKind::Assignment { .. },
                ..
            }
        )));
        let Some(TraceEvent::Finished { result, snapshot }) = events.last() else {
            panic!("solver did not produce a finish event");
        };
        assert_eq!(result, &solver.search(&puzzle, NonZeroUsize::MIN));
        assert_eq!(snapshot.board(), &SOLUTION.parse::<Board>().unwrap());
    }
}

#[test]
fn function_backed_session_preserves_both_games_and_replays_without_restarting_search() {
    use ku_cli::tui::{
        app::{Job, Playback, RunState},
        capability::{FunctionCapability, HookEvent},
        session::{Session, SessionExit},
    };

    let mut app = app();
    let puzzle = app.game().puzzle().clone();
    let script = scripted_trace(&puzzle);
    let mut requested = Vec::new();
    let mut source =
        FunctionSolverSource::new(|solver: &'static dyn TraceSolver, board: &Board| {
            requested.push((solver.name(), board.clone()));
            Ok(script.iter().cloned().map(SourcePoll::Event).collect())
        });
    let mut actions = [
        character('5'),
        key(KeyCode::Right),
        Action::ToggleMode,
        Action::ToggleRun,
        Action::Tick,
        Action::HistoryStart,
        Action::StepForward,
        Action::HistoryEnd,
        Action::ToggleMode,
        character('3'),
        Action::ToggleMode,
        Action::Resize,
        Action::Quit,
    ]
    .into_iter();
    let mut input = || Ok(actions.next());
    let mut frames = Vec::new();
    let mut output = |app: &App| {
        frames.push(app.snapshot());
        Ok(())
    };
    let mut hooks = Vec::new();
    let mut capability = FunctionCapability::new(|hook: HookEvent<'_>| {
        hooks.push(match hook {
            HookEvent::SessionStart { .. } => "session start",
            HookEvent::BeforeActivation { activation, .. }
                if activation.jobs().iter().any(|job| {
                    matches!(job, Job::SolverEvents { .. } | Job::SolverCompleted { .. })
                }) =>
            {
                "solver event"
            }
            HookEvent::BeforeActivation { .. } => "before activation",
            HookEvent::AfterActivation { .. } => "after activation",
            HookEvent::BeforeCommand { .. } => "before command",
            HookEvent::AfterCommand { .. } => "after command",
            HookEvent::BeforeRender { .. } => "before render",
            HookEvent::AfterRender { .. } => "after render",
            HookEvent::Error { .. } => "error",
            HookEvent::SessionEnd { result, .. } => {
                assert_eq!(result.as_ref().unwrap(), &SessionExit::Quit);
                "session end"
            }
        });
        Ok(())
    });
    let exit = Session::new(
        &mut app,
        &mut input,
        &mut source,
        &mut output,
        &mut capability,
    )
    .run()
    .unwrap();
    assert_eq!(exit, SessionExit::Quit);
    assert_eq!(source.starts(), 1);
    assert_eq!(source.cancellations(), 1);
    assert_eq!(requested, [(built_in_trace_solvers()[0].name(), puzzle)]);
    assert!(app.game().is_complete());
    assert_eq!(app.game().events().len(), 2);
    assert_eq!(app.mode(), Mode::Watch);
    assert_eq!(app.trace_cursor(), script.len());
    assert_eq!(app.selected(), CellIndex::try_from(1).unwrap());
    assert_eq!(app.watch().playback(), Playback::Paused);
    assert!(matches!(app.watch().run(), RunState::Finished(_)));
    assert_eq!(hooks.first(), Some(&"session start"));
    assert_eq!(hooks.last(), Some(&"session end"));
    assert!(!hooks.contains(&"error"));
    assert!(hooks.contains(&"before command"));
    // A redraw request cannot consume a trace event or change its focused cell.
    let before_resize = &frames[11];
    let after_resize = &frames[12];
    assert_eq!(before_resize, after_resize);
    assert!(frames.iter().any(|frame| frame.mode == Mode::Play
        && frame.selected == CellIndex::try_from(1).unwrap()
        && frame.play_events == 1));
    assert!(
        hooks.contains(&"solver event"),
        "worker feedback must reenter ordinary activation hooks"
    );
}

#[test]
fn single_event_dispatch_and_full_input_loop_produce_identical_checkpoints() {
    use ku_cli::tui::{
        capability::NoCapability,
        session::{Session, SessionExit},
    };

    let actions = [
        Action::ToggleMode,
        Action::StepForward,
        Action::StepForward,
        Action::ToggleRun,
        Action::Tick,
        Action::HistoryStart,
        Action::HistoryEnd,
        Action::Quit,
    ];
    let run = |manual: bool| {
        let mut app = app();
        let script = scripted_trace(app.game().puzzle());
        let mut source = FunctionSolverSource::new(|_: &'static dyn TraceSolver, _: &Board| {
            Ok(script.iter().cloned().map(SourcePoll::Event).collect())
        });
        let mut stream = actions.into_iter();
        let mut input = || Ok(stream.next());
        let mut checkpoints = Vec::new();
        let mut output = |app: &App| {
            checkpoints.push(app.snapshot());
            Ok(())
        };
        let mut hooks = NoCapability;
        let mut session = Session::new(&mut app, &mut input, &mut source, &mut output, &mut hooks);
        if manual {
            session.present().unwrap();
            for action in actions {
                session.dispatch(action.into()).unwrap();
                session.present().unwrap();
            }
            assert_eq!(session.close(SessionExit::Quit).unwrap(), SessionExit::Quit);
        } else {
            assert_eq!(session.run().unwrap(), SessionExit::Quit);
        }
        drop(session);
        (
            app.snapshot(),
            checkpoints,
            source.starts(),
            source.cancellations(),
        )
    };
    assert_eq!(run(false), run(true));
}

#[test]
fn command_capability_can_replace_the_entire_solver_without_calling_its_source() {
    use ku_cli::tui::{
        app::{Command, CommandKind, Job},
        capability::SessionCapability,
        session::{Session, SessionExit},
    };

    #[derive(Default)]
    struct UnusedSource {
        cancellations: usize,
    }
    impl SolverEventSource for UnusedSource {
        fn start(&mut self, _: &'static dyn TraceSolver, _: &Board) -> io::Result<()> {
            panic!("virtual capability must replace start")
        }
        fn poll_event(&mut self) -> io::Result<SourcePoll> {
            panic!("virtual capability must replace polling")
        }
        fn cancel(&mut self) {
            self.cancellations += 1;
        }
    }
    struct VirtualSearch {
        events: VecDeque<TraceEvent>,
        starts: usize,
        emitted_events: usize,
    }
    impl SessionCapability for VirtualSearch {
        fn wrap_command(
            &mut self,
            _: &App,
            command: &Command,
            handler: &mut dyn FnMut(&Command) -> io::Result<Vec<Job>>,
        ) -> io::Result<Vec<Job>> {
            match &command.kind {
                CommandKind::StartSolver { run, .. } => {
                    self.starts += 1;
                    let mut jobs = vec![Job::SolverStarted {
                        run: *run,
                        command: command.id,
                    }];
                    for event in self.events.drain(..) {
                        self.emitted_events += 1;
                        jobs.push(match event {
                            TraceEvent::Finished { result, snapshot } => Job::SolverCompleted {
                                run: *run,
                                command: command.id,
                                result,
                                snapshot,
                            },
                            event => Job::SolverEvents {
                                run: *run,
                                command: command.id,
                                events: vec![event],
                            },
                        });
                    }
                    Ok(jobs)
                }
                CommandKind::CancelSolver { .. } | CommandKind::ScheduleTimer { .. } => {
                    handler(command)
                }
            }
        }
    }

    let mut app = app();
    let script = scripted_trace(app.game().puzzle());
    let expected_events = script.len();
    let mut capability = VirtualSearch {
        events: script.into(),
        starts: 0,
        emitted_events: 0,
    };
    let mut source = UnusedSource::default();
    let mut actions = [
        Action::ToggleMode,
        Action::ToggleRun,
        Action::Tick,
        Action::Quit,
    ]
    .into_iter();
    let mut input = || Ok(actions.next());
    let mut output = |_: &App| Ok(());
    assert_eq!(
        Session::new(
            &mut app,
            &mut input,
            &mut source,
            &mut output,
            &mut capability
        )
        .run()
        .unwrap(),
        SessionExit::Quit
    );
    assert_eq!(capability.starts, 1);
    assert_eq!(capability.emitted_events, expected_events);
    assert!(capability.events.is_empty());
    assert!(app.trace().finished().is_some());
    assert!(source.cancellations > 0);
}

#[test]
fn capability_errors_report_failure_and_cancel_a_partially_consumed_function_stream() {
    use ku_cli::tui::{
        app::{Playback, RunState, StopReason},
        capability::{FunctionCapability, HookEvent},
        session::Session,
    };

    let mut app = app();
    let script = scripted_trace(app.game().puzzle());
    let mut source = FunctionSolverSource::new(|_: &'static dyn TraceSolver, _: &Board| {
        Ok(script.iter().cloned().map(SourcePoll::Event).collect())
    });
    let mut actions = [Action::ToggleMode, Action::StepForward].into_iter();
    let mut input = || Ok(actions.next());
    let mut output = |_: &App| Ok(());
    let mut observed = Vec::new();
    let mut capability = FunctionCapability::new(|hook: HookEvent<'_>| {
        match hook {
            HookEvent::BeforeRender { app } if app.trace_cursor() == 1 => {
                return Err(io::Error::other("scripted capability failure"));
            }
            HookEvent::Error { error, .. } => {
                assert!(error.to_string().contains("capability failure"));
                observed.push("error");
            }
            HookEvent::SessionEnd { app, result } => {
                assert!(result.is_err());
                assert_eq!(app.watch().run(), &RunState::Stopped(StopReason::Cancelled));
                assert_eq!(app.watch().playback(), Playback::Paused);
                observed.push("cleanup");
            }
            _ => {}
        }
        Ok(())
    });
    let error = Session::new(
        &mut app,
        &mut input,
        &mut source,
        &mut output,
        &mut capability,
    )
    .run()
    .unwrap_err();
    assert!(error.to_string().contains("capability failure"));
    assert_eq!(observed, ["error", "cleanup"]);
    assert_eq!(source.starts(), 1);
    assert_eq!(source.cancellations(), 1);
    assert_eq!(source.poll_event().unwrap(), SourcePoll::Closed);
    assert_eq!(app.trace_cursor(), 1);
    assert_eq!(app.trace_history().len(), 1);
    assert_eq!(app.watch().run(), &RunState::Stopped(StopReason::Cancelled));
    assert_eq!(app.watch().playback(), Playback::Paused);
}
