//! Terminal setup and restoration, kept outside the application model.

use std::io::{self, IsTerminal};

/// Runs a session and restores the terminal on errors, normal exit, or panic.
pub fn with_terminal<F, T>(run: F) -> io::Result<T>
where
    F: FnOnce(&mut ratatui::DefaultTerminal) -> io::Result<T>,
{
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "the TUI requires an interactive terminal on stdin and stdout",
        ));
    }
    // Install cleanup before initialization: setup can fail after raw mode or
    // the alternate screen has already been enabled.
    let mut guard = RestoreGuard(true);
    let mut terminal = ratatui::try_init()?;
    let result = run(&mut terminal);
    let restored = ratatui::try_restore();
    if restored.is_ok() {
        guard.0 = false;
    }
    match result {
        Err(error) => Err(error),
        Ok(value) => restored.map(|()| value),
    }
}

struct RestoreGuard(bool);

impl Drop for RestoreGuard {
    fn drop(&mut self) {
        if self.0 {
            // The Ratatui panic hook also restores before printing the panic.
            // A second attempt here covers initialization errors and unwind.
            ratatui::restore();
        }
    }
}
