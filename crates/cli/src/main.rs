//! Process entry point for the Ku Sudoku command-line application.

use std::{
    io::{self, Write},
    process::ExitCode,
};

fn main() -> ExitCode {
    match ku_cli::run_cli() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error)
            if error
                .downcast_ref::<io::Error>()
                .is_some_and(|error| error.kind() == io::ErrorKind::BrokenPipe) =>
        {
            ExitCode::SUCCESS
        }
        Err(error) => {
            // Reporting a failure must not panic if stderr is also closed.
            let _ = writeln!(io::stderr().lock(), "error: {error:#}");
            ExitCode::FAILURE
        }
    }
}
