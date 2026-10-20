//! Checked wire representations. Core and model types are never deserialized directly.
use std::{collections::BTreeMap, num::NonZeroUsize};

use ku_core::{
    Board, CandidateMask, CellIndex, Digit, SearchExtent, SearchResult, SearchStats, TraceEvent,
    TraceKind, TraceSnapshot,
};
use serde::{Deserialize, Serialize};

use crate::{action::Action, protocol::*};

type Checked<T> = Result<T, String>;
fn domain(error: impl std::fmt::Display) -> String {
    error.to_string()
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StatsWire {
    nodes: u64,
    guesses: u64,
    backtracks: u64,
    forced_moves: u64,
    max_depth: usize,
}
impl From<SearchStats> for StatsWire {
    fn from(s: SearchStats) -> Self {
        Self {
            nodes: s.nodes,
            guesses: s.guesses,
            backtracks: s.backtracks,
            forced_moves: s.forced_moves,
            max_depth: s.max_depth,
        }
    }
}
impl From<StatsWire> for SearchStats {
    fn from(s: StatsWire) -> Self {
        Self {
            nodes: s.nodes,
            guesses: s.guesses,
            backtracks: s.backtracks,
            forced_moves: s.forced_moves,
            max_depth: s.max_depth,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum ExtentWire {
    Exhausted,
    Capped { limit: usize },
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ResultWire {
    solution: Option<String>,
    solutions_found: usize,
    extent: ExtentWire,
    stats: StatsWire,
}
impl ResultWire {
    pub(crate) fn from_domain(result: &SearchResult) -> Self {
        Self {
            solution: result.solution().map(ToString::to_string),
            solutions_found: result.solutions_found(),
            extent: match result.extent() {
                SearchExtent::Exhausted => ExtentWire::Exhausted,
                SearchExtent::Capped(limit) => ExtentWire::Capped { limit: limit.get() },
            },
            stats: result.stats().into(),
        }
    }
    pub(crate) fn into_domain(self, puzzle: &Board) -> Checked<SearchResult> {
        let extent = match self.extent {
            ExtentWire::Exhausted => SearchExtent::Exhausted,
            ExtentWire::Capped { limit } => SearchExtent::Capped(
                NonZeroUsize::new(limit).ok_or("solution cap must be nonzero")?,
            ),
        };
        let solution = self
            .solution
            .map(|text| text.parse::<Board>().map_err(domain))
            .transpose()?;
        SearchResult::new(
            puzzle,
            solution,
            self.solutions_found,
            extent,
            self.stats.into(),
        )
        .map_err(domain)
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SnapshotWire {
    board: String,
    candidates: Option<Vec<u16>>,
    stats: StatsWire,
    depth: usize,
    focus: usize,
}
impl SnapshotWire {
    pub(crate) fn from_domain(snapshot: &TraceSnapshot) -> Self {
        Self {
            board: snapshot.board().to_string(),
            candidates: snapshot
                .candidates()
                .map(|masks| masks.iter().map(|mask| mask.bits()).collect()),
            stats: snapshot.stats().into(),
            depth: snapshot.depth(),
            focus: snapshot.focus().get(),
        }
    }
    pub(crate) fn into_domain(self, puzzle: &Board) -> Checked<TraceSnapshot> {
        let board: Board = self.board.parse().map_err(domain)?;
        if puzzle
            .cells()
            .iter()
            .zip(board.cells())
            .any(|(&given, &value)| given != 0 && given != value)
        {
            return Err("snapshot changes puzzle givens".into());
        }
        let candidates = self
            .candidates
            .map(|masks| {
                let masks: Vec<_> = masks
                    .into_iter()
                    .map(|mask| CandidateMask::try_from(mask).map_err(domain))
                    .collect::<Checked<_>>()?;
                masks.try_into().map_err(|_: Vec<CandidateMask>| {
                    "candidate array must contain 81 masks".to_owned()
                })
            })
            .transpose()?;
        Ok(TraceSnapshot::new(
            board,
            candidates,
            self.stats.into(),
            self.depth,
            CellIndex::try_from(self.focus).map_err(domain)?,
        ))
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum KindWire {
    Node,
    Candidates,
    Assignment {
        cell: usize,
        digit: u8,
        forced: bool,
    },
    Restore,
    Contradiction,
    Solution,
}
impl KindWire {
    fn from_domain(kind: TraceKind) -> Self {
        match kind {
            TraceKind::Node => Self::Node,
            TraceKind::Candidates => Self::Candidates,
            TraceKind::Assignment {
                cell,
                digit,
                forced,
            } => Self::Assignment {
                cell: cell.get(),
                digit: digit.get(),
                forced,
            },
            TraceKind::Restore => Self::Restore,
            TraceKind::Contradiction => Self::Contradiction,
            TraceKind::Solution => Self::Solution,
        }
    }
    fn into_domain(self) -> Checked<TraceKind> {
        Ok(match self {
            Self::Node => TraceKind::Node,
            Self::Candidates => TraceKind::Candidates,
            Self::Assignment {
                cell,
                digit,
                forced,
            } => TraceKind::Assignment {
                cell: CellIndex::try_from(cell).map_err(domain)?,
                digit: Digit::try_from(digit).map_err(domain)?,
                forced,
            },
            Self::Restore => TraceKind::Restore,
            Self::Contradiction => TraceKind::Contradiction,
            Self::Solution => TraceKind::Solution,
        })
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum TraceWire {
    Start {
        solver: String,
        puzzle: String,
    },
    Step {
        kind: KindWire,
        snapshot: SnapshotWire,
    },
    Finished {
        result: ResultWire,
        snapshot: SnapshotWire,
    },
}
impl TraceWire {
    pub(crate) fn from_domain(event: &TraceEvent) -> Self {
        match event {
            TraceEvent::Start { solver, puzzle } => Self::Start {
                solver: solver.to_string(),
                puzzle: puzzle.to_string(),
            },
            TraceEvent::Step { kind, snapshot } => Self::Step {
                kind: KindWire::from_domain(*kind),
                snapshot: SnapshotWire::from_domain(snapshot),
            },
            TraceEvent::Finished { result, snapshot } => Self::Finished {
                result: ResultWire::from_domain(result),
                snapshot: SnapshotWire::from_domain(snapshot),
            },
        }
    }
    pub(crate) fn into_domain(self, puzzle: &Board) -> Checked<TraceEvent> {
        Ok(match self {
            Self::Start {
                solver,
                puzzle: text,
            } => {
                let _: SolverId = solver.clone().try_into().map_err(domain)?;
                let declared: Board = text.parse().map_err(domain)?;
                if &declared != puzzle {
                    return Err("trace start does not match puzzle".into());
                }
                TraceEvent::Start {
                    solver,
                    puzzle: declared,
                }
            }
            Self::Step { kind, snapshot } => {
                let kind = kind.into_domain()?;
                let snapshot = snapshot.into_domain(puzzle)?;
                if let TraceKind::Assignment { cell, digit, .. } = kind
                    && snapshot.cells()[cell.get()] != digit.get()
                {
                    return Err("assignment does not match snapshot".into());
                }
                if kind == TraceKind::Solution && !snapshot.board().is_solution_for(puzzle) {
                    return Err("solution snapshot is incomplete".into());
                }
                TraceEvent::Step { kind, snapshot }
            }
            Self::Finished { result, snapshot } => {
                let result = result.into_domain(puzzle)?;
                let snapshot = snapshot.into_domain(puzzle)?;
                validate_finish(&result, &snapshot)?;
                TraceEvent::Finished { result, snapshot }
            }
        })
    }
}
fn validate_finish(result: &SearchResult, snapshot: &TraceSnapshot) -> Checked<()> {
    if result.stats() != snapshot.stats()
        || result
            .solution()
            .is_some_and(|solution| solution != snapshot.board())
    {
        Err("finish snapshot does not match result".into())
    } else {
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum ActionWire {
    Move { rows: i8, columns: i8 },
    Digit { digit: u8 },
    Clear,
    ToggleMode,
    ToggleSidebar,
    ToggleRun,
    Faster,
    Slower,
    StepForward,
    StepBack,
    HistoryStart,
    HistoryEnd,
    Reset,
    ChooseSolver,
    ChooseTheme,
    Accept,
    Escape,
    NextPuzzle,
    Help,
    Quit,
    Tick,
    Resize,
}
impl ActionWire {
    fn from_domain(action: Action) -> Self {
        match action {
            Action::Move { rows, columns } => Self::Move { rows, columns },
            Action::Digit(digit) => Self::Digit { digit: digit.get() },
            Action::Clear => Self::Clear,
            Action::ToggleMode => Self::ToggleMode,
            Action::ToggleSidebar => Self::ToggleSidebar,
            Action::ToggleRun => Self::ToggleRun,
            Action::Faster => Self::Faster,
            Action::Slower => Self::Slower,
            Action::StepForward => Self::StepForward,
            Action::StepBack => Self::StepBack,
            Action::HistoryStart => Self::HistoryStart,
            Action::HistoryEnd => Self::HistoryEnd,
            Action::Reset => Self::Reset,
            Action::ChooseSolver => Self::ChooseSolver,
            Action::ChooseTheme => Self::ChooseTheme,
            Action::Accept => Self::Accept,
            Action::Escape => Self::Escape,
            Action::NextPuzzle => Self::NextPuzzle,
            Action::Help => Self::Help,
            Action::Quit => Self::Quit,
            Action::Tick => Self::Tick,
            Action::Resize => Self::Resize,
        }
    }
    fn into_domain(self) -> Checked<Action> {
        Ok(match self {
            Self::Move { rows, columns } => Action::Move { rows, columns },
            Self::Digit { digit } => Action::Digit(Digit::try_from(digit).map_err(domain)?),
            Self::Clear => Action::Clear,
            Self::ToggleMode => Action::ToggleMode,
            Self::ToggleSidebar => Action::ToggleSidebar,
            Self::ToggleRun => Action::ToggleRun,
            Self::Faster => Action::Faster,
            Self::Slower => Action::Slower,
            Self::StepForward => Action::StepForward,
            Self::StepBack => Action::StepBack,
            Self::HistoryStart => Action::HistoryStart,
            Self::HistoryEnd => Action::HistoryEnd,
            Self::Reset => Action::Reset,
            Self::ChooseSolver => Action::ChooseSolver,
            Self::ChooseTheme => Action::ChooseTheme,
            Self::Accept => Action::Accept,
            Self::Escape => Action::Escape,
            Self::NextPuzzle => Action::NextPuzzle,
            Self::Help => Action::Help,
            Self::Quit => Action::Quit,
            Self::Tick => Action::Tick,
            Self::Resize => Action::Resize,
        })
    }
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum FailureKindWire {
    Start,
    Execution,
    ProducerClosed,
    Protocol,
}
impl From<FailureKind> for FailureKindWire {
    fn from(kind: FailureKind) -> Self {
        match kind {
            FailureKind::Start => Self::Start,
            FailureKind::Execution => Self::Execution,
            FailureKind::ProducerClosed => Self::ProducerClosed,
            FailureKind::Protocol => Self::Protocol,
        }
    }
}
impl From<FailureKindWire> for FailureKind {
    fn from(kind: FailureKindWire) -> Self {
        match kind {
            FailureKindWire::Start => Self::Start,
            FailureKindWire::Execution => Self::Execution,
            FailureKindWire::ProducerClosed => Self::ProducerClosed,
            FailureKindWire::Protocol => Self::Protocol,
        }
    }
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum EndWire {
    Quit,
    InputEnded,
    Error,
    Dropped,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum JobWire {
    Action {
        action: ActionWire,
    },
    TimerFired {
        command: u64,
    },
    SolverStarted {
        run: u64,
        command: u64,
    },
    SolverEvents {
        run: u64,
        command: u64,
        events: Vec<TraceWire>,
    },
    SolverCompleted {
        run: u64,
        command: u64,
        result: ResultWire,
        snapshot: SnapshotWire,
    },
    SolverFailed {
        run: u64,
        command: u64,
        kind: FailureKindWire,
        message: String,
    },
    CommandFailed {
        command: u64,
        kind: FailureKindWire,
        message: String,
    },
    SolverCancelled {
        run: u64,
        command: u64,
    },
    SessionEnded {
        reason: EndWire,
    },
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum CommandKindWire {
    StartSolver {
        run: u64,
        solver: String,
        puzzle: String,
    },
    CancelSolver {
        run: u64,
    },
    ScheduleTimer {
        deadline_ms: u64,
    },
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CommandWire {
    id: u64,
    kind: CommandKindWire,
}
impl CommandWire {
    pub(crate) fn from_domain(command: &Command) -> Self {
        Self {
            id: command.id.get(),
            kind: match &command.kind {
                CommandKind::StartSolver {
                    run,
                    solver,
                    puzzle,
                } => CommandKindWire::StartSolver {
                    run: run.get(),
                    solver: solver.to_string(),
                    puzzle: puzzle.to_string(),
                },
                CommandKind::CancelSolver { run } => {
                    CommandKindWire::CancelSolver { run: run.get() }
                }
                CommandKind::ScheduleTimer { deadline } => CommandKindWire::ScheduleTimer {
                    deadline_ms: deadline.as_millis(),
                },
            },
        }
    }
    pub(crate) fn into_domain(self) -> Checked<Command> {
        Ok(Command {
            id: self.id.try_into().map_err(domain)?,
            kind: match self.kind {
                CommandKindWire::StartSolver {
                    run,
                    solver,
                    puzzle,
                } => CommandKind::StartSolver {
                    run: run.try_into().map_err(domain)?,
                    solver: solver.try_into().map_err(domain)?,
                    puzzle: puzzle.parse().map_err(domain)?,
                },
                CommandKindWire::CancelSolver { run } => CommandKind::CancelSolver {
                    run: run.try_into().map_err(domain)?,
                },
                CommandKindWire::ScheduleTimer { deadline_ms } => CommandKind::ScheduleTimer {
                    deadline: LogicalTime::from_millis(deadline_ms),
                },
            },
        })
    }
}
impl JobWire {
    pub(crate) fn from_domain(job: &Job) -> Self {
        match job {
            Job::Action(action) => Self::Action {
                action: ActionWire::from_domain(*action),
            },
            Job::TimerFired { command } => Self::TimerFired {
                command: command.get(),
            },
            Job::SolverStarted { run, command } => Self::SolverStarted {
                run: run.get(),
                command: command.get(),
            },
            Job::SolverEvents {
                run,
                command,
                events,
            } => Self::SolverEvents {
                run: run.get(),
                command: command.get(),
                events: events.iter().map(TraceWire::from_domain).collect(),
            },
            Job::SolverCompleted {
                run,
                command,
                result,
                snapshot,
            } => Self::SolverCompleted {
                run: run.get(),
                command: command.get(),
                result: ResultWire::from_domain(result),
                snapshot: SnapshotWire::from_domain(snapshot),
            },
            Job::SolverFailed {
                run,
                command,
                failure,
            } => Self::SolverFailed {
                run: run.get(),
                command: command.get(),
                kind: match failure.kind {
                    FailureKind::Start => FailureKindWire::Start,
                    FailureKind::Execution => FailureKindWire::Execution,
                    FailureKind::ProducerClosed => FailureKindWire::ProducerClosed,
                    FailureKind::Protocol => FailureKindWire::Protocol,
                },
                message: failure.message.clone(),
            },
            Job::SolverCancelled { run, command } => Self::SolverCancelled {
                run: run.get(),
                command: command.get(),
            },
            Job::CommandFailed { command, failure } => Self::CommandFailed {
                command: command.get(),
                kind: failure.kind.into(),
                message: failure.message.clone(),
            },
            Job::SessionEnded { reason } => Self::SessionEnded {
                reason: match reason {
                    SessionEndReason::Quit => EndWire::Quit,
                    SessionEndReason::InputEnded => EndWire::InputEnded,
                    SessionEndReason::Error => EndWire::Error,
                    SessionEndReason::Dropped => EndWire::Dropped,
                },
            },
        }
    }
    pub(crate) fn into_domain(self, commands: &BTreeMap<CommandId, CommandKind>) -> Checked<Job> {
        let start = |run: u64, command: u64| -> Checked<(RunId, CommandId, &Board)> {
            let run = RunId::try_from(run).map_err(domain)?;
            let command = CommandId::try_from(command).map_err(domain)?;
            match commands.get(&command) {
                Some(CommandKind::StartSolver {
                    run: expected,
                    puzzle,
                    ..
                }) if *expected == run => Ok((run, command, puzzle)),
                _ => Err("solver feedback does not match a start command".into()),
            }
        };
        Ok(match self {
            Self::Action { action } => Job::Action(action.into_domain()?),
            Self::CommandFailed {
                command,
                kind,
                message,
            } => {
                let command = CommandId::try_from(command).map_err(domain)?;
                if !commands.contains_key(&command) {
                    return Err("failure does not match an emitted command".into());
                }
                Job::CommandFailed {
                    command,
                    failure: Failure {
                        kind: kind.into(),
                        message,
                    },
                }
            }
            Self::TimerFired { command } => {
                let command = CommandId::try_from(command).map_err(domain)?;
                if !matches!(
                    commands.get(&command),
                    Some(CommandKind::ScheduleTimer { .. })
                ) {
                    return Err("timer feedback does not match a timer command".into());
                }
                Job::TimerFired { command }
            }
            Self::SolverStarted { run, command } => {
                let (run, command, _) = start(run, command)?;
                Job::SolverStarted { run, command }
            }
            Self::SolverEvents {
                run,
                command,
                events,
            } => {
                let (run, command, puzzle) = start(run, command)?;
                Job::SolverEvents {
                    run,
                    command,
                    events: events
                        .into_iter()
                        .map(|event| event.into_domain(puzzle))
                        .collect::<Checked<_>>()?,
                }
            }
            Self::SolverCompleted {
                run,
                command,
                result,
                snapshot,
            } => {
                let (run, command, puzzle) = start(run, command)?;
                let result = result.into_domain(puzzle)?;
                let snapshot = snapshot.into_domain(puzzle)?;
                validate_finish(&result, &snapshot)?;
                Job::SolverCompleted {
                    run,
                    command,
                    result,
                    snapshot,
                }
            }
            Self::SolverFailed {
                run,
                command,
                kind,
                message,
            } => {
                let (run, command, _) = start(run, command)?;
                Job::SolverFailed {
                    run,
                    command,
                    failure: Failure {
                        kind: match kind {
                            FailureKindWire::Start => FailureKind::Start,
                            FailureKindWire::Execution => FailureKind::Execution,
                            FailureKindWire::ProducerClosed => FailureKind::ProducerClosed,
                            FailureKindWire::Protocol => FailureKind::Protocol,
                        },
                        message,
                    },
                }
            }
            Self::SolverCancelled { run, command } => {
                let run = RunId::try_from(run).map_err(domain)?;
                let command = CommandId::try_from(command).map_err(domain)?;
                if !matches!(commands.get(&command),Some(CommandKind::CancelSolver{run:expected}) if *expected==run)
                {
                    return Err("cancellation does not match a cancel command".into());
                }
                Job::SolverCancelled { run, command }
            }
            Self::SessionEnded { reason } => Job::SessionEnded {
                reason: match reason {
                    EndWire::Quit => SessionEndReason::Quit,
                    EndWire::InputEnded => SessionEndReason::InputEnded,
                    EndWire::Error => SessionEndReason::Error,
                    EndWire::Dropped => SessionEndReason::Dropped,
                },
            },
        })
    }
    pub(crate) fn trace_count(&self) -> usize {
        match self {
            Self::SolverEvents { events, .. } => events.len(),
            Self::SolverCompleted { .. } => 1,
            _ => 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SOLUTION: &str =
        "534678912672195348198342567859761423426853791713924856961537284287419635345286179";

    #[test]
    fn decoded_result_requires_a_nonzero_matching_cap_and_a_valid_complete_solution() {
        let valid = ResultWire {
            solution: Some(SOLUTION.into()),
            solutions_found: 2,
            extent: ExtentWire::Capped { limit: 2 },
            stats: SearchStats::default().into(),
        };
        assert_eq!(
            valid
                .clone()
                .into_domain(&Board::default())
                .unwrap()
                .solutions_found(),
            2
        );
        for limit in [0, 1, 3, usize::MAX] {
            let malformed = ResultWire {
                extent: ExtentWire::Capped { limit },
                ..valid.clone()
            };
            assert!(
                malformed.into_domain(&Board::default()).is_err(),
                "cap={limit}"
            );
        }
        for solution in [
            None,
            Some("0".repeat(81)),
            Some("9".repeat(81)),
            Some("123".into()),
        ] {
            assert!(
                ResultWire {
                    solution,
                    ..valid.clone()
                }
                .into_domain(&Board::default())
                .is_err()
            );
        }
        let mut different_givens = [0; 81];
        different_givens[0] = 1;
        assert!(
            valid
                .into_domain(&Board::try_from(different_givens).unwrap())
                .is_err()
        );
    }

    #[test]
    fn decoded_candidate_arrays_require_exactly_81_legal_masks() {
        let valid = SnapshotWire {
            board: "0".repeat(81),
            candidates: Some(vec![0x03fe; 81]),
            stats: SearchStats::default().into(),
            depth: 0,
            focus: 80,
        };
        let decoded = valid.clone().into_domain(&Board::default()).unwrap();
        assert_eq!(decoded.focus().get(), 80);
        assert!(
            decoded
                .candidates()
                .unwrap()
                .iter()
                .all(|mask| mask.iter().count() == 9)
        );
        for len in [0, 1, 80, 82] {
            let malformed = SnapshotWire {
                candidates: Some(vec![0; len]),
                ..valid.clone()
            };
            assert!(
                malformed.into_domain(&Board::default()).is_err(),
                "len={len}"
            );
        }
        for bits in [1, 1 << 10, u16::MAX] {
            let mut masks = vec![0; 81];
            masks[80] = bits;
            assert!(
                SnapshotWire {
                    candidates: Some(masks),
                    ..valid.clone()
                }
                .into_domain(&Board::default())
                .is_err()
            );
        }
    }
}
