//! Render board-first play and optional details through a recorded virtual session.
use std::io;

use anyhow::{Context, Result, ensure};
use ku_cli::tui::{
    app::{App, Command, LogicalTime},
    input::Action,
    session::{Session, SessionExit, SessionOutput, TerminalOutput},
    simulation::{FunctionCommands, ScriptedInput, SessionRecording, simulate},
    source::FunctionSolverSource,
    theme,
};
use ku_core::{Board, TraceSolver};

use ku_session::{history::ReplayMode, simulation::ScheduledJob};
use ratatui::{Terminal, backend::TestBackend};

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let width = args.next().map_or(Ok(90), |value| value.parse::<u16>())?;
    let height = args.next().map_or(Ok(26), |value| value.parse::<u16>())?;
    let theme_name = args.next().unwrap_or_else(|| "ku".into());
    ensure!(
        width > 0 && height > 0,
        "preview dimensions must be nonzero"
    );
    let puzzle: Board =
        "530070000600195000098000060800060003400803001700020006060000280000419005000080079"
            .parse()?;
    let app = ku_cli::tui::app::new(vec![puzzle.into()], 0, "scripted puzzle".into(), "bitmask")?;
    let mut app = App::from_config(theme::configure(app.config().clone(), &theme_name)?);
    let commands =
        FunctionCommands::new(|_: &Command, _: LogicalTime| Ok(Vec::<ScheduledJob>::new()));
    let scheduler = commands.scheduler();
    for (time, action) in [
        (
            10,
            Action::Move {
                rows: 0,
                columns: -2,
            },
        ),
        (20, Action::ToggleSidebar),
        (30, Action::Escape),
        (40, Action::Quit),
    ] {
        scheduler
            .borrow_mut()
            .schedule_at(LogicalTime::from_millis(time), action.into())?;
    }
    let mut input = ScriptedInput::default();
    let mut source = FunctionSolverSource::new(|_: &'static dyn TraceSolver, _: &Board| {
        Err(io::Error::other("play preview must not run a solver"))
    });
    let mut terminal = Terminal::new(TestBackend::new(width, height))?;
    let mut screens = Vec::new();
    let mut hooks = (commands, SessionRecording::default());
    {
        let mut output = |app: &App| -> io::Result<()> {
            TerminalOutput::new(&mut terminal).present(app)?;
            let buffer = terminal.backend().buffer();
            let screen = (0..height)
                .map(|y| {
                    (0..width)
                        .map(|x| buffer[(x, y)].symbol())
                        .collect::<String>()
                        .trim_end()
                        .to_owned()
                })
                .collect::<Vec<_>>()
                .join("\n");
            screens.push(screen);
            Ok(())
        };
        let mut session = Session::new(&mut app, &mut input, &mut source, &mut output, &mut hooks);
        simulate(&mut session, &scheduler, 10)?;
        session.close(SessionExit::Quit)?;
    }
    let history = hooks.1.history().context("missing session recording")?;
    ensure!(
        history.replay(ReplayMode::Strict)?.snapshot() == app.snapshot(),
        "replay diverged"
    );
    ensure!(source.starts() == 0, "unexpected solver execution");
    for (label, screen) in ["BOARD / DEFAULT", "SELECTED GIVEN", "DETAILS / OPEN"]
        .into_iter()
        .zip(screens.iter())
    {
        println!("\n{label}\n{screen}");
    }
    println!(
        "\n{} recorded activations; strict replay verified; no search, threads, or sleeps.",
        history.records().len()
    );
    Ok(())
}
