//! Deterministic application state driven by portable activation jobs.
use std::num::NonZeroUsize;

use ku_core::{
    Board, CellIndex, Digit, Game, GameError, HistoryError, HistoryLimits, SearchResult,
    TraceEvent, TraceHistory, TraceState, dataset::Record,
};

use crate::{
    action::Action,
    protocol::{
        Activation, ActivationId, Command, CommandId, CommandKind, Completion, Failure,
        FailureKind, Job, LogicalTime, ProtocolError, RunId, SolverId,
    },
};

/// The visible session; switching tabs retains both histories.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Edit the puzzle and navigate accepted player moves.
    Play,
    /// Inspect recorded solver events and control playback.
    Watch,
}
/// Producer lifecycle, independent of playback and displayed history.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RunState {
    /// No producer has been requested for the current puzzle and strategy.
    NotStarted,
    /// A start command was emitted and is awaiting acknowledgment.
    Starting(RunId),
    /// A producer is delivering events for this run.
    Active(RunId),
    /// A validated search completion was received.
    Finished(SearchResult),
    /// The run stopped before a validated completion.
    Stopped(StopReason),
}
/// Why a producer stopped before a validated completion.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StopReason {
    /// The configured event or byte budget was reached.
    HistoryLimit,
    /// History storage could not be allocated.
    StorageFailure,
    /// Producer events violated the trace contract; contains the diagnostic.
    InvalidTrace(String),
    /// Infrastructure reported a categorized failure.
    Failed(Failure),
    /// The producer was cancelled.
    Cancelled,
}
/// A single step stays pending until an event arrives or the user pauses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Playback {
    /// Playback is paused with no pending step.
    Paused,
    /// Playback advances on timers and incoming producer events.
    Playing,
    /// One step is waiting for the next producer event.
    AwaitingStep,
}
/// The persistent overlay beneath any temporary help screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BaseOverlay {
    /// No picker is open.
    None,
    /// Solver picker with a zero-based highlighted catalog entry.
    SolverPicker {
        /// Zero-based highlighted strategy index.
        index: usize,
    },
    /// Theme picker with a zero-based highlighted catalog entry.
    ThemePicker {
        /// Zero-based highlighted theme index.
        index: usize,
    },
}
/// Help temporarily covers a picker without losing its selection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OverlayState {
    /// The current picker state without a help screen.
    Base(BaseOverlay),
    /// Help covering a saved picker state.
    Help {
        /// Picker state to restore when help is dismissed.
        return_to: BaseOverlay,
    },
}
/// Presentation severity of a session notice.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    /// An informational status message.
    Info,
    /// An error requiring user attention.
    Error,
}
/// A message and severity form one value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Notice {
    severity: Severity,
    message: String,
}
impl Notice {
    fn info(message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Info,
            message: message.into(),
        }
    }
    fn error(message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Error,
            message: message.into(),
        }
    }
    /// Returns the presentation severity.
    pub fn severity(&self) -> Severity {
        self.severity
    }
    /// Borrows the human-readable message.
    pub fn message(&self) -> &str {
        &self.message
    }
}
/// Portable strategy metadata; implementation resolution belongs to infrastructure.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SolverDescriptor {
    /// Stable identity resolved by the runtime adapter.
    pub id: SolverId,
    /// Whether the strategy supports trace-producing search.
    pub tracing: bool,
}
impl SolverDescriptor {
    /// Borrows the stable solver identity as text.
    pub fn name(&self) -> &str {
        self.id.as_str()
    }
}
/// A validated nonempty, uniquely named strategy catalog.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SolverCatalog(Box<[SolverDescriptor]>);
impl TryFrom<Vec<SolverDescriptor>> for SolverCatalog {
    type Error = AppError;
    fn try_from(entries: Vec<SolverDescriptor>) -> Result<Self, Self::Error> {
        if entries.is_empty()
            || entries
                .iter()
                .enumerate()
                .any(|(index, entry)| entries[..index].iter().any(|other| other.id == entry.id))
        {
            return Err(AppError::Catalog);
        }
        Ok(Self(entries.into_boxed_slice()))
    }
}
impl AsRef<[SolverDescriptor]> for SolverCatalog {
    fn as_ref(&self) -> &[SolverDescriptor] {
        &self.0
    }
}
/// Terminal-independent theme identity and display label.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ThemeDescriptor {
    /// Portable ASCII identity using letters, digits, hyphens, or underscores.
    pub id: String,
    /// Nonempty human-readable label for a theme picker.
    pub label: String,
}
/// A nonempty theme catalog with unique portable identities and visible labels.
///
/// # Examples
///
/// ```
/// use ku_session::app::{ThemeCatalog, ThemeDescriptor};
///
/// let catalog = ThemeCatalog::try_from(vec![ThemeDescriptor {
///     id: "custom-light".into(), label: "Custom Light".into(),
/// }])?;
/// assert_eq!(catalog.as_ref()[0].id, "custom-light");
/// assert!(ThemeCatalog::try_from(vec![ThemeDescriptor {
///     id: "contains spaces".into(), label: "Invalid".into(),
/// }]).is_err());
/// # Ok::<(), ku_session::app::AppError>(())
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ThemeCatalog(Box<[ThemeDescriptor]>);
impl TryFrom<Vec<ThemeDescriptor>> for ThemeCatalog {
    type Error = AppError;
    fn try_from(entries: Vec<ThemeDescriptor>) -> Result<Self, Self::Error> {
        if entries.is_empty()
            || entries.iter().enumerate().any(|(index, entry)| {
                entry.id.is_empty()
                    || !entry
                        .id
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
                    || entry.label.trim().is_empty()
                    || entries[..index].iter().any(|other| other.id == entry.id)
            })
        {
            return Err(AppError::Themes);
        }
        Ok(Self(entries.into_boxed_slice()))
    }
}
impl Default for ThemeCatalog {
    fn default() -> Self {
        Self(
            vec![ThemeDescriptor {
                id: "ku".into(),
                label: "Ku".into(),
            }]
            .into_boxed_slice(),
        )
    }
}
impl AsRef<[ThemeDescriptor]> for ThemeCatalog {
    fn as_ref(&self) -> &[ThemeDescriptor] {
        &self.0
    }
}
/// Invalid genesis data or rejected game-history navigation.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum AppError {
    /// The initial puzzle selection was outside the bank.
    #[error("puzzle record {index} is outside {len} records")]
    Record {
        /// Rejected zero-based puzzle index.
        index: usize,
        /// Number of available records.
        len: usize,
    },
    /// The solver catalog was empty or contained duplicate identities.
    #[error("solver catalog must be nonempty with unique identities")]
    Catalog,
    /// The requested solver was absent from the catalog.
    #[error("unknown solver {0}")]
    UnknownSolver(String),
    /// Theme identities, labels, or catalog uniqueness were invalid.
    #[error(
        "theme catalog must be nonempty with unique ASCII letter/digit/hyphen/underscore identities and nonempty labels"
    )]
    Themes,
    /// The requested theme was absent from the catalog.
    #[error("unknown theme {0}")]
    UnknownTheme(String),
    /// Playback speed exceeded 128 events per tick.
    #[error("speed must be in 1..=128 events per tick")]
    Speed,
    /// A genesis-only setting was changed after activation began.
    #[error("genesis settings cannot change after an activation")]
    AlreadyActivated,
    /// The timer interval was zero.
    #[error("tick interval must be nonzero")]
    TickInterval,
    /// A requested player-history cursor exceeded the retained edits.
    #[error("game cursor {cursor} exceeds history length {len}")]
    GameCursor {
        /// Requested number of applied player edits.
        cursor: usize,
        /// Number of retained player edits.
        len: usize,
    },
    /// A player move or history event violated game rules.
    #[error(transparent)]
    Game(#[from] GameError),
}
/// Complete immutable genesis for reconstructing a session in another process.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppConfig {
    records: Box<[Record]>,
    record_index: usize,
    label: String,
    solver: SolverId,
    solver_index: usize,
    catalog: SolverCatalog,
    history_limits: HistoryLimits,
    initial_mode: Mode,
    events_per_tick: NonZeroUsize,
    tick_interval_ms: u64,
    themes: ThemeCatalog,
    initial_theme_index: usize,
}
impl AppConfig {
    /// Validates the complete genesis without resolving or executing a strategy.
    ///
    /// # Errors
    /// Rejects an empty puzzle bank, invalid selection, unknown strategy, speeds
    /// above 128 events per tick, and a zero timer interval.
    #[expect(
        clippy::too_many_arguments,
        reason = "genesis explicitly validates every independent session setting"
    )]
    pub fn new(
        records: Vec<Record>,
        record_index: usize,
        label: String,
        solver: &str,
        catalog: SolverCatalog,
        history_limits: HistoryLimits,
        initial_mode: Mode,
        events_per_tick: NonZeroUsize,
        tick_interval_ms: u64,
    ) -> Result<Self, AppError> {
        if record_index >= records.len() {
            return Err(AppError::Record {
                index: record_index,
                len: records.len(),
            });
        }
        let solver_index = catalog
            .0
            .iter()
            .position(|entry| entry.name() == solver)
            .ok_or_else(|| AppError::UnknownSolver(solver.to_owned()))?;
        if events_per_tick.get() > 128 {
            return Err(AppError::Speed);
        }
        if tick_interval_ms == 0 {
            return Err(AppError::TickInterval);
        }
        let solver = catalog.0[solver_index].id.clone();
        Ok(Self {
            records: records.into_boxed_slice(),
            record_index,
            label,
            solver,
            solver_index,
            catalog,
            history_limits,
            initial_mode,
            events_per_tick,
            tick_interval_ms,
            themes: ThemeCatalog::default(),
            initial_theme_index: 0,
        })
    }
    /// Borrows the nonempty initial puzzle bank.
    pub fn records(&self) -> &[Record] {
        &self.records
    }
    /// Returns the zero-based initial puzzle selection.
    pub fn record_index(&self) -> usize {
        self.record_index
    }
    /// Borrows the display label for the puzzle source.
    pub fn label(&self) -> &str {
        &self.label
    }
    /// Borrows the initially selected strategy identity.
    pub fn solver(&self) -> &SolverId {
        &self.solver
    }
    /// Borrows the validated strategy catalog.
    pub fn catalog(&self) -> &SolverCatalog {
        &self.catalog
    }
    /// Returns the per-run trace storage budgets.
    pub fn history_limits(&self) -> HistoryLimits {
        self.history_limits
    }
    /// Returns the initial play or watch mode.
    pub fn initial_mode(&self) -> Mode {
        self.initial_mode
    }
    /// Returns the initial playback quota per timer tick.
    pub fn events_per_tick(&self) -> NonZeroUsize {
        self.events_per_tick
    }
    /// Returns the nonzero timer interval in milliseconds.
    pub fn tick_interval_ms(&self) -> u64 {
        self.tick_interval_ms
    }
    /// Supplies portable theme metadata and a validated initial selection.
    ///
    /// # Errors
    /// Returns [`AppError::UnknownTheme`] when the identity is absent from the catalog.
    pub fn with_themes(
        mut self,
        catalog: ThemeCatalog,
        initial_theme: &str,
    ) -> Result<Self, AppError> {
        let index = catalog
            .0
            .iter()
            .position(|entry| entry.id == initial_theme)
            .ok_or_else(|| AppError::UnknownTheme(initial_theme.to_owned()))?;
        self.themes = catalog;
        self.initial_theme_index = index;
        Ok(self)
    }
    /// Borrows the portable theme catalog.
    pub fn themes(&self) -> &ThemeCatalog {
        &self.themes
    }
    /// Borrows the initial theme identity.
    pub fn initial_theme(&self) -> &str {
        &self.themes.0[self.initial_theme_index].id
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
struct PuzzleBank {
    records: Box<[Record]>,
    index: usize,
    label: String,
}
impl PuzzleBank {
    fn puzzle(&self) -> &Board {
        self.records[self.index].puzzle()
    }
    fn next(&mut self) {
        self.index = (self.index + 1) % self.records.len();
    }
}
/// Authoritative accepted game edits and an independent read-only history view.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlaySession {
    game: Game,
    cursor: usize,
    view: Board,
    selected: CellIndex,
}
impl PlaySession {
    fn new(puzzle: Board) -> Self {
        Self {
            selected: initial_cell(&puzzle),
            game: Game::new(puzzle.clone()),
            cursor: 0,
            view: puzzle,
        }
    }
    fn seek(&mut self, cursor: usize) -> Result<(), AppError> {
        let events = self
            .game
            .events()
            .get(..cursor)
            .ok_or(AppError::GameCursor {
                cursor,
                len: self.game.events().len(),
            })?;
        let replayed = Game::replay(self.game.puzzle().clone(), events)?;
        self.cursor = cursor;
        self.view = replayed.board().clone();
        Ok(())
    }
    /// Borrows the authoritative game, including edits beyond the view cursor.
    pub fn game(&self) -> &Game {
        &self.game
    }
    /// Returns the number of accepted player edits applied to the view.
    pub fn cursor(&self) -> usize {
        self.cursor
    }
    /// Borrows the player board at the view cursor.
    pub fn board(&self) -> &Board {
        &self.view
    }
    /// Returns the selected player cell.
    pub fn selected(&self) -> CellIndex {
        self.selected
    }
}
/// Producer lifecycle and playback intent remain orthogonal to recorded history.
#[derive(Clone, Debug)]
pub struct WatchSession {
    solver_index: usize,
    history: TraceHistory,
    run: RunState,
    playback: Playback,
    events_per_tick: NonZeroUsize,
    remaining: usize,
}
impl WatchSession {
    fn new(
        puzzle: Board,
        solver_index: usize,
        solver: &SolverDescriptor,
        limits: HistoryLimits,
        events_per_tick: NonZeroUsize,
    ) -> Self {
        Self {
            solver_index,
            history: TraceHistory::new(puzzle, solver.name().to_owned(), limits),
            run: RunState::NotStarted,
            playback: Playback::Paused,
            events_per_tick,
            remaining: 0,
        }
    }
    fn pause(&mut self) {
        self.playback = Playback::Paused;
        self.remaining = 0;
    }
    /// Borrows the retained trace and its independent replay cursor.
    pub fn history(&self) -> &TraceHistory {
        &self.history
    }
    /// Borrows the producer lifecycle, independently of playback intent.
    pub fn run(&self) -> &RunState {
        &self.run
    }
    /// Returns the current playback intent.
    pub fn playback(&self) -> Playback {
        self.playback
    }
    /// Returns the playback quota per timer tick.
    pub fn events_per_tick(&self) -> usize {
        self.events_per_tick.get()
    }
}
/// Compact observable and future-affecting state at an activation boundary.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppSnapshot {
    /// The visible session tab.
    pub mode: Mode,
    /// Whether the optional details panel is open.
    pub sidebar_open: bool,
    /// The selected portable theme identity.
    pub theme: String,
    /// The board displayed in the visible mode.
    pub board: Board,
    /// The selected or focused cell in the visible mode.
    pub selected: CellIndex,
    /// The authoritative player board after all accepted edits.
    pub play_board: Board,
    /// The player selection, preserved while watching a solver.
    pub play_selected: CellIndex,
    /// The number of player edits applied to the history view.
    pub play_cursor: usize,
    /// The total number of accepted player edits.
    pub play_events: usize,
    /// The number of trace events applied to the watch view.
    pub trace_cursor: usize,
    /// The total number of retained trace events.
    pub trace_events: usize,
    /// The observable solver state at the trace cursor.
    pub trace: TraceState,
    /// The producer lifecycle, independently of playback.
    pub run: RunState,
    /// The current playback intent.
    pub playback: Playback,
    /// The picker and temporary help state.
    pub overlay: OverlayState,
    /// The current status message and severity.
    pub notice: Notice,
    /// The zero-based selected puzzle record.
    pub record_index: usize,
    /// The selected strategy identity.
    pub solver: SolverId,
    /// The playback quota per timer tick.
    pub events_per_tick: usize,
    /// The unconsumed playback quota awaiting producer feedback.
    pub remaining: usize,
    /// The last allocated run counter; incremented for the next run.
    pub next_run: u64,
    /// The current trace storage budgets.
    pub history_limits: HistoryLimits,
    /// Whether exit was requested.
    pub quit: bool,
    /// Whether a session-end job closed the model.
    pub closed: bool,
    /// The last accepted activation, if any.
    pub last_activation: Option<ActivationId>,
    /// The time of the last accepted activation.
    pub logical_time: LogicalTime,
    /// The last emitted command, if any.
    pub last_command: Option<CommandId>,
    /// The latest command failure and its command identity.
    pub last_command_failure: Option<(CommandId, Failure)>,
    /// The active run and its originating start command.
    pub current_run: Option<(RunId, CommandId)>,
    /// The outstanding timer command and its deadline.
    pub pending_timer: Option<(CommandId, LogicalTime)>,
    /// Runs and cancellation commands awaiting acknowledgment.
    pub pending_cancellations: Vec<(RunId, CommandId)>,
}
/// A complete in-process checkpoint; durable replay starts from [`AppConfig`].
#[derive(Clone, Debug)]
pub struct AppCheckpoint(App);
impl AppCheckpoint {
    /// Checks complete in-process state equality, including histories and genesis.
    pub fn matches(&self, app: &App) -> bool {
        self.0.config == app.config
            && self.0.snapshot() == app.snapshot()
            && self.0.bank == app.bank
            && self.0.play == app.play
            && self.0.watch.solver_index == app.watch.solver_index
            && self.0.watch.history.iter().eq(app.watch.history.iter())
    }
}
/// The pure session model; adapters cannot mutate its state directly.
#[derive(Clone, Debug)]
pub struct App {
    config: AppConfig,
    mode: Mode,
    sidebar_open: bool,
    theme_index: usize,
    bank: PuzzleBank,
    catalog: SolverCatalog,
    play: PlaySession,
    watch: WatchSession,
    overlay: OverlayState,
    notice: Notice,
    next_run: u64,
    quit: bool,
    closed: bool,
    last_activation: Option<ActivationId>,
    logical_time: LogicalTime,
    last_command: Option<CommandId>,
    last_command_failure: Option<(CommandId, Failure)>,
    current_run: Option<(RunId, CommandId)>,
    pending_timer: Option<(CommandId, LogicalTime)>,
    pending_cancellations: Vec<(RunId, CommandId)>,
}
impl App {
    /// Creates a play session with default trace budgets and playback settings.
    ///
    /// # Errors
    /// Rejects an empty bank, an out-of-range zero-based `index`, or an unknown solver.
    pub fn new(
        records: Vec<Record>,
        index: usize,
        label: String,
        solver: &str,
        catalog: SolverCatalog,
    ) -> Result<Self, AppError> {
        Self::with_catalog(
            records,
            index,
            label,
            solver,
            catalog,
            HistoryLimits::default(),
        )
    }
    /// Creates a play session with explicit per-run trace budgets.
    ///
    /// # Errors
    /// Rejects an empty bank, an out-of-range zero-based `index`, or an unknown solver.
    pub fn with_catalog(
        records: Vec<Record>,
        index: usize,
        label: String,
        solver: &str,
        catalog: SolverCatalog,
        limits: HistoryLimits,
    ) -> Result<Self, AppError> {
        AppConfig::new(
            records,
            index,
            label,
            solver,
            catalog,
            limits,
            Mode::Play,
            NonZeroUsize::MIN.saturating_add(7),
            50,
        )
        .map(Self::from_config)
    }
    /// Reconstructs a fresh model from validated portable genesis data.
    pub fn from_config(config: AppConfig) -> Self {
        let solver_index = config.solver_index;
        let bank = PuzzleBank {
            records: config.records.clone(),
            index: config.record_index,
            label: config.label.clone(),
        };
        let play = PlaySession::new(bank.puzzle().clone());
        let watch = WatchSession::new(
            bank.puzzle().clone(),
            solver_index,
            &config.catalog.0[solver_index],
            config.history_limits,
            config.events_per_tick,
        );
        let notice = initial_notice(config.initial_mode);
        Self {
            mode: config.initial_mode,
            sidebar_open: false,
            theme_index: config.initial_theme_index,
            bank,
            catalog: config.catalog.clone(),
            config,
            play,
            watch,
            overlay: OverlayState::Base(BaseOverlay::None),
            notice,
            next_run: 0,
            quit: false,
            closed: false,
            last_activation: None,
            logical_time: LogicalTime::ZERO,
            last_command: None,
            last_command_failure: None,
            current_run: None,
            pending_timer: None,
            pending_cancellations: Vec::new(),
        }
    }
    /// Borrows the immutable genesis configuration used by portable replay.
    pub fn config(&self) -> &AppConfig {
        &self.config
    }
    /// Returns the visible session tab.
    pub fn mode(&self) -> Mode {
        self.mode
    }
    /// Whether the optional details panel is visible across session tabs.
    pub fn sidebar_open(&self) -> bool {
        self.sidebar_open
    }
    /// Borrows the currently selected theme metadata.
    pub fn theme(&self) -> &ThemeDescriptor {
        &self.config.themes.0[self.theme_index]
    }
    /// Borrows all portable theme descriptors.
    pub fn theme_catalog(&self) -> &[ThemeDescriptor] {
        self.config.themes.as_ref()
    }
    /// Borrows the player session, preserved across tab changes.
    pub fn play(&self) -> &PlaySession {
        &self.play
    }
    /// Borrows producer state and trace playback.
    pub fn watch(&self) -> &WatchSession {
        &self.watch
    }
    /// Borrows the authoritative game, including edits beyond the view cursor.
    pub fn game(&self) -> &Game {
        self.play.game()
    }
    /// Returns the number of player edits applied to the view.
    pub fn game_cursor(&self) -> usize {
        self.play.cursor()
    }
    /// Returns the number of trace events applied to the watch view.
    pub fn trace_cursor(&self) -> usize {
        self.watch.history.cursor()
    }
    /// Borrows retained trace history without advancing its cursor.
    pub fn trace_history(&self) -> &TraceHistory {
        &self.watch.history
    }
    /// Borrows the observable solver state at the trace cursor.
    pub fn trace(&self) -> &TraceState {
        self.watch.history.state()
    }
    /// Returns the player selection or the current trace focus, depending on mode.
    pub fn selected(&self) -> CellIndex {
        match self.mode {
            Mode::Play => self.play.selected,
            Mode::Watch => self.trace().focus(),
        }
    }
    /// Borrows the board at the visible mode's history cursor.
    pub fn board(&self) -> &Board {
        match self.mode {
            Mode::Play => &self.play.view,
            Mode::Watch => self.trace().snapshot().board(),
        }
    }
    /// Copies the visible board as 81 row-major digits; zero denotes an empty cell.
    pub fn board_cells(&self) -> [u8; 81] {
        *self.board().cells()
    }
    /// Borrows the current status message and severity.
    pub fn notice(&self) -> &Notice {
        &self.notice
    }
    /// Whether an action or runtime closure has requested exit.
    pub fn quit(&self) -> bool {
        self.quit
    }
    /// Whether a session-end job has closed the model.
    pub fn closed(&self) -> bool {
        self.closed
    }
    /// Whether playback intent is playing; this does not imply an active producer.
    pub fn running(&self) -> bool {
        self.watch.playback == Playback::Playing
    }
    /// Returns the playback quota per timer tick.
    pub fn events_per_tick(&self) -> usize {
        self.watch.events_per_tick()
    }
    /// Borrows the selected strategy identity as text.
    pub fn solver_name(&self) -> &str {
        self.catalog.0[self.watch.solver_index].name()
    }
    /// Returns the selected strategy's zero-based catalog index.
    pub fn solver_index(&self) -> usize {
        self.watch.solver_index
    }
    /// Borrows the available strategy descriptors.
    pub fn solvers(&self) -> &[SolverDescriptor] {
        &self.catalog.0
    }
    /// Returns the zero-based current puzzle selection.
    pub fn record_index(&self) -> usize {
        self.bank.index
    }
    /// Returns the size of the nonempty puzzle bank.
    pub fn record_count(&self) -> usize {
        self.bank.records.len()
    }
    /// Borrows the puzzle-source label.
    pub fn source_label(&self) -> &str {
        &self.bank.label
    }
    /// Whether help currently covers the session.
    pub fn help_open(&self) -> bool {
        matches!(self.overlay, OverlayState::Help { .. })
    }
    /// Returns the solver picker selection, including while help covers it.
    pub fn picker_index(&self) -> Option<usize> {
        let base = match self.overlay {
            OverlayState::Base(base) => base,
            OverlayState::Help { return_to } => return_to,
        };
        match base {
            BaseOverlay::None | BaseOverlay::ThemePicker { .. } => None,
            BaseOverlay::SolverPicker { index } => Some(index),
        }
    }
    /// Returns the theme picker selection, including while help covers it.
    pub fn theme_picker_index(&self) -> Option<usize> {
        let base = match self.overlay {
            OverlayState::Base(base) => base,
            OverlayState::Help { return_to } => return_to,
        };
        match base {
            BaseOverlay::ThemePicker { index } => Some(index),
            BaseOverlay::None | BaseOverlay::SolverPicker { .. } => None,
        }
    }
    /// Configures genesis speed before the first activation.
    /// Later speed changes must arrive as semantic actions so replay captures them.
    ///
    /// # Errors
    /// Returns [`AppError::AlreadyActivated`] after the first activation, or
    /// [`AppError::Speed`] when the quota exceeds 128 events per tick.
    pub fn set_events_per_tick(&mut self, speed: NonZeroUsize) -> Result<(), AppError> {
        if self.last_activation.is_some() {
            return Err(AppError::AlreadyActivated);
        }
        if speed.get() > 128 {
            return Err(AppError::Speed);
        }
        self.watch.events_per_tick = speed;
        self.config.events_per_tick = speed;
        Ok(())
    }
    /// Changes genesis mode before the first activation; later changes are actions.
    ///
    /// # Errors
    /// Returns [`AppError::AlreadyActivated`] after the first activation.
    pub fn set_initial_mode(&mut self, mode: Mode) -> Result<(), AppError> {
        if self.last_activation.is_some() {
            return Err(AppError::AlreadyActivated);
        }
        self.mode = mode;
        self.config.initial_mode = mode;
        self.notice = initial_notice(mode);
        Ok(())
    }
    /// Reports transport demand without introducing empty-poll jobs into history.
    /// A delayed producer leaves playback intent and quota intact until feedback.
    pub fn requested_events(&self) -> usize {
        if matches!(self.watch.run, RunState::Active(_)) && self.watch.playback != Playback::Paused
        {
            self.watch.remaining
        } else {
            0
        }
    }
    /// Returns the current run and its originating start command, if any.
    pub fn current_run(&self) -> Option<(RunId, CommandId)> {
        self.current_run
    }
    /// Returns the outstanding timer command and absolute deadline, if any.
    pub fn pending_timer(&self) -> Option<(CommandId, LogicalTime)> {
        self.pending_timer
    }
    /// Returns the last accepted activation identity.
    pub fn last_activation(&self) -> Option<ActivationId> {
        self.last_activation
    }
    /// Returns the time of the last accepted activation.
    pub fn logical_time(&self) -> LogicalTime {
        self.logical_time
    }
    /// Captures observable and future-affecting state for comparison.
    pub fn snapshot(&self) -> AppSnapshot {
        AppSnapshot {
            mode: self.mode,
            sidebar_open: self.sidebar_open,
            theme: self.theme().id.clone(),
            board: self.board().clone(),
            selected: self.selected(),
            play_board: self.game().board().clone(),
            play_selected: self.play.selected,
            play_cursor: self.play.cursor,
            play_events: self.game().events().len(),
            trace_cursor: self.trace_cursor(),
            trace_events: self.trace_history().len(),
            trace: self.trace().clone(),
            run: self.watch.run.clone(),
            playback: self.watch.playback,
            overlay: self.overlay,
            notice: self.notice.clone(),
            record_index: self.record_index(),
            solver: self.catalog.0[self.watch.solver_index].id.clone(),
            events_per_tick: self.events_per_tick(),
            remaining: self.watch.remaining,
            next_run: self.next_run,
            history_limits: self.watch.history.limits(),
            quit: self.quit,
            closed: self.closed,
            last_activation: self.last_activation,
            logical_time: self.logical_time,
            last_command: self.last_command,
            last_command_failure: self.last_command_failure.clone(),
            current_run: self.current_run,
            pending_timer: self.pending_timer,
            pending_cancellations: self.pending_cancellations.clone(),
        }
    }
    /// Clones complete in-process state, including genesis and retained histories.
    pub fn checkpoint(&self) -> AppCheckpoint {
        AppCheckpoint(self.clone())
    }
    /// Applies semantic jobs and returns data-only commands for infrastructure.
    /// Producer failures are recorded model outcomes; old-run feedback is ignored.
    /// Every accepted activation advances its identity and logical clock, including
    /// ignored stale feedback. A closed session accepts command failures and
    /// cancellation acknowledgments while ignoring further user or producer activity.
    ///
    /// # Errors
    /// Rejects noncontiguous activation identities, decreasing logical time, early
    /// identified timers, failure feedback for an unemitted command, or exhausted
    /// identity/time space before changing state.
    pub fn activate(&mut self, activation: &Activation) -> Result<Completion, ProtocolError> {
        let expected = match self.last_activation {
            Some(last) => last.checked_next()?,
            None => ActivationId::FIRST,
        };
        if activation.id() != expected || activation.time() < self.logical_time {
            return Err(ProtocolError::ActivationOrder);
        }
        if activation.jobs().iter().any(|job| {
            matches!(job, Job::CommandFailed { command, .. }
                if self.last_command.is_none_or(|last| *command > last))
        }) {
            return Err(ProtocolError::CommandReference);
        }
        if let Some((pending, deadline)) = self.pending_timer
            && activation.time() < deadline
            && activation
                .jobs()
                .iter()
                .any(|job| matches!(job, Job::TimerFired { command } if *command == pending))
        {
            return Err(ProtocolError::EarlyTimer);
        }
        // Each job can emit at most one solver command, followed by one shared timer.
        // Reserve identity and time space up front so fallible envelopes are atomic.
        let count = u64::try_from(activation.jobs().len()).map_err(|_| ProtocolError::Exhausted)?;
        self.last_command
            .map_or(0, CommandId::get)
            .checked_add(count)
            .and_then(|value| value.checked_add(1))
            .ok_or(ProtocolError::Exhausted)?;
        self.next_run
            .checked_add(count)
            .ok_or(ProtocolError::Exhausted)?;
        let deadline = activation
            .time()
            .checked_add(self.config.tick_interval_ms)?;
        self.logical_time = activation.time();
        let mut commands = Vec::new();
        for job in activation.jobs() {
            self.job(job, &mut commands)?;
        }
        if !self.quit
            && !self.closed
            && self.mode == Mode::Watch
            && self.watch.playback != Playback::Paused
            && self.pending_timer.is_none()
        {
            let id = self.emit(CommandKind::ScheduleTimer { deadline }, &mut commands)?;
            self.pending_timer = Some((id, deadline));
        }
        self.last_activation = Some(activation.id());
        Completion::new(activation.id(), commands)
    }
    fn emit(
        &mut self,
        kind: CommandKind,
        commands: &mut Vec<Command>,
    ) -> Result<CommandId, ProtocolError> {
        let id = match self.last_command {
            Some(last) => last.checked_next()?,
            None => CommandId::FIRST,
        };
        self.last_command = Some(id);
        commands.push(Command { id, kind });
        Ok(id)
    }
    fn pause(&mut self) {
        self.watch.pause();
        self.pending_timer = None;
    }
    fn cancel_current(&mut self, commands: &mut Vec<Command>) -> Result<(), ProtocolError> {
        if let Some((run, _)) = self.current_run.take() {
            let command = self.emit(CommandKind::CancelSolver { run }, commands)?;
            self.pending_cancellations.push((run, command));
        }
        Ok(())
    }
    fn job(&mut self, job: &Job, commands: &mut Vec<Command>) -> Result<(), ProtocolError> {
        if let Job::CommandFailed { command, failure } = job {
            self.last_command_failure = Some((*command, failure.clone()));
            if self.current_run.is_some_and(|(_, start)| start == *command)
                || self
                    .pending_timer
                    .is_some_and(|(timer, _)| timer == *command)
            {
                self.stop(
                    StopReason::Failed(failure.clone()),
                    format!("Command stopped: {}", failure.message),
                    commands,
                )?;
            }
            return Ok(());
        }
        if let Job::SessionEnded { .. } = job {
            self.closed = true;
            self.pause();
            if matches!(self.watch.run, RunState::Starting(_) | RunState::Active(_)) {
                self.watch.run = RunState::Stopped(StopReason::Cancelled);
            }
            return self.cancel_current(commands);
        }
        if let Job::SolverCancelled { run, command } = job {
            if let Some(index) = self
                .pending_cancellations
                .iter()
                .position(|(pending, _)| pending == run)
            {
                if self.pending_cancellations[index].1 == *command {
                    self.pending_cancellations.remove(index);
                } else {
                    self.protocol_failure(
                        "cancellation acknowledgment has the wrong command identity",
                        commands,
                    )?;
                }
            } else if self.current_run.is_some_and(|(active, _)| active == *run) {
                self.protocol_failure("producer cancellation was not requested", commands)?;
            }
            return Ok(());
        }
        if self.quit || self.closed {
            return Ok(());
        }
        match job {
            Job::Action(action) => self.action(*action, commands)?,
            Job::TimerFired { command } => {
                if let Some((pending, deadline)) = self.pending_timer
                    && pending == *command
                    && self.logical_time >= deadline
                {
                    self.pending_timer = None;
                    self.watch.remaining = match self.watch.playback {
                        Playback::Paused => 0,
                        Playback::Playing => self.events_per_tick(),
                        Playback::AwaitingStep => 1,
                    };
                    self.advance();
                }
            }
            Job::SolverStarted { run, command } => {
                if self.correlated(*run, *command, commands)? {
                    if self.watch.run == RunState::Starting(*run) {
                        self.watch.run = RunState::Active(*run);
                        self.advance();
                    } else {
                        self.protocol_failure("producer started more than once", commands)?;
                    }
                }
            }
            Job::SolverEvents {
                run,
                command,
                events,
            } => {
                if self.correlated(*run, *command, commands)? {
                    if self.watch.run != RunState::Active(*run) {
                        self.protocol_failure(
                            "solver events arrived before producer startup",
                            commands,
                        )?;
                    } else if events.is_empty() {
                        self.protocol_failure(
                            "an empty transport poll is not a semantic solver job",
                            commands,
                        )?;
                    } else {
                        for event in events {
                            if matches!(event, TraceEvent::Finished { .. }) {
                                self.protocol_failure(
                                    "completion requires a SolverCompleted job",
                                    commands,
                                )?;
                                break;
                            }
                            if !self.append(event.clone(), commands)? {
                                break;
                            }
                        }
                        if self.watch.run == RunState::Active(*run) {
                            self.advance();
                        }
                    }
                }
            }
            Job::SolverCompleted {
                run,
                command,
                result,
                snapshot,
            } => {
                if self.correlated(*run, *command, commands)? {
                    if self.watch.run != RunState::Active(*run) {
                        self.protocol_failure(
                            "completion arrived before producer startup",
                            commands,
                        )?;
                    } else if self.append(
                        TraceEvent::Finished {
                            result: result.clone(),
                            snapshot: snapshot.clone(),
                        },
                        commands,
                    )? {
                        self.watch.run = RunState::Finished(result.clone());
                        self.cancel_current(commands)?;
                        self.advance();
                    }
                }
            }
            Job::SolverFailed {
                run,
                command,
                failure,
            } => {
                if self.correlated(*run, *command, commands)? {
                    self.stop(
                        StopReason::Failed(failure.clone()),
                        format!("Solver stopped: {}", failure.message),
                        commands,
                    )?;
                }
            }
            Job::CommandFailed { .. } | Job::SolverCancelled { .. } | Job::SessionEnded { .. } => {}
        }
        Ok(())
    }
    fn correlated(
        &mut self,
        run: RunId,
        command: CommandId,
        commands: &mut Vec<Command>,
    ) -> Result<bool, ProtocolError> {
        let Some((active, started_by)) = self.current_run else {
            return Ok(false);
        };
        if run != active {
            return Ok(false);
        }
        if command != started_by {
            self.protocol_failure(
                "active run feedback has the wrong start command identity",
                commands,
            )?;
            return Ok(false);
        }
        Ok(true)
    }
    fn protocol_failure(
        &mut self,
        message: &str,
        commands: &mut Vec<Command>,
    ) -> Result<(), ProtocolError> {
        let failure = Failure {
            kind: FailureKind::Protocol,
            message: message.to_owned(),
        };
        self.stop(StopReason::Failed(failure), message.to_owned(), commands)
    }
    fn action(&mut self, action: Action, commands: &mut Vec<Command>) -> Result<(), ProtocolError> {
        if action == Action::Quit {
            self.quit = true;
            self.pause();
            if matches!(self.watch.run, RunState::Starting(_) | RunState::Active(_)) {
                self.watch.run = RunState::Stopped(StopReason::Cancelled);
            }
            return self.cancel_current(commands);
        }
        if action == Action::ToggleSidebar {
            self.sidebar_open = !self.sidebar_open;
            return Ok(());
        }
        if action == Action::Help {
            self.pause();
            self.overlay = match self.overlay {
                OverlayState::Base(base) => OverlayState::Help { return_to: base },
                OverlayState::Help { return_to } => OverlayState::Base(return_to),
            };
            return Ok(());
        }
        if let OverlayState::Help { return_to } = self.overlay {
            if action == Action::Escape {
                self.overlay = OverlayState::Base(return_to);
            }
            return Ok(());
        }
        if let OverlayState::Base(BaseOverlay::ThemePicker { index }) = self.overlay {
            match action {
                Action::Move { rows, .. } => {
                    self.overlay = OverlayState::Base(BaseOverlay::ThemePicker {
                        index: index
                            .saturating_add_signed(isize::from(rows))
                            .min(self.config.themes.0.len() - 1),
                    });
                }
                Action::Accept => {
                    self.theme_index = index;
                    self.overlay = OverlayState::Base(BaseOverlay::None);
                }
                Action::Escape => self.overlay = OverlayState::Base(BaseOverlay::None),
                _ => {}
            }
            return Ok(());
        }
        if let OverlayState::Base(BaseOverlay::SolverPicker { index }) = self.overlay {
            match action {
                Action::Move { rows, .. } => {
                    self.overlay = OverlayState::Base(BaseOverlay::SolverPicker {
                        index: index
                            .saturating_add_signed(isize::from(rows))
                            .min(self.catalog.0.len() - 1),
                    })
                }
                Action::Accept => {
                    self.cancel_current(commands)?;
                    self.overlay = OverlayState::Base(BaseOverlay::None);
                    self.reset_watch(index);
                    self.notice = Notice::info(format!(
                        "Selected {}. Space runs; ] steps.",
                        self.solver_name()
                    ));
                }
                Action::Escape => self.overlay = OverlayState::Base(BaseOverlay::None),
                _ => {}
            }
            return Ok(());
        }
        match action {
            Action::Escape => self.sidebar_open = false,
            Action::ChooseTheme => {
                self.pause();
                self.overlay = OverlayState::Base(BaseOverlay::ThemePicker {
                    index: self.theme_index,
                });
            }
            Action::ChooseSolver => {
                self.pause();
                self.overlay = OverlayState::Base(BaseOverlay::SolverPicker {
                    index: self.watch.solver_index,
                });
            }
            Action::ToggleMode => {
                self.pause();
                self.mode = match self.mode {
                    Mode::Play => Mode::Watch,
                    Mode::Watch => Mode::Play,
                };
                self.notice = Notice::info(match self.mode {
                    Mode::Play => "Play session restored. Watch uses the original puzzle.",
                    Mode::Watch => {
                        "Watch the original puzzle. Space runs; ] steps; s selects a solver."
                    }
                });
            }
            Action::NextPuzzle => {
                self.cancel_current(commands)?;
                self.bank.next();
                self.play = PlaySession::new(self.bank.puzzle().clone());
                self.reset_watch(self.watch.solver_index);
                self.notice = Notice::info(format!(
                    "Loaded puzzle {}/{}.",
                    self.bank.index + 1,
                    self.bank.records.len()
                ));
            }
            Action::Move { rows, columns } => match self.mode {
                Mode::Play => {
                    self.play.selected = self.play.selected.moved(rows, columns);
                    self.notice = Notice::info("Enter a digit or clear the selected cell.");
                }
                Mode::Watch if columns < 0 => self.step_back(),
                Mode::Watch if columns > 0 => self.request_step(commands)?,
                Mode::Watch => {}
            },
            Action::Digit(digit) if self.mode == Mode::Play => self.edit(Some(digit)),
            Action::Clear if self.mode == Mode::Play => self.edit(None),
            Action::StepBack => self.step_back(),
            Action::StepForward => match self.mode {
                Mode::Play => {
                    self.seek_game((self.play.cursor + 1).min(self.game().events().len()))
                }
                Mode::Watch => self.request_step(commands)?,
            },
            Action::HistoryStart | Action::Reset => {
                self.pause();
                match self.mode {
                    Mode::Play => self.seek_game(0),
                    Mode::Watch => self.seek_trace(0),
                }
            }
            Action::HistoryEnd => {
                self.pause();
                match self.mode {
                    Mode::Play => self.seek_game(self.game().events().len()),
                    Mode::Watch => self.seek_trace(self.trace_history().len()),
                }
            }
            Action::ToggleRun if self.mode == Mode::Watch => {
                if self.running() {
                    self.pause();
                    self.notice = Notice::info("Paused. [ / ] review events.");
                } else {
                    if self.trace_cursor() == self.trace_history().len()
                        && matches!(self.watch.run, RunState::Finished(_) | RunState::Stopped(_))
                    {
                        self.seek_trace(0);
                    }
                    self.watch.playback = Playback::Playing;
                    self.notice = Notice::info("Playing solver events.");
                    self.start_trace(commands)?;
                }
            }
            Action::Faster => {
                self.watch.events_per_tick = self
                    .watch
                    .events_per_tick
                    .saturating_mul(NonZeroUsize::MIN.saturating_add(1))
                    .min(NonZeroUsize::MIN.saturating_add(127))
            }
            Action::Slower => {
                self.watch.events_per_tick =
                    NonZeroUsize::new(self.events_per_tick() / 2).unwrap_or(NonZeroUsize::MIN)
            }
            // Time and repaint transport signals have no semantic effect.
            Action::Tick | Action::Resize => {}
            _ => {}
        }
        Ok(())
    }
    fn edit(&mut self, digit: Option<Digit>) {
        if self.play.cursor != self.game().events().len() {
            self.notice =
                Notice::error("Reviewing history. End returns to the current game before editing.");
            return;
        }
        match self.play.game.edit_cell(self.play.selected, digit) {
            Ok(None) => self.notice = Notice::info("Cell unchanged."),
            Ok(Some(_)) => {
                self.play.cursor = self.game().events().len();
                self.play.view = self.game().board().clone();
                self.notice = Notice::info(if self.game().is_complete() {
                    "Puzzle complete! n starts the next puzzle.".into()
                } else if let Some(digit) = digit {
                    format!(
                        "Placed {} at row {}, column {}.",
                        digit.get(),
                        self.play.selected.row() + 1,
                        self.play.selected.column() + 1
                    )
                } else {
                    "Cell cleared.".into()
                });
            }
            Err(error) => self.notice = Notice::error(error.to_string()),
        }
    }
    fn seek_game(&mut self, cursor: usize) {
        if let Err(error) = self.play.seek(cursor) {
            self.notice = Notice::error(format!("Cannot replay game history: {error}"));
            return;
        }
        self.notice = Notice::info(if cursor < self.game().events().len() {
            "Game history. End returns to the current game."
        } else if self.game().is_complete() {
            "Puzzle complete! n starts the next puzzle."
        } else {
            "Current game. Enter a digit to continue."
        });
    }
    fn step_back(&mut self) {
        self.pause();
        match self.mode {
            Mode::Play => self.seek_game(self.play.cursor.saturating_sub(1)),
            Mode::Watch => self.seek_trace(self.trace_cursor().saturating_sub(1)),
        }
    }
    fn seek_trace(&mut self, cursor: usize) {
        match self.watch.history.seek(cursor) {
            Ok(()) => {
                self.notice = Notice::info(format!(
                    "Paused at event {cursor}/{}. Space replays; ] steps.",
                    self.trace_history().len()
                ))
            }
            Err(error) => {
                self.notice = Notice::error(format!("Cannot replay solver history: {error}"))
            }
        }
    }
    fn reset_watch(&mut self, solver_index: usize) {
        let limits = self.watch.history.limits();
        let speed = self.watch.events_per_tick;
        self.watch = WatchSession::new(
            self.bank.puzzle().clone(),
            solver_index,
            &self.catalog.0[solver_index],
            limits,
            speed,
        );
        self.pending_timer = None;
    }
    fn start_trace(&mut self, commands: &mut Vec<Command>) -> Result<(), ProtocolError> {
        if self.watch.run != RunState::NotStarted {
            return Ok(());
        }
        let solver = &self.catalog.0[self.watch.solver_index];
        if !solver.tracing {
            self.pause();
            self.notice = Notice::error(format!(
                "{} does not support tracing. Choose another solver.",
                self.solver_name()
            ));
            return Ok(());
        }
        let next = self
            .next_run
            .checked_add(1)
            .ok_or(ProtocolError::Exhausted)?;
        let run = RunId::try_from(next)?;
        let id = self.emit(
            CommandKind::StartSolver {
                run,
                solver: solver.id.clone(),
                puzzle: self.bank.puzzle().clone(),
            },
            commands,
        )?;
        self.next_run = next;
        self.current_run = Some((run, id));
        self.watch.run = RunState::Starting(run);
        Ok(())
    }
    fn request_step(&mut self, commands: &mut Vec<Command>) -> Result<(), ProtocolError> {
        self.pending_timer = None;
        self.watch.playback = Playback::AwaitingStep;
        self.watch.remaining = 1;
        self.start_trace(commands)?;
        self.advance();
        Ok(())
    }
    fn advance(&mut self) {
        while self.watch.remaining != 0 && self.watch.history.step_forward() {
            self.watch.remaining -= 1;
            if self.trace().finished().is_some() {
                self.finish_playback();
                return;
            }
        }
        if self.trace_cursor() == self.trace_history().len()
            && matches!(self.watch.run, RunState::Stopped(_))
        {
            self.pause();
            self.notice =
                Notice::info("Reached the end of the retained trace. Home rewinds; Space replays.");
            return;
        }
        if self.watch.remaining == 0 && self.watch.playback == Playback::AwaitingStep {
            self.pause();
        }
    }
    fn append(
        &mut self,
        event: TraceEvent,
        commands: &mut Vec<Command>,
    ) -> Result<bool, ProtocolError> {
        if let Err(error) = self.watch.history.push(event) {
            let message = error.to_string();
            let reason = match error {
                HistoryError::EventLimit | HistoryError::ByteLimit => StopReason::HistoryLimit,
                HistoryError::Allocation => StopReason::StorageFailure,
                HistoryError::Protocol(_) | HistoryError::Cursor { .. } => {
                    StopReason::InvalidTrace(message.clone())
                }
            };
            self.stop(reason, format!("Trace stopped: {message}. Recorded history remains replayable; choose another solver."), commands)?;
            return Ok(false);
        }
        Ok(true)
    }
    fn stop(
        &mut self,
        reason: StopReason,
        message: String,
        commands: &mut Vec<Command>,
    ) -> Result<(), ProtocolError> {
        self.watch.run = RunState::Stopped(reason);
        self.pause();
        self.notice = Notice::error(message);
        self.cancel_current(commands)
    }
    fn finish_playback(&mut self) {
        self.pause();
        self.notice = Notice::info(
            if self
                .trace()
                .finished()
                .is_some_and(|result| result.solution().is_some())
            {
                "Solver finished. Home rewinds; Space replays the recorded run."
            } else {
                "Solver finished: no solution. Home rewinds the search."
            },
        );
    }
}
fn initial_cell(puzzle: &Board) -> CellIndex {
    puzzle
        .cells()
        .iter()
        .position(|&digit| digit == 0)
        .and_then(|index| CellIndex::try_from(index).ok())
        .unwrap_or(CellIndex::FIRST)
}

fn initial_notice(mode: Mode) -> Notice {
    Notice::info(match mode {
        Mode::Play => "Enter digits to play. Tab opens the solver workbench.",
        Mode::Watch => "Watch the original puzzle. Space runs; ] steps; s selects a solver.",
    })
}
