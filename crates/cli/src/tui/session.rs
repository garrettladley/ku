//! The shared live and simulated activation/completion driver.
use std::{collections::VecDeque, io};

use ku_core::{TraceEvent, TraceSolver};
use ku_solvers::solver_registry;
use ratatui::{Terminal, backend::Backend};

use super::{
    app::{
        Activation, ActivationId, App, CommandId, CommandKind, Failure, FailureKind, Job,
        LogicalTime, RunId, SessionEndReason, SolverId,
    },
    capability::SessionCapability,
    input::{Action, InputSource},
    render,
    source::{SolverEventSource, SourcePoll},
};

/// Why an otherwise successful session ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionExit {
    /// The user or model requested exit.
    Quit,
    /// The input source reached its end.
    InputEnded,
}
/// Presents immutable state to a terminal, recorder, or function.
pub trait SessionOutput {
    /// Presents one immutable state, returning any output error to the driver.
    fn present(&mut self, app: &App) -> io::Result<()>;
}
impl<F: FnMut(&App) -> io::Result<()>> SessionOutput for F {
    fn present(&mut self, app: &App) -> io::Result<()> {
        self(app)
    }
}
/// Resolves stable identities only at the infrastructure boundary.
pub trait SolverResolver {
    /// Returns the trace-capable strategy for an identity, or `None` if unavailable.
    fn resolve(&self, id: &SolverId) -> Option<&'static dyn TraceSolver>;
}
impl<F: Fn(&SolverId) -> Option<&'static dyn TraceSolver>> SolverResolver for F {
    fn resolve(&self, id: &SolverId) -> Option<&'static dyn TraceSolver> {
        self(id)
    }
}
/// Resolves trace-capable strategies from the built-in registry.
#[derive(Clone, Copy, Debug, Default)]
pub struct BuiltInResolver;
impl SolverResolver for BuiltInResolver {
    fn resolve(&self, id: &SolverId) -> Option<&'static dyn TraceSolver> {
        solver_registry()
            .iter()
            .find(|entry| entry.name() == id.as_str())
            .and_then(|entry| entry.tracer())
    }
}
/// Ratatui output using any backend, including `TestBackend`.
pub struct TerminalOutput<'a, B: Backend> {
    terminal: &'a mut Terminal<B>,
}
impl<'a, B: Backend> TerminalOutput<'a, B> {
    /// Borrows a terminal without changing its raw-mode or screen lifecycle.
    pub fn new(terminal: &'a mut Terminal<B>) -> Self {
        Self { terminal }
    }
}
impl<B: Backend> SessionOutput for TerminalOutput<'_, B> {
    fn present(&mut self, app: &App) -> io::Result<()> {
        self.terminal
            .draw(|frame| render::render(frame, app))
            .map(|_| ())
            .map_err(|error| io::Error::other(error.to_string()))
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Lifecycle {
    New,
    Running,
    Closed,
}

/// Borrows independent transport, output, resolution, and capability adapters.
///
/// Live input and explicit virtual activations share this driver. Transport polls
/// never enter the model or recordings; only actual outcomes become jobs. Hooks
/// can replace command execution with authored responses. Closing always cancels
/// infrastructure and records a mandatory final activation, even after Quit or a
/// hook failure. Drop cleans up without invoking user hooks during unwinding.
pub struct Session<'a> {
    app: &'a mut App,
    input: &'a mut dyn InputSource,
    source: &'a mut dyn SolverEventSource,
    output: &'a mut dyn SessionOutput,
    capability: &'a mut dyn SessionCapability,
    resolver: &'a dyn SolverResolver,
    lifecycle: Lifecycle,
    transport_run: Option<(RunId, CommandId)>,
}
impl<'a> Session<'a> {
    /// Borrows runtime adapters and uses the built-in strategy resolver.
    /// Initialization hooks run lazily on the first operation.
    pub fn new(
        app: &'a mut App,
        input: &'a mut dyn InputSource,
        source: &'a mut dyn SolverEventSource,
        output: &'a mut dyn SessionOutput,
        capability: &'a mut dyn SessionCapability,
    ) -> Self {
        Self {
            app,
            input,
            source,
            output,
            capability,
            resolver: &BuiltInResolver,
            lifecycle: Lifecycle::New,
            transport_run: None,
        }
    }
    /// Supplies a custom strategy resolver before command execution.
    pub fn with_resolver(mut self, resolver: &'a dyn SolverResolver) -> Self {
        self.resolver = resolver;
        self
    }
    /// Borrows the current model state.
    pub fn app(&self) -> &App {
        self.app
    }
    /// Runs the live input loop; Tick is normalized to an identified due timer.
    pub fn run(&mut self) -> io::Result<SessionExit> {
        let result = (|| {
            self.start()?;
            loop {
                self.render()?;
                if self.app.quit() {
                    return Ok(SessionExit::Quit);
                }
                let Some(action) = self.input.next_action()? else {
                    return Ok(SessionExit::InputEnded);
                };
                let time = self
                    .input
                    .elapsed()
                    .map(|elapsed| {
                        u64::try_from(elapsed.as_millis())
                            .map(LogicalTime::from_millis)
                            .map_err(io::Error::other)
                    })
                    .transpose()?;
                self.dispatch_input_at(action, time)?;
            }
        })();
        self.finish(result)
    }
    /// Dispatches a semantic job and any immediate runtime feedback.
    pub fn dispatch(&mut self, job: Job) -> io::Result<()> {
        let result = (|| {
            self.start()?;
            if let Job::Action(action) = job {
                return self.dispatch_input(action);
            }
            let activation = self.envelope(self.app.logical_time(), vec![job])?;
            self.reduce(activation, true)
        })();
        self.finish_error(result)
    }
    /// Feeds an explicit activation, including virtual time, through ordinary hooks.
    pub fn dispatch_activation(&mut self, activation: Activation) -> io::Result<()> {
        let result = self.start().and_then(|()| self.reduce(activation, true));
        self.finish_error(result)
    }
    /// Starts the session if needed and presents state through render hooks.
    ///
    /// # Errors
    /// Propagates initialization, hook, and output errors after session cleanup.
    pub fn present(&mut self) -> io::Result<()> {
        let result = self.start().and_then(|()| self.render());
        self.finish_error(result)
    }
    /// Ends a manually driven session, cancelling infrastructure and running final hooks.
    ///
    /// # Errors
    /// Propagates initialization, final activation, or cleanup hook errors.
    pub fn close(&mut self, exit: SessionExit) -> io::Result<SessionExit> {
        let result = self.start().map(|()| exit);
        self.finish(result)
    }
    fn start(&mut self) -> io::Result<()> {
        match self.lifecycle {
            Lifecycle::New => {
                self.lifecycle = Lifecycle::Running;
                self.capability.before_session(self.app)
            }
            Lifecycle::Running => Ok(()),
            Lifecycle::Closed => Err(io::Error::new(
                io::ErrorKind::NotConnected,
                "session is closed",
            )),
        }
    }
    fn envelope(&self, time: LogicalTime, jobs: Vec<Job>) -> io::Result<Activation> {
        let id = self
            .app
            .last_activation()
            .map_or(Ok(ActivationId::FIRST), ActivationId::checked_next)
            .map_err(io::Error::other)?;
        Activation::new(id, time, jobs).map_err(io::Error::other)
    }
    fn dispatch_input(&mut self, action: Action) -> io::Result<()> {
        self.dispatch_input_at(action, None)
    }
    fn dispatch_input_at(
        &mut self,
        action: Action,
        observed: Option<LogicalTime>,
    ) -> io::Result<()> {
        let (time, job) = match action {
            Action::Resize => return Ok(()),
            Action::Tick => match self.app.pending_timer() {
                Some((command, deadline)) => {
                    let time = observed.unwrap_or(deadline.max(self.app.logical_time()));
                    if time < deadline {
                        return Ok(());
                    }
                    (time, Job::TimerFired { command })
                }
                None => return Ok(()),
            },
            action => (
                observed.unwrap_or(self.app.logical_time()),
                Job::Action(action),
            ),
        };
        self.reduce(self.envelope(time, vec![job])?, true)
    }
    fn reduce(&mut self, first: Activation, hooks: bool) -> io::Result<()> {
        let mut pending = VecDeque::from([first]);
        while let Some(activation) = pending.pop_front() {
            let offered = (activation.id(), activation.time());
            let activation = if hooks {
                let Some(activation) = self.capability.before_activation(self.app, activation)?
                else {
                    continue;
                };
                activation
            } else {
                activation
            };
            if activation.id() != offered.0 || activation.time() < offered.1 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "activation hooks must preserve identity and cannot move time backwards",
                ));
            }
            let completion = self.app.activate(&activation).map_err(io::Error::other)?;
            self.capability
                .after_activation(self.app, &activation, &completion)?;
            let mut feedback = Vec::new();
            for command in completion.commands() {
                let source = &mut self.source;
                let transport_run = &mut self.transport_run;
                let resolver = self.resolver;
                let result = self
                    .capability
                    .wrap_command(self.app, command, &mut |executed| {
                        if executed != command {
                            return Err(io::Error::new(
                                io::ErrorKind::InvalidInput,
                                "command handlers must execute the issued command",
                            ));
                        }
                        match &executed.kind {
                            CommandKind::StartSolver {
                                run,
                                solver,
                                puzzle,
                            } => {
                                let tracer = resolver.resolve(solver).ok_or_else(|| {
                                    io::Error::new(
                                        io::ErrorKind::Unsupported,
                                        format!("unknown trace solver: {solver}"),
                                    )
                                })?;
                                source.start(tracer, puzzle)?;
                                *transport_run = Some((*run, command.id));
                                Ok(vec![Job::SolverStarted {
                                    run: *run,
                                    command: command.id,
                                }])
                            }
                            CommandKind::CancelSolver { run } => {
                                if transport_run.is_some_and(|(active, _)| active == *run) {
                                    source.cancel();
                                    *transport_run = None;
                                }
                                Ok(vec![Job::SolverCancelled {
                                    run: *run,
                                    command: command.id,
                                }])
                            }
                            CommandKind::ScheduleTimer { .. } => Ok(Vec::new()),
                        }
                    });
                match result {
                    Ok(jobs) => feedback.extend(jobs),
                    Err(error) => {
                        let kind = if matches!(command.kind, CommandKind::StartSolver { .. }) {
                            FailureKind::Start
                        } else {
                            FailureKind::Execution
                        };
                        let failed = self.envelope(
                            self.app.logical_time(),
                            vec![Job::CommandFailed {
                                command: command.id,
                                failure: Failure {
                                    kind,
                                    message: error.to_string(),
                                },
                            }],
                        )?;
                        // Record command failures even if an ordinary before hook failed.
                        let _ = self.reduce(failed, false);
                        return Err(error);
                    }
                }
            }
            if !feedback.is_empty() {
                pending.push_back(self.envelope(self.app.logical_time(), feedback)?);
            }
            // Drain only after command acknowledgments, using bounded requested work.
            if pending.is_empty()
                && let Some(job) = self.poll_job()?
            {
                pending.push_back(self.envelope(self.app.logical_time(), vec![job])?);
            }
        }
        Ok(())
    }
    fn poll_job(&mut self) -> io::Result<Option<Job>> {
        let Some((run, command)) = self
            .transport_run
            .filter(|active| Some(*active) == self.app.current_run())
        else {
            return Ok(None);
        };
        if self.app.requested_events() == 0 {
            return Ok(None);
        }
        match self.source.poll_event() {
            Ok(SourcePoll::Pending) => Ok(None),
            Ok(SourcePoll::Closed) => Ok(Some(Job::SolverFailed {
                run,
                command,
                failure: Failure {
                    kind: FailureKind::ProducerClosed,
                    message: "Solver stopped without a finish event.".into(),
                },
            })),
            Ok(SourcePoll::Event(event)) => {
                if let Err(error) = self.app.trace_history().validate_next(&event) {
                    return Ok(Some(Job::SolverFailed {
                        run,
                        command,
                        failure: Failure {
                            kind: FailureKind::Protocol,
                            message: error.to_string(),
                        },
                    }));
                }
                Ok(Some(match event {
                    TraceEvent::Finished { result, snapshot } => Job::SolverCompleted {
                        run,
                        command,
                        result,
                        snapshot,
                    },
                    event => Job::SolverEvents {
                        run,
                        command,
                        events: vec![event],
                    },
                }))
            }
            Err(error) => {
                let activation = self.envelope(
                    self.app.logical_time(),
                    vec![Job::SolverFailed {
                        run,
                        command,
                        failure: Failure {
                            kind: FailureKind::Execution,
                            message: error.to_string(),
                        },
                    }],
                )?;
                let _ = self.reduce(activation, false);
                Err(error)
            }
        }
    }
    fn render(&mut self) -> io::Result<()> {
        self.capability.before_render(self.app)?;
        self.output.present(self.app)?;
        self.capability.after_render(self.app)
    }
    fn finish_error(&mut self, result: io::Result<()>) -> io::Result<()> {
        match result {
            Ok(()) => Ok(()),
            Err(error) => self.finish(Err(error)).map(|_| ()),
        }
    }
    fn finish(&mut self, mut result: io::Result<SessionExit>) -> io::Result<SessionExit> {
        if self.lifecycle == Lifecycle::Closed {
            return result;
        }
        self.lifecycle = Lifecycle::Closed;
        self.source.cancel();
        self.capability.cancel_infrastructure();
        self.transport_run = None;
        let reason = match result {
            Ok(SessionExit::Quit) => SessionEndReason::Quit,
            Ok(SessionExit::InputEnded) => SessionEndReason::InputEnded,
            Err(_) => SessionEndReason::Error,
        };
        let finalized = self.finalize(reason, true);
        if result.is_ok()
            && let Err(error) = finalized
        {
            result = Err(error);
        }
        if let Err(error) = &result {
            self.capability.on_error(self.app, error);
        }
        let final_hook = self.capability.after_session(self.app, &result);
        let exit = result?;
        if let Err(error) = final_hook {
            self.capability.on_error(self.app, &error);
            return Err(error);
        }
        Ok(exit)
    }
    fn finalize(&mut self, reason: SessionEndReason, observe: bool) -> io::Result<()> {
        let activation =
            self.envelope(self.app.logical_time(), vec![Job::SessionEnded { reason }])?;
        let completion = self.app.activate(&activation).map_err(io::Error::other)?;
        let mut observed = if observe {
            self.capability
                .after_activation(self.app, &activation, &completion)
        } else {
            Ok(())
        };
        let jobs: Vec<_> = self
            .app
            .snapshot()
            .pending_cancellations
            .into_iter()
            .map(|(run, command)| Job::SolverCancelled { run, command })
            .collect();
        if !jobs.is_empty() {
            let activation = self.envelope(self.app.logical_time(), jobs)?;
            let completion = self.app.activate(&activation).map_err(io::Error::other)?;
            if observe {
                let acknowledged =
                    self.capability
                        .after_activation(self.app, &activation, &completion);
                if observed.is_ok() {
                    observed = acknowledged;
                }
            }
        }
        observed
    }
}
impl Drop for Session<'_> {
    fn drop(&mut self) {
        if self.lifecycle != Lifecycle::Closed {
            self.source.cancel();
            self.capability.cancel_infrastructure();
            self.transport_run = None;
            let _ = self.finalize(SessionEndReason::Dropped, false);
        }
    }
}
