//! Replay a saved session in a fresh process without external execution.
use std::{env, fs, io};

use ku_session::history::{History, ReplayMode};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let filename = env::args().nth(1).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "usage: replay HISTORY.json [--semantic]",
        )
    })?;
    let mode = if env::args().any(|argument| argument == "--semantic") {
        ReplayMode::Semantic
    } else {
        ReplayMode::Strict
    };
    let history = History::from_json(&fs::read_to_string(filename)?)?;
    let app = history.replay(mode)?;
    println!(
        "Replayed {} activations at {} ms: {} trace events, solver {}, closed {}.",
        history.records().len(),
        app.logical_time().as_millis(),
        app.trace_history().len(),
        app.solver_name(),
        app.snapshot().closed
    );
    Ok(())
}
