//! A Sudoku workbench: a geometric board and a compact search evidence rail.

use ku_core::{Board, CandidateMask, CellIndex, TraceEvent, TraceHistory, TraceKind};

use ratatui::{
    Frame,
    buffer::Buffer,
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Clear, Gauge, Paragraph, Widget, Wrap},
};

use super::{
    app::{App, Mode, RunState, Severity},
    theme::Palette,
};

/// Draws the application without changing any state or consulting the clock.
pub fn render(frame: &mut Frame, app: &App) {
    let area = frame.area();
    let palette = Palette::for_app(app);
    frame.render_widget(
        Block::default().style(Style::default().fg(palette.ink).bg(palette.table)),
        area,
    );
    if area.width < 37 || area.height < 22 {
        frame.render_widget(
            Paragraph::new("Ku / SUDOKU\n\nResize to at least 37 × 22.\nq / Ctrl+C quit")
                .style(Style::default().fg(palette.brand))
                .wrap(Wrap { trim: false }),
            area,
        );
        return;
    }
    let [header, body, footer] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Min(14),
        Constraint::Length(3),
    ])
    .areas(area);
    render_header(frame, header, app, palette);
    if app.sidebar_open() && body.width >= 89 {
        let [board, _, details] = Layout::horizontal([
            Constraint::Min(52),
            Constraint::Length(3),
            Constraint::Length(34),
        ])
        .areas(body);
        render_board(frame, board, app, palette);
        render_details(frame, centered(details, 32, 28), app, palette);
    } else {
        render_board(frame, body, app, palette);
        if app.sidebar_open() {
            let drawer = centered(body, 52.min(body.width.saturating_sub(2)), 24);
            frame.render_widget(Clear, drawer);
            render_details(frame, drawer, app, palette);
        }
    }
    render_footer(frame, footer, app, palette);
    if app.picker_index().is_some() {
        render_picker(frame, area, app, palette);
    }
    if app.theme_picker_index().is_some() {
        render_theme_picker(frame, area, app, palette);
    }
    if app.help_open() {
        render_help(frame, area, palette);
    }
}

fn render_header(frame: &mut Frame, area: Rect, app: &App, palette: Palette) {
    let mode = match app.mode() {
        Mode::Play => "PLAY",
        Mode::Watch => "SOLVER",
    };
    let inset = Rect::new(
        area.x + 2,
        area.y,
        area.width.saturating_sub(4),
        area.height,
    );
    let brand = Line::from(vec![
        Span::styled(
            "Ku",
            Style::default()
                .fg(palette.brand)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("  /  ", Style::default().fg(palette.muted)),
        Span::styled(mode, Style::default().fg(palette.ink)),
    ]);
    let context = if area.width >= 68 {
        format!(
            "{}  ·  {}/{}",
            app.source_label(),
            app.record_index() + 1,
            app.record_count()
        )
    } else {
        format!("PUZZLE {}/{}", app.record_index() + 1, app.record_count())
    };
    // Independent left and right bands keep long source labels away from the mode.
    let [left, right] =
        Layout::horizontal([Constraint::Length(17), Constraint::Min(1)]).areas(inset);
    frame.render_widget(Paragraph::new(brand), left);
    frame.render_widget(
        Paragraph::new(context)
            .alignment(Alignment::Right)
            .style(Style::default().fg(palette.muted)),
        right,
    );
}

#[derive(Clone, Copy, Debug)]
struct GridGeometry {
    cell_width: usize,
    cell_height: usize,
}

impl GridGeometry {
    fn for_area(area: Rect) -> Self {
        let cell_width = if area.width >= 55 { 5 } else { 3 };
        let cell_height = if cell_width == 5 && area.height >= 32 {
            3
        } else {
            1
        };
        Self {
            cell_width,
            cell_height,
        }
    }

    fn width(self) -> u16 {
        (self.cell_width * 9 + 10) as u16
    }

    fn height(self) -> u16 {
        (self.cell_height * 9 + 5) as u16
    }

    #[cfg(test)]
    fn cell_center(self, cell: CellIndex) -> (u16, u16) {
        (
            (4 + cell.column() * self.cell_width + cell.column() / 3 + self.cell_width / 2) as u16,
            (2 + cell.row() * self.cell_height + cell.row() / 3 + self.cell_height / 2) as u16,
        )
    }
}

/// Borrowed presentation data for one Sudoku grid.
///
/// The boards and cell indexes are validated domain values. Rendering does not
/// need the application, game history, solver, or input infrastructure.
#[derive(Clone, Copy, Debug)]
pub struct BoardView<'a> {
    board: &'a Board,
    puzzle: &'a Board,
    selected: CellIndex,
    guessed: Option<CellIndex>,
    error: bool,
}

impl<'a> BoardView<'a> {
    /// Describes a grid and its immutable given clues.
    pub fn new(board: &'a Board, puzzle: &'a Board, selected: CellIndex) -> Self {
        Self {
            board,
            puzzle,
            selected,
            guessed: None,
            error: false,
        }
    }

    /// Highlights the cell receiving the current unforced solver assignment.
    pub fn with_guess(mut self, guessed: Option<CellIndex>) -> Self {
        self.guessed = guessed;
        self
    }

    /// Marks the selected cell as the subject of invalid-move feedback.
    pub fn with_error(mut self, error: bool) -> Self {
        self.error = error;
        self
    }
}

/// A read-only Sudoku grid usable with any Ratatui backend.
///
/// # Examples
///
/// ```
/// use ku_cli::tui::{render::{BoardView, SudokuBoard}, theme::Palette};
/// use ku_core::{Board, CellIndex};
/// use ratatui::{buffer::Buffer, layout::Rect, widgets::Widget};
///
/// let puzzle = Board::default();
/// let area = Rect::new(0, 0, 55, 32);
/// let mut buffer = Buffer::empty(area);
/// let view = BoardView::new(&puzzle, &puzzle, CellIndex::FIRST);
/// SudokuBoard::new(view).with_palette(Palette::default()).render(area, &mut buffer);
/// assert!(buffer.content().iter().any(|cell| cell.symbol() != " "));
/// ```
#[derive(Clone, Copy, Debug)]
pub struct SudokuBoard<'a> {
    view: BoardView<'a>,
    palette: Palette,
}

impl<'a> SudokuBoard<'a> {
    /// Creates a grid widget from borrowed presentation data.
    pub fn new(view: BoardView<'a>) -> Self {
        Self {
            view,
            palette: Palette::default(),
        }
    }

    /// Uses an immutable semantic palette for every part of this widget.
    pub fn with_palette(mut self, palette: Palette) -> Self {
        self.palette = palette;
        self
    }
}

impl Widget for SudokuBoard<'_> {
    fn render(self, area: Rect, buffer: &mut Buffer) {
        let palette = self.palette;
        let BoardView {
            board,
            puzzle,
            selected,
            guessed,
            error,
        } = self.view;
        let geometry = GridGeometry::for_area(area);
        let grid = centered(area, geometry.width(), geometry.height());
        let width = geometry.cell_width;
        let border_style = Style::default().fg(palette.grid).bg(palette.table);
        let mut columns = vec![Span::raw("    ")];
        for column in 0..9 {
            columns.push(Span::styled(
                format!("{:^width$}", column + 1),
                Style::default().fg(if column == selected.column() {
                    palette.focus
                } else {
                    palette.muted
                }),
            ));
            if column % 3 == 2 && column != 8 {
                columns.push(Span::raw(" "));
            }
        }
        let mut lines = vec![Line::from(columns)];
        let section = "━".repeat(width * 3);
        for row in 0..9 {
            if row % 3 == 0 {
                let (left, cross, right) = if row == 0 {
                    ("┏", "┳", "┓")
                } else {
                    ("┣", "╋", "┫")
                };
                lines.push(Line::styled(
                    format!("   {left}{section}{cross}{section}{cross}{section}{right}"),
                    border_style,
                ));
            }
            for line in 0..geometry.cell_height {
                let middle = line == geometry.cell_height / 2;
                let mut spans = vec![
                    Span::styled(
                        if middle {
                            format!("{}  ", row + 1)
                        } else {
                            "   ".to_owned()
                        },
                        Style::default().fg(if row == selected.row() {
                            palette.focus
                        } else {
                            palette.muted
                        }),
                    ),
                    Span::styled("┃", border_style),
                ];
                for column in 0..9 {
                    let cell = row * 9 + column;
                    let given = puzzle.cells()[cell] != 0;
                    let digit = board.cells()[cell];
                    let selected_cell = cell == selected.get();
                    let mut style = Style::default()
                        .fg(if given {
                            palette.ink
                        } else if digit != 0 {
                            palette.value
                        } else {
                            palette.muted
                        })
                        .bg(if row == selected.row() || column == selected.column() {
                            palette.peer
                        } else {
                            palette.table
                        });
                    if given {
                        style = style.add_modifier(Modifier::BOLD);
                    }
                    if guessed.is_some_and(|guessed| guessed.get() == cell) {
                        style = style.fg(palette.guess).add_modifier(Modifier::BOLD);
                    }
                    if selected_cell {
                        style = style
                            .bg(if error {
                                palette.alert
                            } else {
                                palette.selection_bg
                            })
                            .fg(if error {
                                palette.error_fg
                            } else {
                                palette.selection_fg
                            })
                            .add_modifier(Modifier::BOLD);
                    }
                    let content = if middle {
                        let symbol = if digit == 0 {
                            "·".to_owned()
                        } else {
                            digit.to_string()
                        };
                        if selected_cell && given {
                            format!("[{symbol}]")
                        } else {
                            symbol
                        }
                    } else {
                        String::new()
                    };
                    spans.push(Span::styled(format!("{content:^width$}"), style));
                    if column % 3 == 2 {
                        spans.push(Span::styled("┃", border_style));
                    }
                }
                lines.push(Line::from(spans));
            }
        }
        lines.push(Line::styled(
            format!("   ┗{section}┻{section}┻{section}┛"),
            border_style,
        ));
        Paragraph::new(lines)
            .style(Style::default().bg(palette.table))
            .render(grid, buffer);
    }
}

fn render_board(frame: &mut Frame, area: Rect, app: &App, palette: Palette) {
    let guessed = if app.mode() == Mode::Watch {
        match current_event(app.trace_history()) {
            Some(TraceEvent::Step {
                kind:
                    TraceKind::Assignment {
                        cell,
                        forced: false,
                        ..
                    },
                ..
            }) => Some(*cell),
            _ => None,
        }
    } else {
        None
    };
    let view = BoardView::new(app.board(), app.game().puzzle(), app.selected())
        .with_guess(guessed)
        .with_error(app.notice().severity() == Severity::Error);
    frame.render_widget(SudokuBoard::new(view).with_palette(palette), area);
}

fn current_event(history: &TraceHistory) -> Option<&TraceEvent> {
    history
        .cursor()
        .checked_sub(1)
        .and_then(|index| history.event(index))
}

/// Borrowed facts for the history and search evidence panel.
#[derive(Clone, Copy, Debug)]
pub enum DetailsView<'a> {
    /// A human game at its selected move-history position.
    Play {
        /// Immutable given clues used to recognize completion.
        puzzle: &'a Board,
        /// The board at the current history cursor.
        board: &'a Board,
        /// Cell selected for editing.
        selected: CellIndex,
        /// Number of applied game events.
        cursor: usize,
        /// Number of retained game events.
        events: usize,
    },
    /// A solver trace at its selected replay position.
    Watch {
        /// Immutable given clues used to recognize completion.
        puzzle: &'a Board,
        /// Validated trace and its replay cursor.
        history: &'a TraceHistory,
        /// Cell whose recorded candidates are displayed.
        selected: CellIndex,
        /// Name displayed above the recorded work.
        solver: &'a str,
        /// Whether playback currently advances on ticks.
        running: bool,
        /// Number of events requested per playback tick.
        events_per_tick: usize,
    },
}

/// A read-only history and search evidence widget.
#[derive(Clone, Copy, Debug)]
pub struct SearchDetails<'a> {
    view: DetailsView<'a>,
    palette: Palette,
}

impl<'a> SearchDetails<'a> {
    /// Creates a panel from borrowed history or game facts.
    pub fn new(view: DetailsView<'a>) -> Self {
        Self {
            view,
            palette: Palette::default(),
        }
    }

    /// Uses an immutable semantic palette for every part of this widget.
    pub fn with_palette(mut self, palette: Palette) -> Self {
        self.palette = palette;
        self
    }
}

impl Widget for SearchDetails<'_> {
    fn render(self, area: Rect, buffer: &mut Buffer) {
        let palette = self.palette;
        let (mut lines, value, total, completed, color) = match self.view {
            DetailsView::Play {
                puzzle,
                board,
                selected,
                cursor,
                events,
            } => {
                let filled = board.clue_count();
                let completed = board.is_solution_for(puzzle);
                let mut lines = vec![
                    Line::styled(
                        "YOUR PUZZLE",
                        Style::default()
                            .fg(palette.brand)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Line::raw(if completed {
                        "Complete · well played."
                    } else {
                        "Fill the grid. Givens are locked."
                    }),
                    Line::raw(format!("Filled     {filled}/81")),
                    Line::raw(format!("History    {cursor}/{events}")),
                    Line::raw(format!(
                        "Cell       r{} c{}",
                        selected.row() + 1,
                        selected.column() + 1
                    )),
                ];
                if puzzle.cells()[selected.get()] != 0 {
                    if area.height >= 18 {
                        lines.extend([
                            Line::styled(
                                " ╭─╮     GIVEN / LOCKED",
                                Style::default().fg(palette.brand),
                            ),
                            Line::styled(
                                "╭┴─┴╮    Immutable clue",
                                Style::default().fg(palette.brand),
                            ),
                            Line::styled(
                                "│ • │    Cannot edit",
                                Style::default().fg(palette.brand),
                            ),
                            Line::styled("╰───╯", Style::default().fg(palette.brand)),
                        ]);
                    } else {
                        lines.push(Line::styled(
                            "GIVEN / LOCKED",
                            Style::default().fg(palette.brand),
                        ));
                    }
                } else {
                    lines.push(Line::raw(""));
                }
                lines.extend([
                    Line::styled(
                        "1–9 enter   0 / Del clear",
                        Style::default().fg(palette.muted),
                    ),
                    Line::styled(
                        "[ / ] replay your moves",
                        Style::default().fg(palette.muted),
                    ),
                    Line::styled("Tab inspect a solver", Style::default().fg(palette.muted)),
                ]);
                (lines, filled, 81, completed, palette.success)
            }
            DetailsView::Watch {
                puzzle,
                history,
                selected,
                solver,
                running,
                events_per_tick,
            } => {
                let state = history.state();
                let stats = state.stats();
                let phase = if state.finished().is_some() {
                    "finished"
                } else if running {
                    "running"
                } else {
                    "paused"
                };
                let mut lines = vec![
                    Line::styled(
                        "SEARCH OBSERVATORY",
                        Style::default()
                            .fg(palette.guess)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Line::raw(format!("{solver} · {phase}")),
                    Line::raw(format!("Event      {}/{}", history.cursor(), history.len())),
                    Line::raw(format!("Depth      {}", state.depth())),
                    Line::raw(format!("Nodes      {}", stats.nodes)),
                    Line::raw(format!("Guesses    {}", stats.guesses)),
                    Line::raw(format!("Backtracks {}", stats.backtracks)),
                    Line::raw(format!("Forced     {}", stats.forced_moves)),
                    Line::raw(format!("Speed      {events_per_tick} events/tick")),
                    Line::styled(
                        current_event(history).map_or_else(
                            || "Ready to trace real search".to_owned(),
                            describe_event,
                        ),
                        Style::default().fg(palette.guess),
                    ),
                ];
                lines.extend(candidate_changes(history, palette));
                if let Some(candidates) = state.candidates() {
                    lines.push(Line::raw(format!(
                        "Candidates {}",
                        candidate_digits(candidates[selected.get()])
                    )));
                }
                (
                    lines,
                    history.cursor(),
                    history.len().max(1),
                    state.snapshot().board().is_solution_for(puzzle),
                    palette.guess,
                )
            }
        };
        if completed {
            lines.push(Line::styled(
                "SOLVED",
                Style::default()
                    .fg(palette.success)
                    .add_modifier(Modifier::BOLD),
            ));
        }
        let [details, progress] =
            Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(area);
        Paragraph::new(lines)
            .style(Style::default().fg(palette.ink).bg(palette.table))
            .wrap(Wrap { trim: false })
            .block(
                Block::bordered()
                    .border_style(Style::default().fg(palette.grid))
                    .style(Style::default().bg(palette.table))
                    .title(" DETAILS · i / Esc close "),
            )
            .render(details, buffer);
        Gauge::default()
            .style(Style::default().fg(palette.ink).bg(palette.table))
            .ratio((value as f64 / total as f64).clamp(0.0, 1.0))
            .label(format!("{value}/{total}"))
            .gauge_style(Style::default().fg(color).bg(palette.table))
            .render(progress, buffer);
    }
}

fn render_details(frame: &mut Frame, area: Rect, app: &App, palette: Palette) {
    let view = match app.mode() {
        Mode::Play => DetailsView::Play {
            puzzle: app.game().puzzle(),
            board: app.board(),
            selected: app.selected(),
            cursor: app.game_cursor(),
            events: app.game().events().len(),
        },
        Mode::Watch => DetailsView::Watch {
            puzzle: app.game().puzzle(),
            history: app.trace_history(),
            selected: app.selected(),
            solver: app.solver_name(),
            running: app.running(),
            events_per_tick: app.events_per_tick(),
        },
    };
    frame.render_widget(SearchDetails::new(view).with_palette(palette), area);
}

fn render_footer(frame: &mut Frame, area: Rect, app: &App, palette: Palette) {
    let selected = app.selected();
    let given = app.game().puzzle().cells()[selected.get()] != 0;
    let solved = app.mode() == Mode::Play && app.board().is_solution_for(app.game().puzzle());
    let cell = format!("r{} c{}", selected.row() + 1, selected.column() + 1);
    let context = match app.mode() {
        Mode::Play => format!(
            "{cell} · {}    {}/81 {} · move {}/{}",
            if given { "given / LOCKED" } else { "editable" },
            app.board().clue_count(),
            if solved { "SOLVED" } else { "filled" },
            app.game_cursor(),
            app.game().events().len(),
        ),
        Mode::Watch => format!(
            "{cell} · {}    {} · {} · event {}/{}",
            if given { "given / LOCKED" } else { "open" },
            app.solver_name(),
            watch_phase(app),
            app.trace_cursor(),
            app.trace_history().len(),
        ),
    };
    let context = if area.width < 68 {
        match app.mode() {
            Mode::Play => format!(
                "{cell} · {} · move {}/{}",
                if solved {
                    "SOLVED"
                } else if given {
                    "LOCKED"
                } else {
                    "editable"
                },
                app.game_cursor(),
                app.game().events().len()
            ),
            Mode::Watch => format!(
                "{cell} · event {}/{} · {}",
                app.trace_cursor(),
                app.trace_history().len(),
                watch_phase(app)
            ),
        }
    } else {
        context
    };
    let hints = match app.mode() {
        Mode::Play if area.width < 68 => "arrows / hjkl move · 1–9 enter",
        Mode::Play => "arrows / hjkl move   1–9 enter   0 clear   [ / ] history",
        Mode::Watch if area.width < 68 => "Space run · [ / ] step · s solver",
        Mode::Watch => "Space run / pause   [ / ] step   + / - speed   s solver",
    };
    let discovery = if area.width < 68 {
        "i details  t themes  ? help · q quit"
    } else {
        "Tab play / solver   i details   t themes   ? help   q quit"
    };
    let middle = if app.last_activation().is_some() || app.notice().severity() == Severity::Error {
        Line::styled(
            app.notice().message(),
            Style::default().fg(if app.notice().severity() == Severity::Error {
                palette.alert
            } else {
                palette.muted
            }),
        )
    } else {
        Line::styled(hints, Style::default().fg(palette.muted))
    };
    frame.render_widget(
        Paragraph::new(vec![
            Line::styled(context, Style::default().fg(palette.ink)),
            middle,
            Line::styled(discovery, Style::default().fg(palette.muted)),
        ])
        .alignment(Alignment::Center),
        area,
    );
}

fn watch_phase(app: &App) -> &'static str {
    if app.trace().finished().is_some() {
        "finished"
    } else if app.running() {
        "running"
    } else {
        match app.watch().run() {
            RunState::NotStarted => "ready",
            RunState::Starting(_) => "starting",
            RunState::Stopped(_) => "stopped",
            RunState::Active(_) | RunState::Finished(_) => "paused",
        }
    }
}

fn describe_event(event: &TraceEvent) -> String {
    match event {
        TraceEvent::Start { solver, .. } => format!("Started {solver}"),
        TraceEvent::Finished { result, .. } => {
            format!("Finished · {} solution(s)", result.solutions_found())
        }
        TraceEvent::Step { kind, .. } => match kind {
            TraceKind::Node => "Enter search node".to_owned(),
            TraceKind::Candidates => "Update candidate domains".to_owned(),
            TraceKind::Assignment {
                cell,
                digit,
                forced,
            } => format!(
                "{} r{} c{} = {}",
                if *forced { "Forced" } else { "Guess" },
                cell.row() + 1,
                cell.column() + 1,
                digit.get()
            ),
            TraceKind::Restore => "Restore parent state".to_owned(),
            TraceKind::Contradiction => "Contradiction · backtrack".to_owned(),
            TraceKind::Solution => "Solution found".to_owned(),
        },
    }
}

fn candidate_digits(mask: CandidateMask) -> String {
    let digits: String = mask
        .iter()
        .map(|digit| char::from(b'0' + digit.get()))
        .collect();
    if digits.is_empty() {
        "—".to_owned()
    } else {
        digits
    }
}

/// Compare masks recorded by the solver, without deriving Sudoku candidates.
fn candidate_changes(history: &TraceHistory, palette: Palette) -> Vec<Line<'static>> {
    let Some(TraceEvent::Step {
        kind: TraceKind::Candidates,
        snapshot,
    }) = current_event(history)
    else {
        return Vec::new();
    };
    let Some(current) = snapshot.candidates() else {
        return Vec::new();
    };
    let Some(previous) = history.previous_candidates() else {
        return vec![Line::styled(
            "Candidate domains initialized",
            Style::default().fg(palette.muted),
        )];
    };
    let count = current
        .iter()
        .zip(previous)
        .filter(|(after, before)| after != before)
        .count();
    let mut lines: Vec<_> = current
        .iter()
        .zip(previous)
        .enumerate()
        .filter(|(_, (after, before))| after != before)
        .take(2)
        .map(|(cell, (&after, &before))| {
            Line::styled(
                format!(
                    "r{}c{} {} → {}",
                    cell / 9 + 1,
                    cell % 9 + 1,
                    candidate_digits(before),
                    candidate_digits(after)
                ),
                Style::default().fg(palette.guess),
            )
        })
        .collect();
    if count > 2 {
        lines.push(Line::styled(
            format!("Changed {count} cells"),
            Style::default().fg(palette.muted),
        ));
    }
    lines
}

fn render_help(frame: &mut Frame, area: Rect, palette: Palette) {
    let popup = centered(area, 62, 23);
    frame.render_widget(Clear, popup);
    let lines = if popup.width < 62 || popup.height < 23 {
        vec![
            Line::styled(
                "PLAY THE GRID",
                Style::default()
                    .fg(palette.brand)
                    .add_modifier(Modifier::BOLD),
            ),
            Line::raw("arrows / hjkl  move"),
            Line::raw("1–9 enter · 0 / . / Del clear"),
            Line::raw("Backspace clears · n next puzzle"),
            Line::raw(""),
            Line::styled(
                "WATCH & REPLAY",
                Style::default()
                    .fg(palette.guess)
                    .add_modifier(Modifier::BOLD),
            ),
            Line::raw("Tab play / solver · i details"),
            Line::raw("Space run / pause · [ / ] step"),
            Line::raw("+ / - speed · Home / End history"),
            Line::raw("r rewind · s solver picker"),
            Line::raw("arrows choose · Enter accept"),
            Line::raw(""),
            Line::raw("t themes · ? / F1 help"),
            Line::raw("Esc close panel"),
            Line::raw("q / Ctrl+C quit"),
            Line::styled(
                "Replay never reruns search.",
                Style::default().fg(palette.muted),
            ),
        ]
    } else {
        vec![
            Line::styled(
                "PLAY THE GRID",
                Style::default()
                    .fg(palette.brand)
                    .add_modifier(Modifier::BOLD),
            ),
            Line::raw("Arrows / hjkl   Move between cells"),
            Line::raw("1–9             Enter a digit; givens stay locked"),
            Line::raw("0 / . / Del     Clear a digit (Backspace also works)"),
            Line::raw("n               Load next puzzle from this source"),
            Line::raw(""),
            Line::styled(
                "WATCH & REPLAY",
                Style::default()
                    .fg(palette.guess)
                    .add_modifier(Modifier::BOLD),
            ),
            Line::raw("Tab             Switch play / solver"),
            Line::raw("i               Toggle puzzle / search details"),
            Line::raw("Space           Run / pause the selected solver"),
            Line::raw("+ / -           Faster / slower playback"),
            Line::raw("[ / ]           Previous / next recorded event"),
            Line::raw("Home / End      First / latest recorded event"),
            Line::raw("r               Rewind active history"),
            Line::raw("s               Choose a solver; arrows + Enter"),
            Line::raw(""),
            Line::raw("t               Choose a theme; arrows + Enter"),
            Line::raw("? / F1          Toggle this help"),
            Line::raw("Esc             Close help, picker, or details"),
            Line::raw("q / Ctrl+C      Quit"),
            Line::styled(
                "Recorded history replays without rerunning search.",
                Style::default().fg(palette.muted),
            ),
        ]
    };
    frame.render_widget(
        Paragraph::new(lines)
            .style(Style::default().fg(palette.ink).bg(palette.table))
            .wrap(Wrap { trim: false })
            .block(
                Block::bordered()
                    .border_type(BorderType::Double)
                    .border_style(Style::default().fg(palette.grid))
                    .title(" KU / CONTROLS "),
            ),
        popup,
    );
}

fn render_picker(frame: &mut Frame, area: Rect, app: &App, palette: Palette) {
    let popup = centered(area, 46, 12);
    frame.render_widget(Clear, popup);
    let mut lines = vec![
        Line::styled(
            "Choose existing search strategy",
            Style::default().fg(palette.muted),
        ),
        Line::raw(""),
    ];
    for (index, solver) in app.solvers().iter().enumerate() {
        lines.push(Line::styled(
            format!(
                " {} {}",
                if Some(index) == app.picker_index() {
                    "›"
                } else {
                    " "
                },
                solver.name()
            ),
            if Some(index) == app.picker_index() {
                Style::default()
                    .fg(palette.selection_fg)
                    .bg(palette.selection_bg)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            },
        ));
    }
    lines.push(Line::raw(""));
    lines.push(Line::styled(
        "↑ / ↓ choose · Enter accept · Esc cancel",
        Style::default().fg(palette.muted),
    ));
    frame.render_widget(
        Paragraph::new(lines)
            .style(Style::default().fg(palette.ink).bg(palette.table))
            .block(
                Block::bordered()
                    .border_style(Style::default().fg(palette.guess))
                    .title(" SOLVER REGISTRY "),
            ),
        popup,
    );
}

fn render_theme_picker(frame: &mut Frame, area: Rect, app: &App, palette: Palette) {
    let popup = centered(area, 54, 22);
    frame.render_widget(Clear, popup);
    let block = Block::bordered()
        .style(Style::default().fg(palette.ink).bg(palette.table))
        .border_style(Style::default().fg(palette.grid))
        .title(" THEMES ");
    let inner = block.inner(popup);
    frame.render_widget(block, popup);
    let [header, rows, footer] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Min(1),
        Constraint::Length(2),
    ])
    .areas(inner);
    frame.render_widget(
        Paragraph::new(format!("Current: {}", app.theme().label))
            .style(Style::default().fg(palette.muted).bg(palette.table)),
        header,
    );
    let catalog = app.theme_catalog();
    let selected = app.theme_picker_index().unwrap_or(0);
    let visible = usize::from(rows.height);
    let first = selected
        .saturating_sub(visible / 2)
        .min(catalog.len().saturating_sub(visible));
    for (offset, (index, theme)) in catalog
        .iter()
        .enumerate()
        .skip(first)
        .take(visible)
        .enumerate()
    {
        let selected_row = index == selected;
        let style = if selected_row {
            Style::default()
                .fg(palette.selection_fg)
                .bg(palette.selection_bg)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(palette.ink).bg(palette.table)
        };
        let row = Rect::new(rows.x, rows.y + offset as u16, rows.width, 1);
        frame.render_widget(
            Paragraph::new(format!(
                " {} {} · {}",
                if selected_row { "›" } else { " " },
                theme.label,
                theme.id,
            ))
            .style(style),
            row,
        );
    }
    let hints = if footer.width < 40 {
        vec![
            Line::raw("↑ / ↓ choose · Enter apply"),
            Line::raw(format!("Esc cancel · {} palettes", catalog.len())),
        ]
    } else {
        vec![
            Line::raw("↑ / ↓ choose · Enter apply · Esc cancel"),
            Line::raw(format!("{} palettes · applies on Enter", catalog.len())),
        ]
    };
    frame.render_widget(
        Paragraph::new(hints).style(Style::default().fg(palette.muted).bg(palette.table)),
        footer,
    );
}

fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    )
}

#[cfg(test)]
mod tests {
    use ku_core::{Digit, HistoryLimits, SearchStats, TraceSnapshot};

    use ratatui::{Terminal, backend::TestBackend, style::Color};

    use super::*;
    use crate::tui::input::Action;

    fn puzzle() -> Board {
        "530070000600195000098000060800060003400803001700020006060000280000419005000080079"
            .parse()
            .unwrap()
    }

    fn activate(app: &mut App, job: ku_session::protocol::Job) {
        use ku_session::protocol::{Activation, ActivationId};
        let id = app
            .last_activation()
            .map_or(Ok(ActivationId::FIRST), ActivationId::checked_next)
            .unwrap();
        let activation = Activation::new(id, app.logical_time(), vec![job]).unwrap();
        app.activate(&activation).unwrap();
    }

    fn app() -> App {
        crate::tui::app::new(vec![puzzle().into()], 0, "fixture".into(), "bitmask").unwrap()
    }

    fn draw(app: &App, width: u16, height: u16) -> Terminal<TestBackend> {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| render(frame, app)).unwrap();
        terminal
    }

    fn draw_history(history: &TraceHistory) -> Terminal<TestBackend> {
        let puzzle = puzzle();
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal
            .draw(|frame| {
                frame.render_widget(
                    SearchDetails::new(DetailsView::Watch {
                        puzzle: &puzzle,
                        history,
                        selected: CellIndex::try_from(2).unwrap(),
                        solver: "scripted",
                        running: false,
                        events_per_tick: 8,
                    }),
                    frame.area(),
                );
            })
            .unwrap();
        terminal
    }

    fn history() -> TraceHistory {
        let puzzle = puzzle();
        let mut history = TraceHistory::new(puzzle.clone(), "scripted", HistoryLimits::default());
        history
            .push(TraceEvent::Start {
                solver: "scripted".to_owned(),
                puzzle,
            })
            .unwrap();
        history
    }

    fn screen(terminal: &Terminal<TestBackend>) -> String {
        terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect()
    }

    fn main_grid(width: u16, height: u16) -> (GridGeometry, Rect) {
        let body = Rect::new(0, 2, width, height - 5);
        let geometry = GridGeometry::for_area(body);
        (
            geometry,
            centered(body, geometry.width(), geometry.height()),
        )
    }

    fn cell_position(geometry: GridGeometry, grid: Rect, cell: CellIndex) -> (u16, u16) {
        let (x, y) = geometry.cell_center(cell);
        (grid.x + x, grid.y + y)
    }

    #[test]
    fn default_board_keeps_resting_content_neutral_and_only_colors_selection() {
        let mut app = app();
        activate(&mut app, Action::Digit(Digit::try_from(4).unwrap()).into());
        activate(
            &mut app,
            Action::Move {
                rows: 0,
                columns: 1,
            }
            .into(),
        );
        let terminal = draw(&app, 90, 26);
        let palette = Palette::default();
        let (geometry, grid) = main_grid(90, 26);
        let entered = cell_position(geometry, grid, CellIndex::try_from(2).unwrap());
        let buffer = terminal.backend().buffer();
        assert_eq!(buffer[entered].symbol(), "4");
        assert_eq!(buffer[entered].fg, palette.ink);
        let selected = cell_position(geometry, grid, app.selected());
        assert_eq!(buffer[selected].bg, Color::Rgb(42, 51, 79));
        assert_eq!(buffer[selected].fg, Color::Rgb(245, 244, 239));
        for cell in &buffer.content {
            assert!(
                matches!(cell.fg, Color::Reset | Color::DarkGray)
                    || cell.fg == palette.selection_fg
            );
            assert!(cell.bg == Color::Reset || cell.bg == palette.selection_bg);
        }
    }

    #[test]
    fn centered_board_is_the_default_focus_at_normal_and_compact_sizes() {
        let app = app();
        let checkpoint = app.checkpoint();
        for (width, height) in [
            (37, 22),
            (52, 22),
            (54, 22),
            (55, 22),
            (80, 24),
            (90, 26),
            (120, 40),
        ] {
            let terminal = draw(&app, width, height);
            let buffer = terminal.backend().buffer();
            let output = screen(&terminal);
            let (geometry, grid) = main_grid(width, height);
            assert_eq!(buffer[(grid.x + 3, grid.y + 1)].symbol(), "┏");
            assert_eq!(
                buffer[(grid.x + geometry.width() - 4, grid.y + 1)].symbol(),
                "┓"
            );
            assert_eq!(buffer[(grid.x + 3, grid.bottom() - 1)].symbol(), "┗");
            assert!((grid.x as i32 - (width - grid.width) as i32 / 2).abs() <= 1);
            assert!(output.contains("i details"));
            assert!(!output.contains("YOUR PUZZLE"));
            assert!(!output.contains("DETAILS ·"));
            assert!(!output.contains("GRID /"));
            assert!(!output.contains("╭"));
            assert_eq!(geometry.cell_width, if width < 55 { 3 } else { 5 });
            assert_eq!(geometry.cell_height, if height == 40 { 3 } else { 1 });
        }
        assert!(checkpoint.matches(&app));
    }

    #[test]
    fn givens_cursor_and_invalid_move_feedback_render_distinctly() {
        let mut app = app();
        let (geometry, grid) = main_grid(80, 24);
        let given = cell_position(geometry, grid, CellIndex::FIRST);
        let selected = cell_position(geometry, grid, app.selected());
        let terminal = draw(&app, 80, 24);
        let buffer = terminal.backend().buffer();
        assert_eq!(buffer[given].symbol(), "5");
        assert!(buffer[given].modifier.contains(Modifier::BOLD));
        assert_eq!(buffer[selected].symbol(), "·");
        assert_eq!(buffer[selected].bg, Palette::default().selection_bg);
        activate(
            &mut app,
            Action::Move {
                rows: 0,
                columns: -2,
            }
            .into(),
        );
        activate(&mut app, Action::Clear.into());
        let terminal = draw(&app, 80, 24);
        assert_eq!(
            terminal.backend().buffer()[given].bg,
            Palette::default().alert
        );
        assert!(screen(&terminal).contains("given clue and cannot be changed"));
    }

    #[test]
    fn selected_given_preserves_its_digit_and_lock_status_in_all_layouts() {
        let mut app = app();
        activate(
            &mut app,
            Action::Move {
                rows: 0,
                columns: -2,
            }
            .into(),
        );
        for (width, height) in [(37, 22), (80, 24), (90, 26), (120, 40)] {
            let terminal = draw(&app, width, height);
            let output = screen(&terminal);
            assert!(
                output.contains("[5]"),
                "missing given digit at {width}×{height}"
            );
            assert!(output.contains("r1 c1"));
            assert!(output.contains("LOCKED"));
            let (geometry, grid) = main_grid(width, height);
            assert_eq!(
                terminal.backend().buffer()[cell_position(geometry, grid, CellIndex::FIRST)]
                    .symbol(),
                "5"
            );
        }
    }

    #[test]
    fn editable_selection_keeps_plain_digit_and_emphasizes_its_row_and_column() {
        let app = app();
        let terminal = draw(&app, 90, 26);
        let output = screen(&terminal);
        let buffer = terminal.backend().buffer();
        let (geometry, grid) = main_grid(90, 26);
        let selected = cell_position(geometry, grid, app.selected());
        assert!(!output.contains("LOCKED"));
        assert_eq!(buffer[selected].symbol(), "·");
        assert_eq!(buffer[selected].bg, Palette::default().selection_bg);
        assert_eq!(buffer[(selected.0 - 1, selected.1)].symbol(), " ");
        assert_eq!(buffer[(selected.0 + 1, selected.1)].symbol(), " ");
        assert_eq!(
            buffer[cell_position(geometry, grid, CellIndex::FIRST)].bg,
            Palette::default().peer
        );
        assert_eq!(
            buffer[cell_position(geometry, grid, CellIndex::try_from(11).unwrap())].bg,
            Palette::default().peer
        );
        assert_eq!(
            buffer[cell_position(geometry, grid, CellIndex::try_from(10).unwrap())].bg,
            Palette::default().table
        );
    }

    #[test]
    fn explicit_details_show_a_lock_as_a_drawer_or_a_wide_side_rail() {
        let mut app = app();
        activate(
            &mut app,
            Action::Move {
                rows: 0,
                columns: -2,
            }
            .into(),
        );
        activate(&mut app, Action::ToggleSidebar.into());
        let checkpoint = app.checkpoint();
        for (width, height) in [(37, 22), (80, 24), (90, 26), (120, 40)] {
            let output = screen(&draw(&app, width, height));
            assert!(output.contains("DETAILS · i / Esc close"));
            assert!(output.contains("YOUR PUZZLE"));
            assert!(output.contains("GIVEN / LOCKED"));
            if height >= 24 {
                for lock_part in ["╭─╮", "╭┴─┴╮", "│ • │", "╰───╯"]
                {
                    assert!(output.contains(lock_part), "missing lock part: {lock_part}");
                }
            } else {
                assert!(!output.contains("╭┴─┴╮"));
            }
            if width >= 89 {
                assert!(output.contains("[5]"));
            }
        }
        assert!(checkpoint.matches(&app));
        activate(&mut app, Action::Escape.into());
        let output = screen(&draw(&app, 90, 26));
        assert!(!output.contains("DETAILS ·"));
        assert!(output.contains("[5]"));
    }

    #[test]
    fn selected_given_is_marked_in_solver_mode_without_showing_details_by_default() {
        let mut app = app();
        activate(&mut app, Action::ToggleMode.into());
        activate(
            &mut app,
            Action::Move {
                rows: 0,
                columns: -2,
            }
            .into(),
        );
        let output = screen(&draw(&app, 90, 26));
        assert!(output.contains("[5]"));
        assert!(output.contains("LOCKED"));
        assert!(output.contains("bitmask"));
        assert!(output.contains("event 0/0"));
        assert!(!output.contains("SEARCH OBSERVATORY"));
        activate(&mut app, Action::ToggleSidebar.into());
        let output = screen(&draw(&app, 90, 26));
        assert!(output.contains("SEARCH OBSERVATORY"));
        assert!(!output.contains("Cannot edit"));
    }

    #[test]
    fn standalone_grid_centers_borrowed_domain_data_inside_offset_viewports() {
        let puzzle = puzzle();
        let mut board = *puzzle.cells();
        board[2] = 4;
        let board = Board::try_from(board).unwrap();
        let area = Rect::new(4, 5, 45, 20);
        let geometry = GridGeometry::for_area(area);
        let grid = centered(area, geometry.width(), geometry.height());
        let mut buffer = Buffer::empty(area);
        SudokuBoard::new(
            BoardView::new(&board, &puzzle, CellIndex::try_from(3).unwrap())
                .with_guess(Some(CellIndex::try_from(2).unwrap())),
        )
        .render(area, &mut buffer);
        let guessed = cell_position(geometry, grid, CellIndex::try_from(2).unwrap());
        assert_eq!(buffer[guessed].symbol(), "4");
        assert_eq!(buffer[guessed].fg, Palette::default().guess);
        assert_eq!(
            buffer[cell_position(geometry, grid, CellIndex::try_from(3).unwrap())].bg,
            Palette::default().selection_bg
        );
        assert_eq!(buffer[(48, 24)].symbol(), " ");
        assert_eq!(puzzle.cells()[2], 0);
    }

    #[test]
    fn watch_shows_candidates_and_work_from_the_recorded_state() {
        let mut history = history();
        let mut candidates = [CandidateMask::default(); 81];
        candidates[2] = CandidateMask::try_from((1 << 4) | (1 << 8)).unwrap();
        history
            .push(TraceEvent::Step {
                kind: TraceKind::Candidates,
                snapshot: TraceSnapshot::new(
                    puzzle(),
                    Some(candidates),
                    SearchStats {
                        nodes: 7,
                        guesses: 2,
                        ..SearchStats::default()
                    },
                    3,
                    CellIndex::try_from(2).unwrap(),
                ),
            })
            .unwrap();
        history.seek(history.len()).unwrap();
        let output = screen(&draw_history(&history));
        assert!(output.contains("Candidates 48"));
        assert!(output.contains("Nodes      7"));
        assert!(output.contains("Depth      3"));
        assert!(output.contains("Update candidate domains"));
    }

    #[test]
    fn candidate_changes_compare_exposed_history_masks_at_the_replay_cursor() {
        let mut history = history();
        let mut before = [CandidateMask::default(); 81];
        before[2] = CandidateMask::try_from((1 << 4) | (1 << 8)).unwrap();
        before[3] = CandidateMask::try_from((1 << 1) | (1 << 2)).unwrap();
        before[5] = CandidateMask::try_from((1 << 5) | (1 << 9)).unwrap();
        let mut after = before;
        after[2] = CandidateMask::try_from(1 << 4).unwrap();
        after[3] = CandidateMask::try_from(1 << 2).unwrap();
        after[5] = CandidateMask::try_from(1 << 9).unwrap();
        for candidates in [before, after] {
            history
                .push(TraceEvent::Step {
                    kind: TraceKind::Candidates,
                    snapshot: TraceSnapshot::new(
                        puzzle(),
                        Some(candidates),
                        SearchStats::default(),
                        0,
                        CellIndex::try_from(2).unwrap(),
                    ),
                })
                .unwrap();
        }
        history.seek(3).unwrap();
        let output = screen(&draw_history(&history));
        assert!(output.contains("r1c3 48 → 4"));
        assert!(output.contains("r1c4 12 → 2"));
        assert!(output.contains("Changed 3 cells"));
        assert!(!output.contains("r1c6 59 → 9"));
        history.seek(2).unwrap();
        let output = screen(&draw_history(&history));
        assert!(output.contains("Candidate domains initialized"));
        assert!(output.contains("Candidates 48"));
        assert!(!output.contains("r1c3 48 → 4"));
    }

    #[test]
    fn compact_screen_retains_help_quit_and_active_mode() {
        let mut app = app();
        let output = screen(&draw(&app, 37, 22));
        assert!(output.contains("PLAY"));
        assert!(output.contains("? help · q quit"));
        activate(&mut app, Action::ToggleMode.into());
        assert!(screen(&draw(&app, 37, 22)).contains("SOLVER"));
        assert!(screen(&draw(&app, 30, 10)).contains("Resize to at least 37 × 22."));
    }

    #[test]
    fn solved_play_status_and_success_notices_remain_visible_without_details() {
        let puzzle: Board =
            "034678912672195348198342567859761423426853791713924856961537284287419635345286179"
                .parse()
                .unwrap();
        let mut app =
            crate::tui::app::new(vec![puzzle.into()], 0, "fixture".into(), "bitmask").unwrap();
        activate(&mut app, Action::Digit(Digit::try_from(5).unwrap()).into());
        for (width, height) in [(37, 22), (80, 24), (120, 40)] {
            let output = screen(&draw(&app, width, height));
            assert!(output.contains("SOLVED"));
            assert!(output.contains("Puzzle complete!"));
            assert!(!output.contains("DETAILS ·"));
        }
        activate(&mut app, Action::NextPuzzle.into());
        assert!(screen(&draw(&app, 80, 24)).contains("Loaded puzzle 1/1."));
    }

    #[test]
    fn help_covers_details_and_picker_with_readable_compact_quit_controls() {
        let mut app = app();
        activate(&mut app, Action::ToggleSidebar.into());
        activate(&mut app, Action::ChooseSolver.into());
        activate(&mut app, Action::Help.into());
        for (width, height) in [(37, 22), (80, 24), (120, 40)] {
            let output = screen(&draw(&app, width, height));
            assert!(output.contains("KU / CONTROLS"));
            assert!(output.contains("Ctrl+C"));
            assert!(output.contains("Esc"));
            assert!(output.contains("details"));
            assert!(!output.contains("SOLVER REGISTRY"));
        }
    }

    #[test]
    fn help_overlay_lists_complete_controls() {
        let mut app = app();
        activate(&mut app, Action::Help.into());
        let output = screen(&draw(&app, 80, 24));
        for control in [
            "PLAY THE GRID",
            "WATCH & REPLAY",
            "Faster / slower",
            "First / latest",
            "Ctrl+C",
            "without rerunning",
        ] {
            assert!(output.contains(control), "missing help control: {control}");
        }
    }

    #[test]
    fn selected_palette_styles_board_and_invalid_move_feedback() {
        let config =
            crate::tui::theme::configure(app().config().clone(), "catppuccin-latte").unwrap();
        let mut app = App::from_config(config);
        let palette = Palette::for_app(&app);
        let (geometry, grid) = main_grid(90, 26);
        let terminal = draw(&app, 90, 26);
        let buffer = terminal.backend().buffer();
        let selected = cell_position(geometry, grid, app.selected());
        assert_eq!(buffer[(0, 0)].bg, palette.table);
        assert_eq!(buffer[selected].bg, palette.selection_bg);
        assert_eq!(buffer[selected].fg, palette.selection_fg);
        assert_eq!(
            buffer[cell_position(geometry, grid, CellIndex::FIRST)].fg,
            palette.ink
        );

        activate(
            &mut app,
            Action::Move {
                rows: 0,
                columns: -2,
            }
            .into(),
        );
        activate(&mut app, Action::Clear.into());
        let terminal = draw(&app, 90, 26);
        let selected = cell_position(geometry, grid, app.selected());
        assert_eq!(terminal.backend().buffer()[selected].bg, palette.alert);
        assert_eq!(terminal.backend().buffer()[selected].fg, palette.error_fg);
        assert!(screen(&terminal).contains("given clue and cannot be changed"));
    }

    #[test]
    fn standalone_details_use_palette_for_the_whole_panel_and_progress_rail() {
        let puzzle = puzzle();
        let palette = Palette::for_id("dracula");
        let area = Rect::new(4, 5, 40, 20);
        let mut buffer = Buffer::empty(area);
        SearchDetails::new(DetailsView::Play {
            puzzle: &puzzle,
            board: &puzzle,
            selected: CellIndex::FIRST,
            cursor: 0,
            events: 0,
        })
        .with_palette(palette)
        .render(area, &mut buffer);
        assert_eq!(buffer[(area.x, area.y)].bg, palette.table);
        assert_eq!(buffer[(area.x, area.y)].fg, palette.grid);
        assert_eq!(buffer[(area.x + 1, area.y + 1)].fg, palette.brand);
        assert!(buffer.content.iter().all(|cell| cell.bg == palette.table));
    }

    #[test]
    fn theme_picker_scrolls_selected_palette_into_view_and_help_covers_it() {
        let config = crate::tui::theme::configure(app().config().clone(), "dracula").unwrap();
        let mut app = App::from_config(config);
        let palette = Palette::for_app(&app);
        activate(&mut app, Action::ChooseTheme.into());
        activate(
            &mut app,
            Action::Move {
                rows: 127,
                columns: 0,
            }
            .into(),
        );
        let selected = app.theme_picker_index().unwrap();
        assert_eq!(selected, app.theme_catalog().len() - 1);
        let last_label = app.theme_catalog()[selected].label.clone();
        let checkpoint = app.checkpoint();
        for (width, height) in [(37, 22), (90, 22), (90, 26)] {
            let terminal = draw(&app, width, height);
            let output = screen(&terminal);
            assert!(output.contains("THEMES"));
            assert!(output.contains(&last_label));
            assert!(output.contains("Enter apply"));
            assert!(output.contains("Esc cancel"));
            let selection = terminal
                .backend()
                .buffer()
                .content
                .iter()
                .find(|cell| cell.symbol() == "›")
                .unwrap();
            assert_eq!(selection.bg, palette.selection_bg);
            assert_eq!(selection.fg, palette.selection_fg);
            let popup = centered(Rect::new(0, 0, width, height), 54, 22);
            assert_eq!(
                terminal.backend().buffer()[(popup.x, popup.y)].bg,
                palette.table
            );
        }
        assert!(checkpoint.matches(&app));
        activate(&mut app, Action::Help.into());
        for (width, height) in [(37, 22), (90, 22), (90, 26)] {
            let terminal = draw(&app, width, height);
            let output = screen(&terminal);
            assert!(output.contains("KU / CONTROLS"));
            assert!(output.contains("Ctrl+C"));
            assert!(output.contains("theme"));
            assert!(!output.contains("Current:"));
            let popup = centered(Rect::new(0, 0, width, height), 62, 23);
            assert_eq!(
                terminal.backend().buffer()[(popup.x, popup.y)].bg,
                palette.table
            );
            assert_eq!(
                terminal.backend().buffer()[(popup.x, popup.y)].fg,
                palette.grid
            );
        }
        activate(&mut app, Action::Escape.into());
        assert_eq!(app.theme_picker_index(), Some(selected));
    }

    #[test]
    fn assignment_description_uses_validated_digit_and_coordinates() {
        let mut cells = *puzzle().cells();
        cells[2] = 4;
        let event = TraceEvent::Step {
            kind: TraceKind::Assignment {
                cell: CellIndex::try_from(2).unwrap(),
                digit: Digit::try_from(4).unwrap(),
                forced: false,
            },
            snapshot: TraceSnapshot::new(
                Board::try_from(cells).unwrap(),
                None,
                SearchStats::default(),
                0,
                CellIndex::try_from(2).unwrap(),
            ),
        };
        assert_eq!(describe_event(&event), "Guess r1 c3 = 4");
    }

    #[test]
    fn overlays_fit_small_and_offset_viewports() {
        assert_eq!(
            centered(Rect::new(4, 5, 30, 10), 62, 21),
            Rect::new(4, 5, 30, 10)
        );
        assert_eq!(
            centered(Rect::new(4, 5, 80, 24), 40, 10),
            Rect::new(24, 12, 40, 10)
        );
    }
}
