//! Deterministic session state, semantic commands, and portable replay.
//!
//! The kernel owns decisions; runtime adapters execute the returned commands.
//! Replay supplies recorded outcomes and never calls a solver or an I/O adapter.
//!
//! Start with [`app::App`] and a validated [`app::SolverCatalog`]. Deliver ordered
//! [`protocol::Activation`] batches, execute their returned [`protocol::Command`]s
//! in a runtime adapter, and feed results back as [`protocol::Job`]s.
//! [`simulation::VirtualScheduler`] provides deterministic time and authored
//! responses. [`history::History`] records the same protocol for portable replay.
//!
//! # Recording and replaying a session
//!
//! ```
//! use ku_core::Board;
//! use ku_session::{
//!     action::Action,
//!     app::{App, SolverCatalog, SolverDescriptor},
//!     history::{History, ReplayMode},
//!     protocol::{Activation, ActivationId, LogicalTime},
//! };
//!
//! let catalog = SolverCatalog::try_from(vec![SolverDescriptor {
//!     id: "example".parse()?, tracing: true,
//! }])?;
//! let mut app = App::new(vec![Board::default().into()], 0, "example".into(), "example", catalog)?;
//! let mut history = History::new(app.config().clone());
//! let activation = Activation::new(ActivationId::FIRST, LogicalTime::ZERO,
//!     vec![Action::ToggleMode.into()])?;
//! let completion = app.activate(&activation)?;
//! history.record(&activation, &completion, &app)?;
//! let saved = history.to_json()?;
//! let replayed = History::from_json(&saved)?.replay(ReplayMode::Strict)?;
//! assert_eq!(replayed.snapshot(), app.snapshot());
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
pub mod action;
pub mod app;
pub mod history;
pub mod protocol;
pub mod simulation;
mod wire;
