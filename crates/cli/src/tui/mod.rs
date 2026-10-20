//! Play Sudoku and inspect live solver work, with deterministic session seams.

use std::{io, time::Duration};

use ratatui::{Terminal, backend::Backend};

pub mod app;
pub mod capability;
pub mod input;
pub mod render;
pub mod session;
pub mod simulation;
pub mod source;
pub mod splash;
pub mod terminal;
pub mod theme;

use app::App;
use capability::NoCapability;
use input::{CrosstermEvents, CrosstermInput, InputSource, MonotonicClock};
use session::{Session, TerminalOutput};
use source::{SolverEventSource, WorkerSource};

/// Runs the ordinary session driver for a live terminal or a test backend.
/// All exits cancel the producer, including rendering, hook, and input errors.
pub fn run_session<B: Backend>(
    terminal: &mut Terminal<B>,
    app: &mut App,
    input: &mut impl InputSource,
    source: &mut impl SolverEventSource,
) -> io::Result<()> {
    Session::new(
        app,
        input,
        source,
        &mut TerminalOutput::new(terminal),
        &mut NoCapability,
    )
    .run()
    .map(|_| ())
}

/// Launches an interactive session, restoring terminal state on all exits.
pub fn run(mut app: App) -> io::Result<()> {
    terminal::with_terminal(|terminal| {
        splash::show_with_palette(
            terminal,
            &MonotonicClock::default(),
            &mut CrosstermEvents,
            theme::Palette::for_app(&app),
        )?;
        let mut input = CrosstermInput::new(
            MonotonicClock::default(),
            CrosstermEvents,
            Duration::from_millis(app.config().tick_interval_ms()),
        )?;
        run_session(terminal, &mut app, &mut input, &mut WorkerSource::default())
    })
}
