//! Full virtual session: authored solver outcomes, real hooks, output and replay.
use ku_cli::tui::{
    app::{Command, CommandKind, Job, LogicalTime},
    capability::NoCapability,
    input::Action,
    session::{Session, SessionExit},
    simulation::{FunctionCommands, RecordingOutput, ScriptedInput, SessionRecording, simulate},
    source::WorkerSource,
};
use ku_core::{
    Board, CellIndex, Digit, SearchExtent, SearchResult, SearchStats, TraceEvent, TraceKind,
    TraceSnapshot,
};

use ku_session::{
    history::{History, ReplayMode},
    simulation::{ScheduledJob, SimulationError},
};
const SOLUTION: &str =
    "534678912672195348198342567859761423426853791713924856961537284287419635345286179";
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut cells = *SOLUTION.parse::<Board>()?.cells();
    cells[0] = 0;
    let puzzle = Board::try_from(cells)?;
    let mut app =
        ku_cli::tui::app::new(vec![puzzle.into()], 0, "virtual session".into(), "bitmask")?;
    let starts = std::cell::Cell::new(0);
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
            let solution: Board = SOLUTION
                .parse::<Board>()
                .map_err(|error| SimulationError::Executor(error.to_string()))?;
            let stats = SearchStats {
                nodes: 1,
                forced_moves: 1,
                max_depth: 1,
                ..Default::default()
            };
            let result = SearchResult::new(
                puzzle,
                Some(solution.clone()),
                1,
                SearchExtent::Exhausted,
                stats,
            )
            .map_err(|error| SimulationError::Executor(error.to_string()))?;
            let snapshot = TraceSnapshot::new(solution, None, stats, 1, CellIndex::FIRST);
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
                        events: vec![
                            TraceEvent::Start {
                                solver: solver.to_string(),
                                puzzle: puzzle.clone(),
                            },
                            TraceEvent::Step {
                                kind: TraceKind::Assignment {
                                    cell: CellIndex::FIRST,
                                    digit: Digit::try_from(5).map_err(|error| {
                                        SimulationError::Executor(error.to_string())
                                    })?,
                                    forced: true,
                                },
                                snapshot: snapshot.clone(),
                            },
                        ],
                    },
                ),
                ScheduledJob::new(
                    now.checked_add(20)?,
                    Job::SolverCompleted {
                        run: *run,
                        command: command.id,
                        result,
                        snapshot,
                    },
                ),
            ])
        },
    );
    let scheduler = commands.scheduler();
    scheduler
        .borrow_mut()
        .schedule_at(LogicalTime::ZERO, Action::ToggleMode.into())?;
    scheduler
        .borrow_mut()
        .schedule_at(LogicalTime::ZERO, Action::ToggleRun.into())?;
    scheduler
        .borrow_mut()
        .schedule_at(LogicalTime::from_millis(100), Action::HistoryStart.into())?;
    scheduler
        .borrow_mut()
        .schedule_at(LogicalTime::from_millis(100), Action::StepForward.into())?;
    scheduler
        .borrow_mut()
        .schedule_at(LogicalTime::from_millis(110), Action::HistoryEnd.into())?;
    scheduler
        .borrow_mut()
        .schedule_at(LogicalTime::from_millis(120), Action::Quit.into())?;
    let mut input = ScriptedInput::default();
    let mut source = WorkerSource::default();
    let mut output = RecordingOutput::default();
    let mut hooks = (commands, (NoCapability, SessionRecording::default()));
    {
        let mut session = Session::new(&mut app, &mut input, &mut source, &mut output, &mut hooks);
        simulate(&mut session, &scheduler, 100)?;
        session.close(SessionExit::Quit)?;
    }
    assert_eq!(starts.get(), 1);
    assert!(app.trace().finished().is_some());
    let history = hooks.1.1.history().ok_or("missing recording")?;
    let json = history.to_json()?;
    if let Some(filename) = std::env::args().nth(1) {
        std::fs::write(filename, &json)?;
    }
    let decoded = History::from_json(&json)?;
    assert_eq!(
        decoded.replay(ReplayMode::Strict)?.snapshot(),
        app.snapshot()
    );
    println!(
        "Simulated {} activations and {} frames through the live driver; JSON replay verified. No solver search, threads, or sleeps.",
        history.records().len(),
        output.frames().len()
    );
    Ok(())
}
