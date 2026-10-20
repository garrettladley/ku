//! User intent independent of terminal transport.
use ku_core::Digit;
/// Semantic user input. Tick and Resize are transport compatibility signals.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// Moves the player selection, navigates a picker, or steps watch history.
    Move {
        /// Signed row offset; positive values move down.
        rows: i8,
        /// Signed column offset; positive values move right.
        columns: i8,
    },
    /// Enters a digit in the selected editable cell in play mode.
    Digit(Digit),
    /// Clears the selected editable cell in play mode.
    Clear,
    /// Switches between play and solver watch mode.
    ToggleMode,
    /// Shows or hides the details panel.
    ToggleSidebar,
    /// Starts or pauses trace playback.
    ToggleRun,
    /// Doubles playback speed, up to 128 events per tick.
    Faster,
    /// Halves playback speed, down to one event per tick.
    Slower,
    /// Advances the visible history by one event, requesting solver work if needed.
    StepForward,
    /// Rewinds the visible history by one event.
    StepBack,
    /// Seeks to the beginning of the visible history.
    HistoryStart,
    /// Seeks to the end of the retained visible history.
    HistoryEnd,
    /// Rewinds the visible history to its initial puzzle state, retaining all events.
    Reset,
    /// Opens the solver picker.
    ChooseSolver,
    /// Opens the theme picker.
    ChooseTheme,
    /// Confirms the current picker selection.
    Accept,
    /// Dismisses help or a picker; otherwise hides the details panel.
    Escape,
    /// Selects the next puzzle, wrapping at the end of the bank.
    NextPuzzle,
    /// Shows or dismisses help while preserving the underlying picker.
    Help,
    /// Requests session exit.
    Quit,
    /// Signals a transport tick; live adapters normalize it to an identified timer.
    Tick,
    /// Requests a redraw after a terminal resize.
    Resize,
}
