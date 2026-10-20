//! Portable, checked activation history and deterministic replay.
//!
//! JSON schema version 1 records validated genesis configuration, ordered jobs,
//! commands, and comparison-only state checkpoints. Loading never restores model
//! state from a checkpoint: all domain values use checked constructors, and replay
//! applies recorded activations to a fresh model without invoking infrastructure.
use std::{collections::BTreeMap, num::NonZeroUsize};

use ku_core::{Board, HistoryLimits, dataset::Record};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    app::{App, AppConfig, Mode, SolverCatalog, SolverDescriptor, ThemeCatalog, ThemeDescriptor},
    protocol::*,
    wire::*,
};

/// Maximum encoded history size accepted on input and output (64 MiB).
/// This bound is checked before JSON parsing; configured trace budgets also apply
/// to each event batch, with explicit limits on history records and total jobs.
pub const MAX_JSON_BYTES: usize = 64 * 1024 * 1024;
const MAX_RECORDS: usize = 100_000;
const MAX_JOBS: usize = 1_000_000;
const SCHEMA_VERSION: u32 = 1;

/// A malformed, inconsistent, unsupported, or oversized portable history.
#[derive(Debug, thiserror::Error)]
pub enum HistoryError {
    /// Encoded input or output exceeded [`MAX_JSON_BYTES`].
    #[error("history JSON exceeds the 64 MiB limit")]
    JsonLimit,
    /// A configured trace budget or global record/job bound was exceeded.
    #[error("history exceeds its configured event or record limits")]
    CountLimit,
    /// The encoded schema version is not supported.
    #[error("unsupported history schema version {0}")]
    UnsupportedVersion(u32),
    /// The input was not valid history JSON.
    #[error("invalid history JSON: {0}")]
    Json(#[from] serde_json::Error),
    /// A decoded domain value failed checked construction.
    #[error("invalid history domain value: {0}")]
    Domain(String),
    /// A record failed protocol or replay validation.
    #[error("history record for activation {activation} is invalid: {message}")]
    Record {
        /// The rejected activation identity.
        activation: ActivationId,
        /// The validation diagnostic.
        message: String,
    },
}
/// Replay checks semantic commands by default; strict replay also checks state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReplayMode {
    /// Checks that replay emits the recorded semantic commands.
    Semantic,
    /// Also compares every recorded state checkpoint.
    Strict,
}
/// The first activation at which replay diverges.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ReplayError {
    /// The activation was rejected by the deterministic model.
    #[error("activation {activation} violates the protocol: {source}")]
    Protocol {
        /// The rejected activation identity.
        activation: ActivationId,
        /// The underlying envelope or ordering violation.
        source: ProtocolError,
    },
    /// Replay emitted a different command sequence.
    #[error("commands diverged at activation {activation}")]
    Commands {
        /// The first divergent activation identity.
        activation: ActivationId,
        /// The command sequence stored in the recording.
        expected: Vec<Command>,
        /// The command sequence emitted during replay.
        actual: Vec<Command>,
    },
    /// Replay state differed from the recorded strict checkpoint.
    #[error("state checkpoint diverged at activation {activation}")]
    Snapshot {
        /// The first activation with a divergent state checkpoint.
        activation: ActivationId,
    },
}
impl ReplayError {
    /// Returns the first activation at which replay diverged.
    pub fn activation(&self) -> ActivationId {
        match self {
            Self::Protocol { activation, .. }
            | Self::Commands { activation, .. }
            | Self::Snapshot { activation } => *activation,
        }
    }
}
/// A validated activation, its expected completion, and a state checkpoint.
#[derive(Clone, Debug)]
pub struct HistoryRecord {
    activation: Activation,
    completion: Completion,
    snapshot: Value,
}
impl HistoryRecord {
    /// Borrows the validated activation and its ordered jobs.
    pub fn activation(&self) -> &Activation {
        &self.activation
    }
    /// Borrows the recorded expected command sequence.
    pub fn completion(&self) -> &Completion {
        &self.completion
    }
    /// A strict replay expectation, never trusted to reconstruct model state.
    pub fn snapshot(&self) -> &Value {
        &self.snapshot
    }
}
/// Append-only session history with complete portable genesis configuration.
#[derive(Clone, Debug)]
pub struct History {
    config: AppConfig,
    records: Vec<HistoryRecord>,
    validator: App,
    commands: BTreeMap<CommandId, CommandKind>,
    jobs: usize,
    encoded_bytes: usize,
}
impl History {
    /// Starts an empty recording from validated immutable genesis data.
    pub fn new(config: AppConfig) -> Self {
        let validator = App::from_config(config.clone());
        Self {
            config,
            records: Vec::new(),
            validator,
            commands: BTreeMap::new(),
            jobs: 0,
            encoded_bytes: 0,
        }
    }
    /// Borrows the genesis used to reconstruct the model.
    pub fn config(&self) -> &AppConfig {
        &self.config
    }
    /// Borrows the accepted records in activation order.
    pub fn records(&self) -> &[HistoryRecord] {
        &self.records
    }
    /// Checks and appends the actual model response to one activation.
    /// Rejected appends leave the recording and replay validator unchanged.
    ///
    /// # Errors
    /// Rejects mismatched genesis, invalid activations or command references,
    /// divergent commands or checkpoints, and exceeded history budgets.
    pub fn record(
        &mut self,
        activation: &Activation,
        completion: &Completion,
        app: &App,
    ) -> Result<(), HistoryError> {
        self.check_budget(
            activation.jobs().len(),
            activation
                .jobs()
                .iter()
                .map(|job| match job {
                    Job::SolverEvents { events, .. } => events.len(),
                    Job::SolverCompleted { .. } => 1,
                    _ => 0,
                })
                .sum(),
        )?;
        let id = activation.id();
        if completion.activation() != id || app.config() != &self.config {
            return Err(HistoryError::Record {
                activation: id,
                message: "completion identity or model configuration differs".into(),
            });
        }
        // Conversion validates feedback against recorded command identities and
        // validates trace/result data before the model is allowed to consume it.
        for job in activation.jobs() {
            JobWire::from_domain(job)
                .into_domain(&self.commands)
                .map_err(HistoryError::Domain)?;
        }
        let record = HistoryRecord {
            activation: activation.clone(),
            completion: completion.clone(),
            snapshot: snapshot_value(app),
        };
        let bytes = serde_json::to_vec(&RecordWire::from_domain(&record))?.len();
        if self.encoded_bytes.saturating_add(bytes) > MAX_JSON_BYTES {
            return Err(HistoryError::JsonLimit);
        }
        let expected =
            self.validator
                .activate(activation)
                .map_err(|source| HistoryError::Record {
                    activation: id,
                    message: source.to_string(),
                })?;
        if expected != *completion || self.validator.snapshot() != app.snapshot() {
            self.restore_validator()?;
            return Err(HistoryError::Record {
                activation: id,
                message: "supplied completion or model state differs from replay".into(),
            });
        }
        self.encoded_bytes += bytes;
        self.jobs += activation.jobs().len();
        for command in completion.commands() {
            self.commands.insert(command.id, command.kind.clone());
        }
        self.records.push(record);
        Ok(())
    }
    fn restore_validator(&mut self) -> Result<(), HistoryError> {
        let mut validator = App::from_config(self.config.clone());
        for record in &self.records {
            validator
                .activate(&record.activation)
                .map_err(|error| HistoryError::Record {
                    activation: record.activation.id(),
                    message: error.to_string(),
                })?;
        }
        self.validator = validator;
        Ok(())
    }
    fn check_budget(&self, jobs: usize, events: usize) -> Result<(), HistoryError> {
        if self.records.len() >= MAX_RECORDS
            || self.jobs.saturating_add(jobs) > MAX_JOBS
            || events > self.config.history_limits().max_events().get()
        {
            Err(HistoryError::CountLimit)
        } else {
            Ok(())
        }
    }
    /// Encodes the complete history as versioned JSON.
    ///
    /// # Errors
    /// Returns a JSON serialization error or [`HistoryError::JsonLimit`] if the output is too large.
    pub fn to_json(&self) -> Result<String, HistoryError> {
        let wire = HistoryWire {
            schema_version: SCHEMA_VERSION,
            config: ConfigWire::from_domain(&self.config),
            records: self.records.iter().map(RecordWire::from_domain).collect(),
        };
        let json = serde_json::to_string(&wire)?;
        if json.len() > MAX_JSON_BYTES {
            return Err(HistoryError::JsonLimit);
        }
        Ok(json)
    }
    /// Loads versioned JSON through checked domain constructors and protocol validation.
    ///
    /// No solver, runtime adapter, or terminal is invoked.
    ///
    /// # Errors
    /// Rejects oversized input, unsupported versions, malformed domain values,
    /// invalid protocol ordering, and exceeded history budgets. Expected commands and
    /// checkpoints are compared separately by [`Self::replay`].
    pub fn from_json(json: &str) -> Result<Self, HistoryError> {
        if json.len() > MAX_JSON_BYTES {
            return Err(HistoryError::JsonLimit);
        }
        // Read the version before interpreting any version-specific domain data.
        let version: VersionProbe = serde_json::from_str(json)?;
        if version.schema_version != SCHEMA_VERSION {
            return Err(HistoryError::UnsupportedVersion(version.schema_version));
        }
        let wire: HistoryWire = serde_json::from_str(json)?;
        if wire.schema_version != SCHEMA_VERSION {
            return Err(HistoryError::UnsupportedVersion(wire.schema_version));
        }
        let mut history = Self::new(wire.config.into_domain()?);
        for record in wire.records {
            history.check_budget(
                record.activation.jobs.len(),
                record
                    .activation
                    .jobs
                    .iter()
                    .map(JobWire::trace_count)
                    .sum(),
            )?;
            let jobs = record
                .activation
                .jobs
                .into_iter()
                .map(|job| job.into_domain(&history.commands))
                .collect::<Result<Vec<_>, _>>()
                .map_err(HistoryError::Domain)?;
            let activation = Activation::new(
                record
                    .activation
                    .id
                    .try_into()
                    .map_err(|error: ProtocolError| HistoryError::Domain(error.to_string()))?,
                LogicalTime::from_millis(record.activation.time_ms),
                jobs,
            )
            .map_err(|error| HistoryError::Domain(error.to_string()))?;
            let commands = record
                .completion
                .commands
                .into_iter()
                .map(CommandWire::into_domain)
                .collect::<Result<Vec<_>, _>>()
                .map_err(HistoryError::Domain)?;
            let completion = Completion::new(
                record
                    .completion
                    .activation
                    .try_into()
                    .map_err(|error: ProtocolError| HistoryError::Domain(error.to_string()))?,
                commands,
            )
            .map_err(|error| HistoryError::Domain(error.to_string()))?;
            if completion.activation() != activation.id() {
                return Err(HistoryError::Record {
                    activation: activation.id(),
                    message: "completion acknowledges another activation".into(),
                });
            }
            // Loading checks protocol order and domain relationships while leaving
            // expected commands/checkpoints intact for a useful divergence report.
            history
                .validator
                .activate(&activation)
                .map_err(|error| HistoryError::Record {
                    activation: activation.id(),
                    message: error.to_string(),
                })?;
            history.jobs += activation.jobs().len();
            for command in completion.commands() {
                if history
                    .commands
                    .last_key_value()
                    .is_some_and(|(id, _)| *id >= command.id)
                {
                    return Err(HistoryError::Record {
                        activation: activation.id(),
                        message: "command identities are not globally increasing".into(),
                    });
                }
                if history
                    .commands
                    .insert(command.id, command.kind.clone())
                    .is_some()
                {
                    return Err(HistoryError::Record {
                        activation: activation.id(),
                        message: "duplicate command identity".into(),
                    });
                }
            }
            history.records.push(HistoryRecord {
                activation,
                completion,
                snapshot: record.snapshot,
            });
        }
        history.encoded_bytes = json.len();
        Ok(history)
    }
    /// Replays recorded jobs without executing commands or consulting adapters.
    ///
    /// # Errors
    /// Returns the first protocol or command divergence, or state checkpoint
    /// divergence when using [`ReplayMode::Strict`].
    pub fn replay(&self, mode: ReplayMode) -> Result<App, ReplayError> {
        let mut app = App::from_config(self.config.clone());
        for record in &self.records {
            let activation = record.activation.id();
            let actual = app
                .activate(&record.activation)
                .map_err(|source| ReplayError::Protocol { activation, source })?;
            if actual != record.completion {
                return Err(ReplayError::Commands {
                    activation,
                    expected: record.completion.commands().to_vec(),
                    actual: actual.commands().to_vec(),
                });
            }
            if mode == ReplayMode::Strict && snapshot_value(&app) != record.snapshot {
                return Err(ReplayError::Snapshot { activation });
            }
        }
        Ok(app)
    }
}
#[derive(Deserialize)]
struct VersionProbe {
    schema_version: u32,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct HistoryWire {
    schema_version: u32,
    config: ConfigWire,
    records: Vec<RecordWire>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RecordWire {
    activation: ActivationWire,
    completion: CompletionWire,
    snapshot: Value,
}
impl RecordWire {
    fn from_domain(record: &HistoryRecord) -> Self {
        Self {
            activation: ActivationWire {
                id: record.activation.id().get(),
                time_ms: record.activation.time().as_millis(),
                jobs: record
                    .activation
                    .jobs()
                    .iter()
                    .map(JobWire::from_domain)
                    .collect(),
            },
            completion: CompletionWire {
                activation: record.completion.activation().get(),
                commands: record
                    .completion
                    .commands()
                    .iter()
                    .map(CommandWire::from_domain)
                    .collect(),
            },
            snapshot: record.snapshot.clone(),
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ActivationWire {
    id: u64,
    time_ms: u64,
    jobs: Vec<JobWire>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CompletionWire {
    activation: u64,
    commands: Vec<CommandWire>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RecordConfigWire {
    puzzle: String,
    solution: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DescriptorWire {
    id: String,
    tracing: bool,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ModeWire {
    Play,
    Watch,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfigWire {
    records: Vec<RecordConfigWire>,
    record_index: usize,
    label: String,
    solver: String,
    catalog: Vec<DescriptorWire>,
    max_events: usize,
    max_bytes: usize,
    initial_mode: ModeWire,
    events_per_tick: usize,
    tick_interval_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    themes: Option<Vec<ThemeWire>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    initial_theme: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ThemeWire {
    id: String,
    label: String,
}
impl ConfigWire {
    fn from_domain(config: &AppConfig) -> Self {
        Self {
            records: config
                .records()
                .iter()
                .map(|record| RecordConfigWire {
                    puzzle: record.puzzle().to_string(),
                    solution: record.solution().map(ToString::to_string),
                })
                .collect(),
            record_index: config.record_index(),
            label: config.label().to_owned(),
            solver: config.solver().to_string(),
            catalog: config
                .catalog()
                .as_ref()
                .iter()
                .map(|entry| DescriptorWire {
                    id: entry.id.to_string(),
                    tracing: entry.tracing,
                })
                .collect(),
            max_events: config.history_limits().max_events().get(),
            max_bytes: config.history_limits().max_bytes().get(),
            initial_mode: match config.initial_mode() {
                Mode::Play => ModeWire::Play,
                Mode::Watch => ModeWire::Watch,
            },
            events_per_tick: config.events_per_tick().get(),
            tick_interval_ms: config.tick_interval_ms(),
            themes: (config.themes() != &ThemeCatalog::default()).then(|| {
                config
                    .themes()
                    .as_ref()
                    .iter()
                    .map(|theme| ThemeWire {
                        id: theme.id.clone(),
                        label: theme.label.clone(),
                    })
                    .collect()
            }),
            initial_theme: (config.initial_theme() != "ku")
                .then(|| config.initial_theme().to_owned()),
        }
    }
    fn into_domain(self) -> Result<AppConfig, HistoryError> {
        let records = self
            .records
            .into_iter()
            .map(|record| {
                let puzzle: Board = record
                    .puzzle
                    .parse()
                    .map_err(|error| HistoryError::Domain(format!("{error}")))?;
                let solution = record
                    .solution
                    .map(|text| {
                        text.parse::<Board>()
                            .map_err(|error| HistoryError::Domain(error.to_string()))
                    })
                    .transpose()?;
                Record::new(puzzle, solution)
                    .map_err(|error| HistoryError::Domain(error.to_string()))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let catalog = SolverCatalog::try_from(
            self.catalog
                .into_iter()
                .map(|entry| {
                    Ok(SolverDescriptor {
                        id: entry.id.try_into().map_err(|error: ProtocolError| {
                            HistoryError::Domain(error.to_string())
                        })?,
                        tracing: entry.tracing,
                    })
                })
                .collect::<Result<Vec<_>, HistoryError>>()?,
        )
        .map_err(|error| HistoryError::Domain(error.to_string()))?;
        let max_events = NonZeroUsize::new(self.max_events)
            .ok_or_else(|| HistoryError::Domain("event limit must be nonzero".into()))?;
        let max_bytes = NonZeroUsize::new(self.max_bytes)
            .ok_or_else(|| HistoryError::Domain("byte limit must be nonzero".into()))?;
        let speed = NonZeroUsize::new(self.events_per_tick)
            .ok_or_else(|| HistoryError::Domain("playback speed must be nonzero".into()))?;
        let themes = self
            .themes
            .map(|themes| {
                ThemeCatalog::try_from(
                    themes
                        .into_iter()
                        .map(|theme| ThemeDescriptor {
                            id: theme.id,
                            label: theme.label,
                        })
                        .collect::<Vec<_>>(),
                )
                .map_err(|error| HistoryError::Domain(error.to_string()))
            })
            .transpose()?
            .unwrap_or_default();
        AppConfig::new(
            records,
            self.record_index,
            self.label,
            &self.solver,
            catalog,
            HistoryLimits::new(max_events, max_bytes),
            match self.initial_mode {
                ModeWire::Play => Mode::Play,
                ModeWire::Watch => Mode::Watch,
            },
            speed,
            self.tick_interval_ms,
        )
        .and_then(|config| {
            config.with_themes(themes, self.initial_theme.as_deref().unwrap_or("ku"))
        })
        .map_err(|error| HistoryError::Domain(error.to_string()))
    }
}

fn snapshot_value(app: &App) -> Value {
    use crate::app::{
        AppSnapshot, BaseOverlay, OverlayState, Playback, RunState, Severity, StopReason,
    };
    let AppSnapshot {
        mode,
        sidebar_open,
        theme,
        board,
        selected,
        play_board,
        play_selected,
        play_cursor,
        play_events,
        trace_cursor,
        trace_events,
        trace,
        run,
        playback,
        overlay,
        notice,
        record_index,
        solver,
        events_per_tick,
        remaining,
        next_run,
        history_limits,
        quit,
        last_activation,
        logical_time,
        last_command,
        current_run,
        pending_timer,
        pending_cancellations,
        closed,
        last_command_failure,
    } = app.snapshot();
    let failure = |failure: &Failure| serde_json::json!({"kind":match failure.kind{FailureKind::Start=>"start",FailureKind::Execution=>"execution",FailureKind::ProducerClosed=>"producer_closed",FailureKind::Protocol=>"protocol"},"message":failure.message});
    let run = match run {
        RunState::NotStarted => serde_json::json!({"type":"not_started"}),
        RunState::Starting(run) => serde_json::json!({"type":"starting","run":run.get()}),
        RunState::Active(run) => serde_json::json!({"type":"active","run":run.get()}),
        RunState::Finished(result) => {
            serde_json::json!({"type":"finished","result":ResultWire::from_domain(&result)})
        }
        RunState::Stopped(reason) => serde_json::json!({"type":"stopped","reason":match reason {
            StopReason::HistoryLimit=>serde_json::json!({"type":"history_limit"}),
            StopReason::StorageFailure=>serde_json::json!({"type":"storage_failure"}),
            StopReason::InvalidTrace(message)=>serde_json::json!({"type":"invalid_trace","message":message}),
            StopReason::Failed(error)=>serde_json::json!({"type":"failed","failure":failure(&error)}),
            StopReason::Cancelled=>serde_json::json!({"type":"cancelled"}),
        }}),
    };
    let base = |overlay: BaseOverlay| match overlay {
        BaseOverlay::None => serde_json::json!({"type":"none"}),
        BaseOverlay::SolverPicker { index } => {
            serde_json::json!({"type":"solver_picker","index":index})
        }
        BaseOverlay::ThemePicker { index } => {
            serde_json::json!({"type":"theme_picker","index":index})
        }
    };
    let overlay = match overlay {
        OverlayState::Base(overlay) => serde_json::json!({"type":"base","overlay":base(overlay)}),
        OverlayState::Help { return_to } => {
            serde_json::json!({"type":"help","return_to":base(return_to)})
        }
    };
    let mut value = serde_json::json!({
        "mode":match mode{Mode::Play=>"play",Mode::Watch=>"watch"},"board":board.to_string(),"selected":selected.get(),
        "play_board":play_board.to_string(),"play_selected":play_selected.get(),"play_cursor":play_cursor,"play_events":play_events,
        "trace_cursor":trace_cursor,"trace_events":trace_events,"trace":{"snapshot":SnapshotWire::from_domain(trace.snapshot()),"finished":trace.finished().map(ResultWire::from_domain)},
        "run":run,"playback":match playback{Playback::Paused=>"paused",Playback::Playing=>"playing",Playback::AwaitingStep=>"awaiting_step"},
        "overlay":overlay,"notice":{"severity":match notice.severity(){Severity::Info=>"info",Severity::Error=>"error"},"message":notice.message()},
        "record_index":record_index,"solver":solver.as_str(),"events_per_tick":events_per_tick,"remaining":remaining,"next_run":next_run,
        "history_limits":{"max_events":history_limits.max_events().get(),"max_bytes":history_limits.max_bytes().get()},"quit":quit,
        "last_activation":last_activation.map(ActivationId::get),"logical_time_ms":logical_time.as_millis(),"last_command":last_command.map(CommandId::get),
        "last_command_failure":last_command_failure.map(|(command,error)|serde_json::json!({"command":command.get(),"failure":failure(&error)})),
        "closed":closed,"current_run":current_run.map(|(run,command)|(run.get(),command.get())),"pending_timer":pending_timer.map(|(command,time)|(command.get(),time.as_millis())),
        "pending_cancellations":pending_cancellations.into_iter().map(|(run,command)|(run.get(),command.get())).collect::<Vec<_>>()
    });
    // False is the schema-v1 default, preserving strict replay of existing histories.
    if sidebar_open {
        value["sidebar_open"] = Value::Bool(true);
    }
    if theme != "ku" {
        value["theme"] = Value::String(theme);
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_history() -> History {
        let catalog = SolverCatalog::try_from(vec![SolverDescriptor {
            id: "test".parse().unwrap(),
            tracing: true,
        }])
        .unwrap();
        let app = App::new(
            vec![Board::default().into()],
            0,
            "test".into(),
            "test",
            catalog,
        )
        .unwrap();
        History::new(app.config().clone())
    }

    #[test]
    fn job_and_event_budgets_are_inclusive_and_cannot_wrap_on_overflow() {
        let mut history = empty_history();
        let max_events = history.config().history_limits().max_events().get();
        assert!(history.check_budget(1, max_events).is_ok());
        assert!(matches!(
            history.check_budget(1, max_events + 1),
            Err(HistoryError::CountLimit)
        ));
        history.jobs = MAX_JOBS - 1;
        assert!(history.check_budget(1, 0).is_ok());
        assert!(matches!(
            history.check_budget(2, 0),
            Err(HistoryError::CountLimit)
        ));
        assert!(matches!(
            history.check_budget(usize::MAX, 0),
            Err(HistoryError::CountLimit)
        ));
        assert_eq!(history.jobs, MAX_JOBS - 1);
        assert!(history.records().is_empty());
    }

    #[test]
    fn oversized_malformed_json_is_rejected_before_schema_decoding() {
        let oversized = "!".repeat(MAX_JSON_BYTES + 1);
        assert!(matches!(
            History::from_json(&oversized),
            Err(HistoryError::JsonLimit)
        ));
        // The advertised cap is inclusive, so malformed input at the cap reaches JSON validation.
        assert!(matches!(
            History::from_json(&oversized[..MAX_JSON_BYTES]),
            Err(HistoryError::Json(_))
        ));
    }
}
