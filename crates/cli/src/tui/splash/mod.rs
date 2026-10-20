//! Terminal startup presentation with a deterministic, replaceable clock and input.

use std::{io, time::Duration};

use ratatui::{
    Terminal,
    backend::Backend,
    crossterm::event::{Event, KeyEventKind},
};

use super::{
    input::{Clock, EventReader},
    theme::Palette,
};

pub mod render;

/// Maximum time spent on the opening screen; any keypress skips it.
pub const SPLASH_DURATION: Duration = Duration::from_millis(1600);

/// The input or deadline that dismissed the opening screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SplashExit {
    /// The fixed presentation duration elapsed.
    Timeout,
    /// A keypress dismissed the screen and was consumed.
    Key,
}

/// Deterministic startup state, independent of terminal I/O and wall-clock time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SplashState {
    deadline: Duration,
    exit: Option<SplashExit>,
}

impl SplashState {
    /// Starts the opening screen at an explicit monotonic time.
    pub fn new(now: Duration) -> Self {
        Self {
            deadline: now.saturating_add(SPLASH_DURATION),
            exit: None,
        }
    }

    /// Time left before automatic dismissal. Events never extend the deadline.
    pub fn remaining(&self, now: Duration) -> Duration {
        if self.exit.is_some() {
            Duration::ZERO
        } else {
            self.deadline.saturating_sub(now)
        }
    }

    /// Applies explicit time and optional input, preserving the first dismissal.
    /// Releases and non-key events do not dismiss the screen. At the deadline,
    /// timeout takes precedence over an input arriving at the same time.
    pub fn update(&mut self, now: Duration, event: Option<&Event>) -> Option<SplashExit> {
        if self.exit.is_none() {
            self.exit = if now >= self.deadline {
                Some(SplashExit::Timeout)
            } else if matches!(event, Some(Event::Key(key)) if key.kind != KeyEventKind::Release) {
                Some(SplashExit::Key)
            } else {
                None
            };
        }
        self.exit
    }
}

/// Presents the startup screen using replaceable terminal, clock, and input adapters.
/// The dismissing key is consumed; resize redraws retain the original deadline.
///
/// # Errors
/// Propagates terminal drawing and input polling/reading errors to the caller.
pub fn show<B: Backend>(
    terminal: &mut Terminal<B>,
    clock: &impl Clock,
    reader: &mut impl EventReader,
) -> io::Result<SplashExit> {
    show_with_palette(terminal, clock, reader, Palette::default())
}

/// Presents the startup screen using the selected immutable palette.
/// The dismissing key is consumed; resize redraws retain the original deadline.
///
/// # Errors
/// Propagates terminal drawing and input polling/reading errors to the caller.
pub fn show_with_palette<B: Backend>(
    terminal: &mut Terminal<B>,
    clock: &impl Clock,
    reader: &mut impl EventReader,
    palette: Palette,
) -> io::Result<SplashExit> {
    let mut splash = SplashState::new(clock.now());
    terminal
        .draw(|frame| render::render_with_palette(frame, palette))
        .map_err(|error| io::Error::other(error.to_string()))?;
    loop {
        let now = clock.now();
        if let Some(exit) = splash.update(now, None) {
            return Ok(exit);
        }
        if reader.poll(splash.remaining(now))? {
            let event = reader.read()?;
            if let Some(exit) = splash.update(clock.now(), Some(&event)) {
                return Ok(exit);
            }
            if matches!(event, Event::Resize(_, _)) {
                terminal
                    .draw(|frame| render::render_with_palette(frame, palette))
                    .map_err(|error| io::Error::other(error.to_string()))?;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{cell::Cell, collections::VecDeque, rc::Rc};

    use ratatui::{
        backend::TestBackend,
        crossterm::event::{KeyCode, KeyEvent, KeyModifiers},
    };

    use super::*;
    use crate::tui::input::{Action, InputSource, ScheduledInput};

    type TimedEvent = (Duration, io::Result<Event>);

    struct Reader {
        now: Rc<Cell<Duration>>,
        events: VecDeque<TimedEvent>,
        poll_error: Option<io::ErrorKind>,
    }

    impl EventReader for Reader {
        fn poll(&mut self, timeout: Duration) -> io::Result<bool> {
            if let Some(kind) = self.poll_error.take() {
                return Err(kind.into());
            }
            let deadline = self.now.get() + timeout;
            if let Some((at, _)) = self.events.front()
                && *at <= deadline
            {
                self.now.set(self.now.get().max(*at));
                Ok(true)
            } else {
                self.now.set(deadline);
                Ok(false)
            }
        }

        fn read(&mut self) -> io::Result<Event> {
            self.events.pop_front().expect("successful poll").1
        }
    }

    fn reader(now: &Rc<Cell<Duration>>, events: impl IntoIterator<Item = TimedEvent>) -> Reader {
        Reader {
            now: Rc::clone(now),
            events: events.into_iter().collect(),
            poll_error: None,
        }
    }

    fn key(kind: KeyEventKind) -> Event {
        Event::Key(KeyEvent::new_with_kind(
            KeyCode::Char('x'),
            KeyModifiers::NONE,
            kind,
        ))
    }

    #[test]
    fn timeout_uses_elapsed_time_from_start_and_cannot_reopen() {
        let start = Duration::from_secs(10);
        let mut state = SplashState::new(start);
        assert_eq!(
            state.update(start + SPLASH_DURATION - Duration::from_nanos(1), None),
            None
        );
        assert_eq!(
            state.update(start + SPLASH_DURATION, None),
            Some(SplashExit::Timeout)
        );
        assert_eq!(
            state.update(start, Some(&key(KeyEventKind::Press))),
            Some(SplashExit::Timeout)
        );
    }

    #[test]
    fn any_keypress_or_repeat_dismisses_but_release_does_not() {
        for kind in [KeyEventKind::Press, KeyEventKind::Repeat] {
            let mut state = SplashState::new(Duration::ZERO);
            assert_eq!(
                state.update(Duration::ZERO, Some(&key(KeyEventKind::Release))),
                None
            );
            assert_eq!(
                state.update(Duration::ZERO, Some(&key(kind))),
                Some(SplashExit::Key)
            );
            assert_eq!(state.remaining(Duration::ZERO), Duration::ZERO);
        }
    }

    #[test]
    fn resize_and_ignored_events_do_not_extend_the_timeout() {
        let now = Rc::new(Cell::new(Duration::from_secs(4)));
        let clock = || now.get();
        let mut input = reader(
            &now,
            [
                (Duration::from_millis(4100), Ok(Event::Resize(90, 26))),
                (Duration::from_millis(4200), Ok(Event::FocusGained)),
                (Duration::from_millis(4300), Ok(key(KeyEventKind::Release))),
            ],
        );
        let mut terminal = Terminal::new(TestBackend::new(90, 26)).unwrap();
        assert_eq!(
            show(&mut terminal, &clock, &mut input).unwrap(),
            SplashExit::Timeout
        );
        assert_eq!(now.get(), Duration::from_secs(4) + SPLASH_DURATION);
    }

    #[test]
    fn chosen_palette_survives_resize_redraws_until_timeout() {
        let now = Rc::new(Cell::new(Duration::ZERO));
        let clock = || now.get();
        let mut input = reader(
            &now,
            [(Duration::from_millis(100), Ok(Event::Resize(90, 26)))],
        );
        let mut terminal = Terminal::new(TestBackend::new(90, 26)).unwrap();
        let palette = Palette::for_id("dracula");
        assert_eq!(
            show_with_palette(&mut terminal, &clock, &mut input, palette).unwrap(),
            SplashExit::Timeout,
        );
        assert_eq!(terminal.backend().buffer()[(34, 7)].fg, palette.accent);
        assert!(
            terminal
                .backend()
                .buffer()
                .content
                .iter()
                .all(|cell| cell.bg == palette.table)
        );
        assert_eq!(now.get(), SPLASH_DURATION);
    }

    #[test]
    fn dismissing_key_is_consumed_and_game_input_starts_on_a_fresh_cadence() {
        let now = Rc::new(Cell::new(Duration::ZERO));
        let clock = || now.get();
        let mut input = reader(
            &now,
            [
                (Duration::from_millis(100), Ok(key(KeyEventKind::Press))),
                (
                    Duration::from_millis(120),
                    Ok(Event::Key(KeyEvent::new(
                        KeyCode::Char('q'),
                        KeyModifiers::NONE,
                    ))),
                ),
            ],
        );
        let mut terminal = Terminal::new(TestBackend::new(90, 26)).unwrap();
        assert_eq!(
            show(&mut terminal, &clock, &mut input).unwrap(),
            SplashExit::Key
        );
        let mut input = ScheduledInput::new(clock, input, Duration::from_millis(50)).unwrap();
        assert_eq!(input.next_action().unwrap(), Some(Action::Quit));
        assert_eq!(input.next_action().unwrap(), Some(Action::Tick));
        assert_eq!(now.get(), Duration::from_millis(150));
    }

    #[test]
    fn input_errors_propagate_without_waiting_out_the_deadline() {
        for fail_poll in [true, false] {
            let now = Rc::new(Cell::new(Duration::ZERO));
            let clock = || now.get();
            let mut input = reader(
                &now,
                [(
                    Duration::from_millis(1),
                    Err(io::ErrorKind::BrokenPipe.into()),
                )],
            );
            if fail_poll {
                input.poll_error = Some(io::ErrorKind::BrokenPipe);
            }
            let mut terminal = Terminal::new(TestBackend::new(90, 26)).unwrap();
            assert_eq!(
                show(&mut terminal, &clock, &mut input).unwrap_err().kind(),
                io::ErrorKind::BrokenPipe
            );
            assert!(now.get() < SPLASH_DURATION);
        }
    }
}
