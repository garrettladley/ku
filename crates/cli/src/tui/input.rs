//! Terminal input translated into deterministic application actions.

use std::{
    io,
    time::{Duration, Instant},
};

use ku_core::Digit;
pub use ku_session::action::Action;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};

/// Substitute scripted actions here to exercise the complete application loop.
pub trait InputSource {
    /// Returns the next action; `None` marks the end of the input stream.
    fn next_action(&mut self) -> io::Result<Option<Action>>;

    /// Elapsed monotonic time for the last delivered input, when available.
    /// Scripted sources may omit it and advance timers through explicit ticks.
    fn elapsed(&self) -> Option<Duration> {
        None
    }
}

impl<F> InputSource for F
where
    F: FnMut() -> io::Result<Option<Action>>,
{
    fn next_action(&mut self) -> io::Result<Option<Action>> {
        self()
    }
}

/// Supplies elapsed time from a monotonic origin.
pub trait Clock {
    /// Returned times must never decrease during an input session.
    fn now(&self) -> Duration;
}

impl<F: Fn() -> Duration> Clock for F {
    fn now(&self) -> Duration {
        self()
    }
}

/// Supplies terminal events independently of the input scheduler.
pub trait EventReader {
    /// Waits up to `timeout` for an event without consuming it.
    fn poll(&mut self, timeout: Duration) -> io::Result<bool>;

    /// Consumes an event after a successful poll.
    fn read(&mut self) -> io::Result<Event>;
}

/// The terminal's playback and redraw cadence.
pub const DEFAULT_TICK_INTERVAL: Duration = Duration::from_millis(50);

/// Maps terminal events and emits ticks on deadlines independent of keyboard activity.
///
/// Clock and reader implementations can share a simulated clock to exercise the
/// production scheduler without sleeping or opening a terminal.
pub struct ScheduledInput<C, R> {
    clock: C,
    reader: R,
    interval: Duration,
    next_tick: Duration,
}

impl<C: Clock, R: EventReader> ScheduledInput<C, R> {
    /// Creates an input scheduler with a nonzero tick interval.
    ///
    /// # Errors
    /// Returns [`io::ErrorKind::InvalidInput`] for a zero interval.
    pub fn new(clock: C, reader: R, interval: Duration) -> io::Result<Self> {
        if interval.is_zero() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "tick interval must be nonzero",
            ));
        }
        let next_tick = clock.now().saturating_add(interval);
        Ok(Self {
            clock,
            reader,
            interval,
            next_tick,
        })
    }
}

impl<C: Clock, R: EventReader> InputSource for ScheduledInput<C, R> {
    fn elapsed(&self) -> Option<Duration> {
        Some(self.clock.now())
    }
    fn next_action(&mut self) -> io::Result<Option<Action>> {
        loop {
            let now = self.clock.now();
            if now >= self.next_tick {
                // Rebase after a delayed frame so lag cannot create a burst of ticks.
                self.next_tick = now.saturating_add(self.interval);
                return Ok(Some(Action::Tick));
            }
            if self.reader.poll(self.next_tick.saturating_sub(now))?
                && let Some(action) = map_event(self.reader.read()?)
            {
                return Ok(Some(action));
            }
        }
    }
}

/// Elapsed wall-clock time, unaffected by system clock adjustments.
pub struct MonotonicClock {
    origin: Instant,
}

impl Default for MonotonicClock {
    fn default() -> Self {
        Self {
            origin: Instant::now(),
        }
    }
}

impl Clock for MonotonicClock {
    fn now(&self) -> Duration {
        self.origin.elapsed()
    }
}

/// Crossterm's blocking terminal event adapter.
#[derive(Default)]
pub struct CrosstermEvents;

impl EventReader for CrosstermEvents {
    fn poll(&mut self, timeout: Duration) -> io::Result<bool> {
        event::poll(timeout)
    }

    fn read(&mut self) -> io::Result<Event> {
        event::read()
    }
}

/// Synchronous terminal input using a monotonic clock and Crossterm events.
pub type CrosstermInput = ScheduledInput<MonotonicClock, CrosstermEvents>;

impl Default for CrosstermInput {
    fn default() -> Self {
        let clock = MonotonicClock::default();
        let next_tick = clock.now().saturating_add(DEFAULT_TICK_INTERVAL);
        Self {
            clock,
            reader: CrosstermEvents,
            interval: DEFAULT_TICK_INTERVAL,
            next_tick,
        }
    }
}

/// Maps a keyboard or resize event without inspecting application state.
pub fn map_event(event: Event) -> Option<Action> {
    let Event::Key(key) = event else {
        return matches!(event, Event::Resize(_, _)).then_some(Action::Resize);
    };
    if key.kind == KeyEventKind::Release {
        return None;
    }
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        return matches!(key.code, KeyCode::Char('c') | KeyCode::Char('C')).then_some(Action::Quit);
    }
    if key.modifiers.contains(KeyModifiers::ALT) {
        return None;
    }
    Some(match key.code {
        KeyCode::Up | KeyCode::Char('k') => Action::Move {
            rows: -1,
            columns: 0,
        },
        KeyCode::Down | KeyCode::Char('j') => Action::Move {
            rows: 1,
            columns: 0,
        },
        KeyCode::Left | KeyCode::Char('h') => Action::Move {
            rows: 0,
            columns: -1,
        },
        KeyCode::Right | KeyCode::Char('l') => Action::Move {
            rows: 0,
            columns: 1,
        },
        KeyCode::Char(digit @ '1'..='9') => {
            Action::Digit(Digit::try_from(digit as u8 - b'0').ok()?)
        }
        KeyCode::Backspace | KeyCode::Delete | KeyCode::Char('0' | '.') => Action::Clear,
        KeyCode::Tab => Action::ToggleMode,
        KeyCode::Char('i') => Action::ToggleSidebar,
        KeyCode::Char(' ') => Action::ToggleRun,
        KeyCode::Char('+' | '=') => Action::Faster,
        KeyCode::Char('-') => Action::Slower,
        KeyCode::Char(']') => Action::StepForward,
        KeyCode::Char('[') => Action::StepBack,
        KeyCode::Home => Action::HistoryStart,
        KeyCode::End => Action::HistoryEnd,
        KeyCode::Char('r') => Action::Reset,
        KeyCode::Char('s') => Action::ChooseSolver,
        KeyCode::Char('t') => Action::ChooseTheme,
        KeyCode::Char('n') => Action::NextPuzzle,
        KeyCode::Char('?') | KeyCode::F(1) => Action::Help,
        KeyCode::Char('q') => Action::Quit,
        KeyCode::Enter => Action::Accept,
        KeyCode::Esc => Action::Escape,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use std::{cell::Cell, collections::VecDeque, rc::Rc};

    use ratatui::crossterm::event::KeyEvent;

    use super::*;

    #[test]
    fn maps_digits_navigation_and_playback_commands() {
        for (code, action) in [
            (
                KeyCode::Char('8'),
                Action::Digit(Digit::try_from(8).unwrap()),
            ),
            (
                KeyCode::Left,
                Action::Move {
                    rows: 0,
                    columns: -1,
                },
            ),
            (KeyCode::Char('0'), Action::Clear),
            (KeyCode::Char('['), Action::StepBack),
            (KeyCode::Char(']'), Action::StepForward),
            (KeyCode::Home, Action::HistoryStart),
            (KeyCode::End, Action::HistoryEnd),
            (KeyCode::Tab, Action::ToggleMode),
            (KeyCode::Char('i'), Action::ToggleSidebar),
            (KeyCode::Char('t'), Action::ChooseTheme),
            (KeyCode::Char('?'), Action::Help),
            (KeyCode::Char('+'), Action::Faster),
            (KeyCode::Char('='), Action::Faster),
            (KeyCode::Char('-'), Action::Slower),
        ] {
            assert_eq!(
                map_event(Event::Key(KeyEvent::new(code, KeyModifiers::NONE))),
                Some(action)
            );
        }
    }

    #[test]
    fn release_events_do_not_duplicate_input_and_control_c_quits() {
        let released = KeyEvent::new_with_kind(
            KeyCode::Char('4'),
            KeyModifiers::NONE,
            KeyEventKind::Release,
        );
        assert_eq!(map_event(Event::Key(released)), None);
        assert_eq!(
            map_event(Event::Key(KeyEvent::new(
                KeyCode::Char('c'),
                KeyModifiers::CONTROL
            ))),
            Some(Action::Quit)
        );
        assert_eq!(
            map_event(Event::Key(KeyEvent::new(
                KeyCode::Char('r'),
                KeyModifiers::CONTROL
            ))),
            None
        );
        assert_eq!(map_event(Event::Resize(80, 24)), Some(Action::Resize));
    }

    #[test]
    fn maps_all_digits_and_preserves_key_repeats() {
        for digit in 1..=9 {
            let event = KeyEvent::new_with_kind(
                KeyCode::Char(char::from(b'0' + digit)),
                KeyModifiers::NONE,
                KeyEventKind::Repeat,
            );
            assert_eq!(
                map_event(Event::Key(event)),
                Some(Action::Digit(Digit::try_from(digit).unwrap()))
            );
        }
    }

    struct FakeClock(Rc<Cell<Duration>>);

    impl Clock for FakeClock {
        fn now(&self) -> Duration {
            self.0.get()
        }
    }

    struct FakeReader {
        clock: Rc<Cell<Duration>>,
        events: VecDeque<(Duration, io::Result<Event>)>,
        poll_error: Option<io::ErrorKind>,
        last_timeout: Duration,
    }

    impl EventReader for FakeReader {
        fn poll(&mut self, timeout: Duration) -> io::Result<bool> {
            self.last_timeout = timeout;
            if let Some(kind) = self.poll_error.take() {
                return Err(io::Error::from(kind));
            }
            let now = self.clock.get();
            let deadline = now + timeout;
            if let Some((at, _)) = self.events.front()
                && *at <= deadline
            {
                self.clock.set(now.max(*at));
                return Ok(true);
            }
            self.clock.set(deadline);
            Ok(false)
        }

        fn read(&mut self) -> io::Result<Event> {
            self.events.pop_front().expect("successful poll").1
        }
    }

    struct Fixture {
        input: ScheduledInput<FakeClock, FakeReader>,
        clock: Rc<Cell<Duration>>,
    }

    impl Fixture {
        fn new(events: impl IntoIterator<Item = (u64, Event)>) -> Self {
            let clock = Rc::new(Cell::new(Duration::ZERO));
            let reader = FakeReader {
                clock: Rc::clone(&clock),
                events: events
                    .into_iter()
                    .map(|(at, event)| (Duration::from_millis(at), Ok(event)))
                    .collect(),
                poll_error: None,
                last_timeout: Duration::ZERO,
            };
            let input =
                ScheduledInput::new(FakeClock(Rc::clone(&clock)), reader, DEFAULT_TICK_INTERVAL)
                    .unwrap();
            Self { input, clock }
        }

        fn next(&mut self) -> Action {
            self.input.next_action().unwrap().unwrap()
        }
    }

    fn key(code: KeyCode) -> Event {
        Event::Key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    #[test]
    fn sustained_keyboard_activity_does_not_starve_ticks() {
        let mut fixture = Fixture::new((1..=10).map(|n| (n * 10, key(KeyCode::Right))));
        for deadline in [50, 100] {
            for _ in 0..5 {
                assert_eq!(
                    fixture.next(),
                    Action::Move {
                        rows: 0,
                        columns: 1
                    }
                );
            }
            assert_eq!(fixture.next(), Action::Tick);
            assert_eq!(fixture.clock.get(), Duration::from_millis(deadline));
        }
    }

    #[test]
    fn ignored_events_do_not_reset_tick_deadlines() {
        let released = Event::Key(KeyEvent::new_with_kind(
            KeyCode::Char('4'),
            KeyModifiers::NONE,
            KeyEventKind::Release,
        ));
        let control = Event::Key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL));
        let alt = Event::Key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::ALT));
        let mut fixture = Fixture::new([
            (10, key(KeyCode::Char('x'))),
            (20, released),
            (30, control),
            (40, alt),
            (45, Event::FocusLost),
        ]);
        assert_eq!(fixture.next(), Action::Tick);
        assert_eq!(fixture.clock.get(), DEFAULT_TICK_INTERVAL);
        assert_eq!(fixture.input.reader.last_timeout, Duration::from_millis(5));
    }

    #[test]
    fn resize_requests_redraw_without_advancing_the_tick_deadline() {
        let mut fixture = Fixture::new([(20, Event::Resize(120, 40))]);
        assert_eq!(fixture.next(), Action::Resize);
        assert_eq!(fixture.clock.get(), Duration::from_millis(20));
        assert_eq!(fixture.next(), Action::Tick);
        assert_eq!(fixture.clock.get(), DEFAULT_TICK_INTERVAL);
        assert_eq!(fixture.input.reader.last_timeout, Duration::from_millis(30));
    }

    #[test]
    fn lag_produces_one_tick_and_rebases_the_next_deadline() {
        let mut fixture = Fixture::new([]);
        fixture.clock.set(Duration::from_millis(170));
        assert_eq!(fixture.next(), Action::Tick);
        assert_eq!(fixture.clock.get(), Duration::from_millis(170));
        assert_eq!(fixture.next(), Action::Tick);
        assert_eq!(fixture.clock.get(), Duration::from_millis(220));
        assert_eq!(fixture.input.reader.last_timeout, DEFAULT_TICK_INTERVAL);
    }

    #[test]
    fn configurable_intervals_are_measured_from_construction() {
        let fixture = Fixture::new([]);
        fixture.clock.set(Duration::from_secs(4));
        let mut input = ScheduledInput::new(
            fixture.input.clock,
            fixture.input.reader,
            Duration::from_millis(17),
        )
        .unwrap();
        assert_eq!(input.next_action().unwrap(), Some(Action::Tick));
        assert_eq!(fixture.clock.get(), Duration::from_millis(4017));
    }

    #[test]
    fn zero_tick_intervals_are_rejected() {
        let fixture = Fixture::new([]);
        let result = ScheduledInput::new(fixture.input.clock, fixture.input.reader, Duration::ZERO);
        assert_eq!(result.err().unwrap().kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn poll_errors_are_propagated_and_leave_the_deadline_intact() {
        let mut fixture = Fixture::new([]);
        fixture.input.reader.poll_error = Some(io::ErrorKind::Interrupted);
        assert_eq!(
            fixture.input.next_action().unwrap_err().kind(),
            io::ErrorKind::Interrupted
        );
        assert_eq!(fixture.next(), Action::Tick);
        assert_eq!(fixture.clock.get(), DEFAULT_TICK_INTERVAL);
    }

    #[test]
    fn read_errors_are_propagated_and_leave_the_deadline_intact() {
        let mut fixture = Fixture::new([]);
        fixture.input.reader.events.push_back((
            Duration::from_millis(20),
            Err(io::Error::from(io::ErrorKind::UnexpectedEof)),
        ));
        assert_eq!(
            fixture.input.next_action().unwrap_err().kind(),
            io::ErrorKind::UnexpectedEof
        );
        assert_eq!(fixture.next(), Action::Tick);
        assert_eq!(fixture.clock.get(), DEFAULT_TICK_INTERVAL);
    }

    #[test]
    fn functions_can_supply_finite_input_streams() {
        let mut actions = [Action::Help, Action::Quit].into_iter();
        let mut input = || Ok(actions.next());
        assert_eq!(input.next_action().unwrap(), Some(Action::Help));
        assert_eq!(input.next_action().unwrap(), Some(Action::Quit));
        assert_eq!(input.next_action().unwrap(), None);
    }
}
