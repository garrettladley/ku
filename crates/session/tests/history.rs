//! Portable history validation, resource bounds, and deterministic replay.

mod support;
use ku_core::{Board, Digit};
use ku_session::{
    action::Action,
    app::{App, ThemeCatalog, ThemeDescriptor},
    history::{History, HistoryError, ReplayError, ReplayMode},
    protocol::{Activation, ActivationId, CommandKind, Job, LogicalTime, SessionEndReason},
};
use serde_json::Value;

fn app() -> App {
    support::app(
        vec![Board::default().into()],
        0,
        "portable".into(),
        "bitmask",
    )
    .unwrap()
}
fn record(history: &mut History, app: &mut App, id: u64, jobs: Vec<Job>) {
    let activation = Activation::new(
        id.try_into().unwrap(),
        LogicalTime::from_millis(id * 10),
        jobs,
    )
    .unwrap();
    let completion = app.activate(&activation).unwrap();
    history.record(&activation, &completion, app).unwrap();
}
fn history() -> History {
    let mut app = app();
    let mut history = History::new(app.config().clone());
    record(
        &mut history,
        &mut app,
        1,
        vec![Action::Digit(Digit::try_from(1).unwrap()).into()],
    );
    record(&mut history, &mut app, 2, vec![Action::ToggleMode.into()]);
    record(&mut history, &mut app, 3, vec![Action::ToggleRun.into()]);
    history
}
fn json(history: &History) -> Value {
    serde_json::from_str(&history.to_json().unwrap()).unwrap()
}
fn load(value: &Value) -> Result<History, HistoryError> {
    History::from_json(&value.to_string())
}

#[test]
fn portable_json_round_trips_and_replays_without_infrastructure() {
    let history = history();
    let restored = History::from_json(&history.to_json().unwrap()).unwrap();
    assert_eq!(history.to_json().unwrap(), restored.to_json().unwrap());
    assert_eq!(
        history.replay(ReplayMode::Strict).unwrap().snapshot(),
        restored.replay(ReplayMode::Strict).unwrap().snapshot()
    );
    assert_eq!(restored.records().len(), 3);
}

#[test]
fn sidebar_actions_round_trip_and_strict_replay_tracks_optional_view_state() {
    let mut app = app();
    let mut history = History::new(app.config().clone());
    for (index, action) in [
        Action::ToggleSidebar,
        Action::ToggleMode,
        Action::Escape,
        Action::ToggleSidebar,
    ]
    .into_iter()
    .enumerate()
    {
        record(
            &mut history,
            &mut app,
            index as u64 + 1,
            vec![action.into()],
        );
    }
    let encoded = history.to_json().unwrap();
    let restored = History::from_json(&encoded).unwrap();
    assert_eq!(restored.to_json().unwrap(), encoded);
    assert_eq!(
        restored.replay(ReplayMode::Strict).unwrap().snapshot(),
        app.snapshot()
    );
    assert!(app.sidebar_open());
    let mut value = json(&history);
    let records = value["records"].as_array().unwrap();
    assert_eq!(
        records[0]["activation"]["jobs"][0]["action"]["type"],
        "toggle_sidebar"
    );
    for (index, open) in [true, true, false, true].into_iter().enumerate() {
        assert_eq!(
            records[index]["snapshot"].get("sidebar_open"),
            open.then_some(&Value::Bool(true))
        );
        assert!(
            records[index]["completion"]["commands"]
                .as_array()
                .unwrap()
                .is_empty()
        );
    }
    value["records"][0]["snapshot"]
        .as_object_mut()
        .unwrap()
        .remove("sidebar_open");
    assert!(
        matches!(load(&value).unwrap().replay(ReplayMode::Strict), Err(ReplayError::Snapshot { activation }) if activation == ActivationId::FIRST)
    );
}

#[test]
fn portable_theme_genesis_and_picker_selection_round_trip_with_strict_replay() {
    let catalog = ThemeCatalog::try_from(vec![
        ThemeDescriptor {
            id: "ku".into(),
            label: "Ku".into(),
        },
        ThemeDescriptor {
            id: "nord".into(),
            label: "Nord".into(),
        },
    ])
    .unwrap();
    let config = app().config().clone().with_themes(catalog, "nord").unwrap();
    let mut app = App::from_config(config);
    let mut history = History::new(app.config().clone());
    for (index, action) in [
        Action::ChooseTheme,
        Action::Move {
            rows: -1,
            columns: 0,
        },
        Action::Help,
        Action::Escape,
        Action::Accept,
    ]
    .into_iter()
    .enumerate()
    {
        record(
            &mut history,
            &mut app,
            index as u64 + 1,
            vec![action.into()],
        );
    }
    assert_eq!(app.theme().id, "ku");
    let encoded = history.to_json().unwrap();
    let restored = History::from_json(&encoded).unwrap();
    assert_eq!(restored.to_json().unwrap(), encoded);
    assert_eq!(
        restored.replay(ReplayMode::Strict).unwrap().snapshot(),
        app.snapshot()
    );
    let mut value = json(&history);
    assert_eq!(value["config"]["initial_theme"], "nord");
    assert_eq!(value["config"]["themes"][1]["label"], "Nord");
    assert_eq!(
        value["records"][0]["activation"]["jobs"][0]["action"]["type"],
        "choose_theme"
    );
    assert_eq!(
        value["records"][0]["snapshot"]["overlay"]["overlay"]["type"],
        "theme_picker"
    );
    assert_eq!(value["records"][0]["snapshot"]["theme"], "nord");
    assert!(value["records"][4]["snapshot"].get("theme").is_none());
    for record in value["records"].as_array().unwrap() {
        assert!(
            record["completion"]["commands"]
                .as_array()
                .unwrap()
                .is_empty()
        );
    }
    value["records"][0]["snapshot"]["theme"] = Value::String("ku".into());
    assert!(
        matches!(load(&value).unwrap().replay(ReplayMode::Strict), Err(ReplayError::Snapshot { activation }) if activation == ActivationId::FIRST)
    );
}

#[test]
fn portable_history_validates_theme_catalog_and_initial_identity() {
    let original = json(&history());
    assert!(original["config"].get("themes").is_none());
    assert!(original["config"].get("initial_theme").is_none());
    for themes in [
        serde_json::json!([]),
        serde_json::json!([{ "id": "invalid id", "label": "Invalid" }]),
        serde_json::json!([{ "id": "ku", "label": "" }]),
        serde_json::json!([{ "id": "ku", "label": "One" }, { "id": "ku", "label": "Two" }]),
    ] {
        let mut value = original.clone();
        value["config"]["themes"] = themes;
        assert!(matches!(load(&value), Err(HistoryError::Domain(_))));
    }
    let mut value = original;
    value["config"]["initial_theme"] = Value::String("missing".into());
    assert!(matches!(load(&value), Err(HistoryError::Domain(_))));
}
#[test]
fn semantic_replay_ignores_display_copy_but_strict_reports_its_first_activation() {
    let mut value = json(&history());
    value["records"][0]["snapshot"]["notice"]["message"] = Value::String("new display copy".into());
    let history = load(&value).unwrap();
    history.replay(ReplayMode::Semantic).unwrap();
    assert!(
        matches!(history.replay(ReplayMode::Strict),Err(ReplayError::Snapshot{activation}) if activation==ActivationId::FIRST)
    );
}
#[test]
fn semantic_replay_reports_the_first_command_divergence() {
    let mut value = json(&history());
    let records = value["records"].as_array_mut().unwrap();
    let record = records
        .iter_mut()
        .find(|record| {
            !record["completion"]["commands"]
                .as_array()
                .unwrap()
                .is_empty()
        })
        .unwrap();
    let id = record["activation"]["id"].as_u64().unwrap();
    record["completion"]["commands"] = serde_json::json!([]);
    let history = load(&value).unwrap();
    assert!(
        matches!(history.replay(ReplayMode::Semantic),Err(ReplayError::Commands{activation,..}) if activation.get()==id)
    );
}
#[test]
fn unsupported_schema_is_rejected_before_interpreting_its_domain_fields() {
    assert!(matches!(
        History::from_json(r#"{"schema_version":99}"#),
        Err(HistoryError::UnsupportedVersion(99))
    ));
}
#[test]
fn malformed_identifiers_digits_and_genesis_are_rejected() {
    let original = json(&history());
    for mutate in [
        |value: &mut Value| value["records"][0]["activation"]["id"] = 0.into(),
        |value: &mut Value| value["records"][0]["completion"]["activation"] = 0.into(),
        |value: &mut Value| {
            value["records"][0]["activation"]["jobs"][0]["action"]["digit"] = 10.into()
        },
        |value: &mut Value| value["config"]["records"][0]["puzzle"] = "broken".into(),
        |value: &mut Value| value["config"]["events_per_tick"] = 0.into(),
        |value: &mut Value| value["config"]["catalog"][0]["id"] = "invalid solver".into(),
    ] {
        let mut value = original.clone();
        mutate(&mut value);
        assert!(load(&value).is_err(), "accepted malformed history: {value}");
    }
}
#[test]
fn rejected_append_does_not_corrupt_the_next_valid_record() {
    let mut app = app();
    let mut history = History::new(app.config().clone());
    let activation = Activation::new(
        ActivationId::FIRST,
        LogicalTime::ZERO,
        vec![Action::ToggleMode.into(), Action::ToggleRun.into()],
    )
    .unwrap();
    let completion = app.activate(&activation).unwrap();
    let incorrect = ku_session::protocol::Completion::new(ActivationId::FIRST, vec![]).unwrap();
    assert!(history.record(&activation, &incorrect, &app).is_err());
    assert!(history.records().is_empty());
    history.record(&activation, &completion, &app).unwrap();
    history.replay(ReplayMode::Strict).unwrap();
}

#[test]
fn rejecting_an_append_restores_a_nonempty_validators_complete_prior_state() {
    let mut app = app();
    let mut history = History::new(app.config().clone());
    record(
        &mut history,
        &mut app,
        1,
        vec![Action::Digit(Digit::try_from(4).unwrap()).into()],
    );
    record(&mut history, &mut app, 2, vec![Action::ToggleMode.into()]);
    let prefix = history.to_json().unwrap();
    let activation = Activation::new(
        3.try_into().unwrap(),
        LogicalTime::from_millis(30),
        vec![Action::ToggleRun.into()],
    )
    .unwrap();
    let completion = app.activate(&activation).unwrap();
    assert!(!completion.commands().is_empty());
    let wrong = ku_session::protocol::Completion::new(activation.id(), vec![]).unwrap();
    assert!(matches!(
        history.record(&activation, &wrong, &app),
        Err(HistoryError::Record { .. })
    ));
    assert_eq!(history.to_json().unwrap(), prefix);
    history.record(&activation, &completion, &app).unwrap();
    record(&mut history, &mut app, 4, vec![Action::Quit.into()]);
    assert_eq!(
        history.replay(ReplayMode::Strict).unwrap().snapshot(),
        app.snapshot()
    );
    let restored = History::from_json(&history.to_json().unwrap()).unwrap();
    assert_eq!(
        restored.replay(ReplayMode::Strict).unwrap().snapshot(),
        app.snapshot()
    );
}

#[test]
fn portable_history_rejects_mismatched_acknowledgements_and_reused_command_ids() {
    let mut history = history();
    let mut app = history.replay(ReplayMode::Strict).unwrap();
    record(&mut history, &mut app, 4, vec![Action::Quit.into()]);
    let original = json(&history);
    let mut wrong_ack = original.clone();
    wrong_ack["records"][2]["completion"]["activation"] = 2.into();
    assert!(
        matches!(load(&wrong_ack), Err(HistoryError::Record { activation, .. }) if activation.get() == 3)
    );

    let mut repeated_command = original;
    let old_id = repeated_command["records"][2]["completion"]["commands"][0]["id"].clone();
    repeated_command["records"][3]["completion"]["commands"][0]["id"] = old_id;
    assert!(
        matches!(load(&repeated_command), Err(HistoryError::Record { activation, .. }) if activation.get() == 4)
    );
}

#[test]
fn portable_genesis_rejects_zero_budgets_and_solutions_that_change_givens() {
    let original = json(&history());
    for field in ["max_events", "max_bytes"] {
        let mut value = original.clone();
        value["config"][field] = 0.into();
        assert!(
            matches!(load(&value), Err(HistoryError::Domain(_))),
            "field={field}"
        );
    }
    let mut incomplete = original.clone();
    incomplete["config"]["records"][0]["solution"] = "0".repeat(81).into();
    assert!(matches!(load(&incomplete), Err(HistoryError::Domain(_))));
    let mut conflicting = original;
    conflicting["config"]["records"][0]["puzzle"] = format!("1{}", "0".repeat(80)).into();
    conflicting["config"]["records"][0]["solution"] =
        "534678912672195348198342567859761423426853791713924856961537284287419635345286179".into();
    assert!(matches!(load(&conflicting), Err(HistoryError::Domain(_))));
}
#[test]
fn typed_failure_and_cancellation_feedback_survive_json() {
    use ku_session::protocol::{Failure, FailureKind};
    let mut app = app();
    let mut history = History::new(app.config().clone());
    record(&mut history, &mut app, 1, vec![Action::ToggleMode.into()]);
    record(&mut history, &mut app, 2, vec![Action::ToggleRun.into()]);
    let start = history
        .records()
        .iter()
        .flat_map(|record| record.completion().commands())
        .find(|command| matches!(command.kind, CommandKind::StartSolver { .. }))
        .unwrap();
    let (run, command) = match start.kind {
        CommandKind::StartSolver { run, .. } => (run, start.id),
        _ => unreachable!(),
    };
    record(
        &mut history,
        &mut app,
        3,
        vec![Job::SolverFailed {
            run,
            command,
            failure: Failure {
                kind: FailureKind::Execution,
                message: "scripted worker failure".into(),
            },
        }],
    );
    if let Some(cancel) = history
        .records()
        .last()
        .unwrap()
        .completion()
        .commands()
        .iter()
        .find(|command| matches!(command.kind, CommandKind::CancelSolver { .. }))
    {
        let command = cancel.id;
        record(
            &mut history,
            &mut app,
            4,
            vec![Job::SolverCancelled { run, command }],
        );
        record(
            &mut history,
            &mut app,
            5,
            vec![Job::SessionEnded {
                reason: SessionEndReason::Error,
            }],
        );
    } else {
        record(
            &mut history,
            &mut app,
            4,
            vec![Job::SessionEnded {
                reason: SessionEndReason::Error,
            }],
        );
    }
    let encoded = history.to_json().unwrap();
    assert!(encoded.contains("scripted worker failure"));
    assert!(encoded.contains("execution"));
    History::from_json(&encoded)
        .unwrap()
        .replay(ReplayMode::Strict)
        .unwrap();
}
#[test]
fn checked_in_cross_process_fixture_replays_in_a_fresh_model() {
    let encoded = include_str!("history_fixtures/play.json");
    let history = History::from_json(encoded).unwrap();
    let app = history.replay(ReplayMode::Strict).unwrap();
    assert_eq!(history.records().len(), 3);
    assert_eq!(app.snapshot().play_board.cells()[0], 4);
    assert!(app.snapshot().closed);
}

#[test]
fn checked_trace_domain_and_feedback_correlations_reject_malformed_payloads() {
    use ku_core::{
        CandidateMask, CellIndex, SearchExtent, SearchResult, SearchStats, TraceEvent, TraceKind,
        TraceSnapshot,
    };
    let puzzle: Board =
        "100000000000000000000000000000000000000000000000000000000000000000000000000000000"
            .parse()
            .unwrap();
    let mut app = support::app(vec![puzzle.clone().into()], 0, "trace".into(), "bitmask").unwrap();
    let mut history = History::new(app.config().clone());
    record(&mut history, &mut app, 1, vec![Action::ToggleMode.into()]);
    record(&mut history, &mut app, 2, vec![Action::ToggleRun.into()]);
    let command = history
        .records()
        .last()
        .unwrap()
        .completion()
        .commands()
        .iter()
        .find(|command| matches!(command.kind, CommandKind::StartSolver { .. }))
        .unwrap();
    let (run, command) = match command.kind {
        CommandKind::StartSolver { run, .. } => (run, command.id),
        _ => unreachable!(),
    };
    record(
        &mut history,
        &mut app,
        3,
        vec![Job::SolverStarted { run, command }],
    );
    let stats = SearchStats {
        nodes: 1,
        ..SearchStats::default()
    };
    let snapshot = TraceSnapshot::new(
        puzzle.clone(),
        Some([CandidateMask::default(); 81]),
        stats,
        0,
        CellIndex::FIRST,
    );
    record(
        &mut history,
        &mut app,
        4,
        vec![Job::SolverEvents {
            run,
            command,
            events: vec![
                TraceEvent::Start {
                    solver: "bitmask".into(),
                    puzzle: puzzle.clone(),
                },
                TraceEvent::Step {
                    kind: TraceKind::Candidates,
                    snapshot: snapshot.clone(),
                },
            ],
        }],
    );
    let result = SearchResult::new(&puzzle, None, 0, SearchExtent::Exhausted, stats).unwrap();
    record(
        &mut history,
        &mut app,
        5,
        vec![Job::SolverCompleted {
            run,
            command,
            result,
            snapshot,
        }],
    );
    let original = json(&history);
    let mutations: [fn(&mut Value); 11] = [
        |value| value["records"][3]["activation"]["jobs"][0]["run"] = 0.into(),
        |value| value["records"][3]["activation"]["jobs"][0]["command"] = 999.into(),
        |value| {
            value["records"][3]["activation"]["jobs"][0]["events"][1]["snapshot"]["focus"] =
                81.into()
        },
        |value| {
            value["records"][3]["activation"]["jobs"][0]["events"][1]["snapshot"]["candidates"][0] =
                1.into()
        },
        |value| {
            value["records"][3]["activation"]["jobs"][0]["events"][1]["snapshot"]["board"] =
                "000000000000000000000000000000000000000000000000000000000000000000000000000000000"
                    .into()
        },
        |value| {
            value["records"][3]["activation"]["jobs"][0]["events"][1]["kind"] =
                serde_json::json!({"type":"assignment","cell":0,"digit":0,"forced":true})
        },
        |value| {
            value["records"][4]["activation"]["jobs"][0]["result"]["solutions_found"] = 1.into()
        },
        |value| {
            value["records"][3]["activation"]["jobs"][0]["events"][1]["snapshot"]["candidates"] =
                serde_json::json!(vec![0; 80])
        },
        |value| {
            value["records"][3]["activation"]["jobs"][0]["events"][1]["kind"] =
                serde_json::json!({"type":"solution"})
        },
        |value| {
            value["records"][3]["activation"]["jobs"][0]["events"][0]["puzzle"] =
                "0".repeat(81).into()
        },
        |value| {
            value["records"][4]["activation"]["jobs"][0]["snapshot"]["stats"]["nodes"] = 2.into()
        },
    ];
    for mutate in mutations {
        let mut value = original.clone();
        mutate(&mut value);
        assert!(load(&value).is_err(), "accepted malformed trace");
    }
    History::from_json(&history.to_json().unwrap())
        .unwrap()
        .replay(ReplayMode::Strict)
        .unwrap();
}

#[test]
fn timer_firing_preserves_command_correlation_and_logical_time() {
    let mut app = app();
    let mut history = History::new(app.config().clone());
    record(
        &mut history,
        &mut app,
        1,
        vec![Action::ToggleMode.into(), Action::ToggleRun.into()],
    );
    let timer = history.records()[0]
        .completion()
        .commands()
        .iter()
        .find(|command| matches!(command.kind, CommandKind::ScheduleTimer { .. }))
        .unwrap()
        .id;
    let activation = Activation::new(
        2_u64.try_into().unwrap(),
        LogicalTime::from_millis(100),
        vec![Job::TimerFired { command: timer }],
    )
    .unwrap();
    let completion = app.activate(&activation).unwrap();
    history.record(&activation, &completion, &app).unwrap();
    let restored = History::from_json(&history.to_json().unwrap()).unwrap();
    assert!(
        matches!(restored.records()[1].activation().jobs(),[Job::TimerFired{command}] if *command==timer)
    );
    assert_eq!(
        restored
            .replay(ReplayMode::Strict)
            .unwrap()
            .snapshot()
            .logical_time,
        LogicalTime::from_millis(100)
    );
}

#[test]
fn timer_and_cancel_command_failures_are_portable_even_after_quit() {
    use ku_session::protocol::{Failure, FailureKind};
    for cancel in [false, true] {
        let mut app = app();
        let mut history = History::new(app.config().clone());
        record(
            &mut history,
            &mut app,
            1,
            vec![Action::ToggleMode.into(), Action::ToggleRun.into()],
        );
        let (id, command) = if cancel {
            record(&mut history, &mut app, 2, vec![Action::Quit.into()]);
            (
                3,
                history.records()[1]
                    .completion()
                    .commands()
                    .iter()
                    .find(|command| matches!(command.kind, CommandKind::CancelSolver { .. }))
                    .unwrap()
                    .id,
            )
        } else {
            (
                2,
                history.records()[0]
                    .completion()
                    .commands()
                    .iter()
                    .find(|command| matches!(command.kind, CommandKind::ScheduleTimer { .. }))
                    .unwrap()
                    .id,
            )
        };
        let failure = Failure {
            kind: FailureKind::Execution,
            message: if cancel {
                "cancel adapter failed"
            } else {
                "timer adapter failed"
            }
            .into(),
        };
        record(
            &mut history,
            &mut app,
            id,
            vec![Job::CommandFailed {
                command,
                failure: failure.clone(),
            }],
        );
        assert_eq!(
            app.snapshot().last_command_failure,
            Some((command, failure))
        );
        record(
            &mut history,
            &mut app,
            id + 1,
            vec![Job::SessionEnded {
                reason: SessionEndReason::Error,
            }],
        );
        let encoded = history.to_json().unwrap();
        assert!(encoded.contains("command_failed"));
        let restored = History::from_json(&encoded).unwrap();
        assert_eq!(
            restored.replay(ReplayMode::Strict).unwrap().snapshot(),
            app.snapshot()
        );
        let mut malformed: Value = serde_json::from_str(&encoded).unwrap();
        malformed["records"][(id - 1) as usize]["activation"]["jobs"][0]["command"] = 999.into();
        assert!(load(&malformed).is_err());
    }
}
