//! Composable session hooks shared by interactive runs and deterministic dispatch.
//!
//! Hooks inspect immutable application state. Activations can be transformed or
//! suppressed before the reducer, and command execution can be replaced before
//! infrastructure is invoked. The reducer remains the sole owner of transitions.
//!
//! A tuple composes capabilities as nested wrappers: before hooks run left to
//! right, after hooks run right to left, and the left command wrapper surrounds the
//! right wrapper. Nest tuples to compose more than two capabilities.

use std::io;

use super::{
    app::{Activation, App, Command, Completion, Job},
    session::SessionExit,
};

/// Intercepts the lifecycle of a session without owning application state.
///
/// Defaults pass activations and commands through unchanged. Returning `None` from
/// [`Self::before_activation`] suppresses that activation and its reducer commands. A command
/// wrapper may call its handler or return authored jobs instead of invoking
/// infrastructure. Returned jobs reenter the regular activation hooks.
///
/// A hook error stops the operation and propagates to the session caller. The
/// session runner still cancels infrastructure, reports the error, and invokes
/// [`Self::after_session`], even when a before hook failed. Implement cleanup in
/// `after_session` defensively: session initialization may not have succeeded.
pub trait SessionCapability {
    /// Releases resources owned by command replacements, including during Drop.
    /// Must be idempotent and must not panic; ordinary observer hooks are skipped
    /// during unwinding. Cleanup cannot be suppressed by activation transforms.
    fn cancel_infrastructure(&mut self) {}

    /// Runs before the first render or input request.
    fn before_session(&mut self, _app: &App) -> io::Result<()> {
        Ok(())
    }

    /// Transforms or suppresses an activation before the reducer sees it.
    /// Preserve its identity; time may advance but cannot move backwards.
    fn before_activation(
        &mut self,
        _app: &App,
        activation: Activation,
    ) -> io::Result<Option<Activation>> {
        Ok(Some(activation))
    }

    /// Observes the reduced state and the commands about to be executed.
    fn after_activation(
        &mut self,
        _app: &App,
        _activation: &Activation,
        _completion: &Completion,
    ) -> io::Result<()> {
        Ok(())
    }

    /// Wraps one command, optionally replacing its infrastructure or returned activation.
    ///
    /// Invoking the handler zero times permits simulation or suppression. Invoke
    /// it once with the issued command for ordinary forwarding; repeated calls
    /// repeat the external command. Change execution by replacing the handler,
    /// rather than changing the recorded command identity or parameters.
    fn wrap_command(
        &mut self,
        _app: &App,
        command: &Command,
        handler: &mut dyn FnMut(&Command) -> io::Result<Vec<Job>>,
    ) -> io::Result<Vec<Job>> {
        handler(command)
    }

    /// Runs before rendering the current state.
    fn before_render(&mut self, _app: &App) -> io::Result<()> {
        Ok(())
    }

    /// Runs after a successful render.
    fn after_render(&mut self, _app: &App) -> io::Result<()> {
        Ok(())
    }

    /// Observes a session failure before the final lifecycle hook.
    fn on_error(&mut self, _app: &App, _error: &io::Error) {}

    /// Runs after infrastructure cleanup on every session exit.
    ///
    /// A failure here is returned when the session otherwise succeeded. A prior
    /// session failure takes precedence so cleanup does not obscure its cause.
    fn after_session(&mut self, _app: &App, _result: &io::Result<SessionExit>) -> io::Result<()> {
        Ok(())
    }
}

/// A zero-sized capability that forwards everything unchanged.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NoCapability;

impl SessionCapability for NoCapability {}

impl<A: SessionCapability, B: SessionCapability> SessionCapability for (A, B) {
    fn cancel_infrastructure(&mut self) {
        self.1.cancel_infrastructure();
        self.0.cancel_infrastructure();
    }
    fn before_session(&mut self, app: &App) -> io::Result<()> {
        self.0.before_session(app)?;
        self.1.before_session(app)
    }

    fn before_activation(
        &mut self,
        app: &App,
        activation: Activation,
    ) -> io::Result<Option<Activation>> {
        let Some(activation) = self.0.before_activation(app, activation)? else {
            return Ok(None);
        };
        self.1.before_activation(app, activation)
    }

    fn after_activation(
        &mut self,
        app: &App,
        activation: &Activation,
        completion: &Completion,
    ) -> io::Result<()> {
        self.1.after_activation(app, activation, completion)?;
        self.0.after_activation(app, activation, completion)
    }

    fn wrap_command(
        &mut self,
        app: &App,
        command: &Command,
        handler: &mut dyn FnMut(&Command) -> io::Result<Vec<Job>>,
    ) -> io::Result<Vec<Job>> {
        let (outer, inner) = self;
        outer.wrap_command(app, command, &mut |command| {
            inner.wrap_command(app, command, handler)
        })
    }

    fn before_render(&mut self, app: &App) -> io::Result<()> {
        self.0.before_render(app)?;
        self.1.before_render(app)
    }

    fn after_render(&mut self, app: &App) -> io::Result<()> {
        self.1.after_render(app)?;
        self.0.after_render(app)
    }

    fn on_error(&mut self, app: &App, error: &io::Error) {
        self.1.on_error(app, error);
        self.0.on_error(app, error);
    }

    fn after_session(&mut self, app: &App, result: &io::Result<SessionExit>) -> io::Result<()> {
        // Both capabilities must get their cleanup opportunity, even on failure.
        let inner = self.1.after_session(app, result);
        let outer = self.0.after_session(app, result);
        inner.and(outer)
    }
}

/// A borrowed observation of one lifecycle boundary.
///
/// Each observation is valid only during the callback. Collect application
/// snapshots or selected owned values when a test needs to inspect them later.
#[derive(Debug)]
pub enum HookEvent<'a> {
    /// Session initialization is about to start.
    SessionStart {
        /// Model state before initialization.
        app: &'a App,
    },
    /// An activation has arrived before reduction.
    BeforeActivation {
        /// Current immutable model state at this hook boundary.
        app: &'a App,
        /// The activation being observed, with jobs in reduction order.
        activation: &'a Activation,
    },
    /// An activation has been reduced and produced commands.
    AfterActivation {
        /// Current immutable model state at this hook boundary.
        app: &'a App,
        /// The activation being observed, with jobs in reduction order.
        activation: &'a Activation,
        /// The ordered commands emitted by the observed activation.
        completion: &'a Completion,
    },
    /// An infrastructure command is about to execute.
    BeforeCommand {
        /// Current immutable model state.
        app: &'a App,
        /// The command about to execute.
        command: &'a Command,
    },
    /// An infrastructure command returned a follow-up activation or failed.
    AfterCommand {
        /// Current immutable model state at this hook boundary.
        app: &'a App,
        /// The command whose execution is being observed.
        command: &'a Command,
        /// Immediate feedback jobs or the infrastructure failure.
        result: &'a io::Result<Vec<Job>>,
    },
    /// The current state is about to render.
    BeforeRender {
        /// State about to be presented.
        app: &'a App,
    },
    /// The current state rendered successfully.
    AfterRender {
        /// State successfully presented.
        app: &'a App,
    },
    /// Infrastructure was cleaned up and the session is ending.
    SessionEnd {
        /// Current immutable model state at this hook boundary.
        app: &'a App,
        /// The session outcome after infrastructure cleanup.
        result: &'a io::Result<SessionExit>,
    },
    /// The session failed.
    Error {
        /// Model state at the failure boundary.
        app: &'a App,
        /// The runtime or hook failure.
        error: &'a io::Error,
    },
}

/// Adapts a callback into a lifecycle observer.
///
/// This adapter forwards activations and commands. Implement [`SessionCapability`]
/// directly for transformation or infrastructure substitution. Its callback is
/// borrowed synchronously, so tests can capture ordinary mutable collections.
///
/// # Examples
///
/// ```
/// use ku_cli::tui::{app, capability::{FunctionCapability, HookEvent, SessionCapability}};
/// use ku_core::Board;
///
/// let app = app::new(vec![Board::default().into()], 0, "example".into(), "bitmask")?;
/// let mut renders = 0;
/// {
///     let mut observer = FunctionCapability::new(|event: HookEvent<'_>| {
///         if matches!(event, HookEvent::AfterRender { .. }) { renders += 1; }
///         Ok(())
///     });
///     observer.after_render(&app)?;
/// }
/// assert_eq!(renders, 1);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Clone, Copy, Debug)]
pub struct FunctionCapability<F> {
    observe: F,
}

impl<F> FunctionCapability<F> {
    /// Wraps a callback accepting [`HookEvent`] and returning an I/O result.
    pub fn new(observe: F) -> Self {
        Self { observe }
    }

    /// Returns the callback, including any state it owns.
    pub fn into_inner(self) -> F {
        self.observe
    }
}

impl<F> SessionCapability for FunctionCapability<F>
where
    F: for<'a> FnMut(HookEvent<'a>) -> io::Result<()>,
{
    fn before_session(&mut self, app: &App) -> io::Result<()> {
        (self.observe)(HookEvent::SessionStart { app })
    }

    fn before_activation(
        &mut self,
        app: &App,
        activation: Activation,
    ) -> io::Result<Option<Activation>> {
        (self.observe)(HookEvent::BeforeActivation {
            app,
            activation: &activation,
        })?;
        Ok(Some(activation))
    }

    fn after_activation(
        &mut self,
        app: &App,
        activation: &Activation,
        completion: &Completion,
    ) -> io::Result<()> {
        (self.observe)(HookEvent::AfterActivation {
            app,
            activation,
            completion,
        })
    }

    fn wrap_command(
        &mut self,
        app: &App,
        command: &Command,
        handler: &mut dyn FnMut(&Command) -> io::Result<Vec<Job>>,
    ) -> io::Result<Vec<Job>> {
        (self.observe)(HookEvent::BeforeCommand { app, command })?;
        let result = handler(command);
        let observed = (self.observe)(HookEvent::AfterCommand {
            app,
            command,
            result: &result,
        });
        // Preserve infrastructure failures when reporting them also fails.
        let jobs = result?;
        observed?;
        Ok(jobs)
    }

    fn before_render(&mut self, app: &App) -> io::Result<()> {
        (self.observe)(HookEvent::BeforeRender { app })
    }

    fn after_render(&mut self, app: &App) -> io::Result<()> {
        (self.observe)(HookEvent::AfterRender { app })
    }

    fn on_error(&mut self, app: &App, error: &io::Error) {
        // Reporting cannot replace the failure it observes.
        let _ = (self.observe)(HookEvent::Error { app, error });
    }

    fn after_session(&mut self, app: &App, result: &io::Result<SessionExit>) -> io::Result<()> {
        (self.observe)(HookEvent::SessionEnd { app, result })
    }
}
