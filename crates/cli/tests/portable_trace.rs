//! Portable strict replay of actual solver outcomes across solution multiplicities.

use std::{num::NonZeroUsize, ops::ControlFlow};

use ku_core::{Board, HistoryLimits, Multiplicity, TraceEvent, TraceOutcome};
use ku_session::{
    action::Action,
    app::{App, AppConfig, Mode, RunState},
    history::{History, ReplayMode},
    protocol::{Activation, CommandKind, Completion, Job, LogicalTime},
};
use ku_solvers::built_in_trace_solvers;

const SOLUTION: &str =
    "534678912672195348198342567859761423426853791713924856961537284287419635345286179";
const PUZZLE: &str =
    "530070000600195000098000060800060003400803001700020006060000280000419005000080079";

fn activate(app: &mut App, history: &mut History, jobs: Vec<Job>) -> Completion {
    let id = history.records().len() as u64 + 1;
    let activation =
        Activation::new(id.try_into().unwrap(), LogicalTime::from_millis(id), jobs).unwrap();
    let completion = app.activate(&activation).unwrap();
    history.record(&activation, &completion, app).unwrap();
    completion
}

#[test]
fn real_solver_runs_survive_portable_json_and_strict_replay_for_every_multiplicity() {
    let solved: Board = SOLUTION.parse().unwrap();
    let mut ambiguous = *solved.cells();
    for cell in [3, 4, 30, 31] {
        ambiguous[cell] = 0;
    }
    let mut impossible = [0; 81];
    impossible[..8].copy_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8]);
    impossible[17] = 9;
    for (puzzle, multiplicity) in [
        (solved, Multiplicity::Unique),
        (PUZZLE.parse().unwrap(), Multiplicity::Unique),
        (Board::try_from(ambiguous).unwrap(), Multiplicity::Multiple),
        (
            Board::try_from(impossible).unwrap(),
            Multiplicity::Unsatisfiable,
        ),
    ] {
        for &solver in built_in_trace_solvers() {
            let config = AppConfig::new(
                vec![puzzle.clone().into()],
                0,
                "portable solver trace".into(),
                solver.name(),
                ku_cli::tui::app::solver_catalog(),
                HistoryLimits::default(),
                Mode::Watch,
                NonZeroUsize::new(128).unwrap(),
                50,
            )
            .unwrap();
            let mut history = History::new(config.clone());
            let mut app = App::from_config(config);
            let completion = activate(&mut app, &mut history, vec![Action::ToggleRun.into()]);
            let start = completion
                .commands()
                .iter()
                .find(|command| matches!(command.kind, CommandKind::StartSolver { .. }))
                .unwrap();
            let CommandKind::StartSolver { run, .. } = start.kind else {
                unreachable!()
            };
            let command = start.id;
            activate(
                &mut app,
                &mut history,
                vec![Job::SolverStarted { run, command }],
            );
            let mut events = Vec::new();
            let outcome =
                solver.trace_search(&puzzle, NonZeroUsize::new(2).unwrap(), &mut |event| {
                    events.push(event);
                    ControlFlow::Continue(())
                });
            let TraceOutcome::Completed(result) = outcome else {
                panic!("collecting cannot cancel")
            };
            assert_eq!(result.multiplicity(), multiplicity);
            let Some(TraceEvent::Finished {
                result: terminal,
                snapshot,
            }) = events.pop()
            else {
                panic!("completed search must end with a terminal event")
            };
            assert_eq!(terminal, result);
            // Small batches cross activation and serialization boundaries repeatedly.
            for batch in events.chunks(17) {
                activate(
                    &mut app,
                    &mut history,
                    vec![Job::SolverEvents {
                        run,
                        command,
                        events: batch.to_vec(),
                    }],
                );
            }
            activate(
                &mut app,
                &mut history,
                vec![Job::SolverCompleted {
                    run,
                    command,
                    result: terminal,
                    snapshot,
                }],
            );
            assert_eq!(app.watch().run(), &RunState::Finished(result.clone()));
            activate(&mut app, &mut history, vec![Action::HistoryEnd.into()]);
            assert_eq!(app.trace().finished(), Some(&result));

            let encoded = history.to_json().unwrap();
            let restored = History::from_json(&encoded).unwrap();
            assert_eq!(restored.to_json().unwrap(), encoded);
            assert_eq!(
                restored.replay(ReplayMode::Strict).unwrap().snapshot(),
                app.snapshot(),
                "{} {multiplicity:?}",
                solver.name()
            );
        }
    }
}
