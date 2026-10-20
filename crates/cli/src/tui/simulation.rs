//! Function-backed adapters for the ordinary session driver.
use std::{cell::RefCell, collections::VecDeque, io, rc::Rc};

use ku_session::{
    history::{History, ReplayMode},
    simulation::{ScheduledJob, SimulationError, VirtualScheduler},
};

use super::{
    app::{Activation, ActivationId, App, AppSnapshot, Command, Completion, Job, LogicalTime},
    capability::SessionCapability,
    input::{Action, InputSource},
    session::{Session, SessionOutput},
};

/// Finite user intent for the ordinary input loop.
#[derive(Clone, Debug, Default)]
pub struct ScriptedInput {
    actions: VecDeque<Action>,
}
impl ScriptedInput {
    /// Queues finite actions in iteration order; exhaustion ends the input loop.
    pub fn new(actions: impl IntoIterator<Item = Action>) -> Self {
        Self {
            actions: actions.into_iter().collect(),
        }
    }
}
impl InputSource for ScriptedInput {
    fn next_action(&mut self) -> io::Result<Option<Action>> {
        Ok(self.actions.pop_front())
    }
}
/// Captures presented state; collections are intentionally unbounded and opt-in.
#[derive(Clone, Debug, Default)]
pub struct RecordingOutput {
    frames: Vec<AppSnapshot>,
}
impl RecordingOutput {
    /// Borrows snapshots in presentation order, including repeated renders.
    pub fn frames(&self) -> &[AppSnapshot] {
        &self.frames
    }
}
impl SessionOutput for RecordingOutput {
    fn present(&mut self, app: &App) -> io::Result<()> {
        self.frames.push(app.snapshot());
        Ok(())
    }
}
/// Records fully transformed activations, their commands, and strict snapshots.
///
/// Place this last in a capability tuple. Portable recordings begin at a fresh
/// configured model; retained mid-session state is reconstructed by replaying its
/// history. Close manually driven sessions to capture final cancellation.
#[derive(Clone, Debug, Default)]
pub struct SessionRecording {
    history: Option<History>,
}
impl SessionRecording {
    /// Borrows the recording after the session initialization hook has run.
    pub fn history(&self) -> Option<&History> {
        self.history.as_ref()
    }
    /// Transfers the recording, if session initialization created one.
    pub fn into_history(self) -> Option<History> {
        self.history
    }
    /// Replays the recording without running solvers or runtime adapters.
    ///
    /// # Errors
    /// Returns an I/O error if recording has not begun or replay diverges.
    pub fn replay(&self, mode: ReplayMode) -> io::Result<App> {
        self.history
            .as_ref()
            .ok_or_else(|| io::Error::other("session has not been recorded"))?
            .replay(mode)
            .map_err(io::Error::other)
    }
}
impl SessionCapability for SessionRecording {
    fn before_session(&mut self, app: &App) -> io::Result<()> {
        if app.last_activation().is_some() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "portable recording must begin at session genesis",
            ));
        }
        self.history = Some(History::new(app.config().clone()));
        Ok(())
    }
    fn after_activation(
        &mut self,
        app: &App,
        activation: &Activation,
        completion: &Completion,
    ) -> io::Result<()> {
        if let Some(history) = &mut self.history {
            history
                .record(activation, completion, app)
                .map_err(io::Error::other)?;
        }
        Ok(())
    }
}
/// Shared virtual time queue used by a command callback and a test driver.
pub type SharedScheduler = Rc<RefCell<VirtualScheduler>>;
/// Replaces semantic commands with authored scheduled jobs without solver work.
///
/// Timer commands are handled by the scheduler. The callback supplies solver
/// lifecycle and trace responses, including explicit failures and races.
pub struct FunctionCommands<F> {
    scheduler: SharedScheduler,
    function: F,
}
impl<F> FunctionCommands<F> {
    /// Wraps a command callback with a fresh shared virtual scheduler.
    pub fn new(function: F) -> Self {
        Self {
            scheduler: Rc::new(RefCell::new(VirtualScheduler::new())),
            function,
        }
    }
    /// Clones the shared scheduler handle for a simulation driver or job injector.
    pub fn scheduler(&self) -> SharedScheduler {
        Rc::clone(&self.scheduler)
    }
}
impl<F: FnMut(&Command, LogicalTime) -> Result<Vec<ScheduledJob>, SimulationError>>
    SessionCapability for FunctionCommands<F>
{
    fn wrap_command(
        &mut self,
        app: &App,
        command: &Command,
        _: &mut dyn FnMut(&Command) -> io::Result<Vec<Job>>,
    ) -> io::Result<Vec<Job>> {
        let now = app.logical_time();
        self.scheduler
            .borrow_mut()
            .execute_with(command, &mut |command| (self.function)(command, now))
            .map_err(io::Error::other)?;
        Ok(Vec::new())
    }
    fn after_activation(&mut self, app: &App, _: &Activation, _: &Completion) -> io::Result<()> {
        let frontier = self.scheduler.borrow().now().max(app.logical_time());
        self.scheduler
            .borrow_mut()
            .advance_to(frontier)
            .map_err(io::Error::other)
    }
    fn cancel_infrastructure(&mut self) {
        self.scheduler.borrow_mut().clear();
    }
}
/// Advances virtual deadlines through the real driver with an explicit work bound.
///
/// Inputs can be scheduled as jobs, including Quit. Empty queues return control
/// to the caller so another action or race can be injected before continuing.
pub fn simulate(
    session: &mut Session<'_>,
    scheduler: &SharedScheduler,
    max_activations: usize,
) -> io::Result<usize> {
    session.present()?;
    for count in 0..max_activations {
        if session.app().quit() {
            return Ok(count);
        }
        let id = session
            .app()
            .last_activation()
            .map_or(Ok(ActivationId::FIRST), ActivationId::checked_next)
            .map_err(io::Error::other)?;
        let batch = scheduler.borrow_mut().next_jobs();
        let Some((time, jobs)) = batch else {
            return Ok(count);
        };
        let activation = Activation::new(id, time.max(session.app().logical_time()), jobs)
            .map_err(io::Error::other)?;
        session.dispatch_activation(activation)?;
        session.present()?;
    }
    if scheduler.borrow().is_empty() || session.app().quit() {
        return Ok(max_activations);
    }
    Err(io::Error::other(
        "virtual session exceeded activation budget",
    ))
}
