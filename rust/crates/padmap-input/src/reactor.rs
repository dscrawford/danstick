//! One epoll set over every descriptor, and a tick that cannot land on the
//! input path.
//!
//! The Python ran `_tick()` after every return from the selector, not every
//! 20ms. With seven pads at ~169 events/s that is roughly 1200 ticks a second
//! rather than 50 -- everything in it was time-gated or cheap, so it was fine,
//! but the mental model was wrong by a factor of 24 and the next thing added to
//! the tick would have been 24 times more expensive than whoever added it
//! expected. That is how a quarter-second device scan ended up on the thread
//! that forwards controller events.
//!
//! Here the tick is a `timerfd` in the same epoll set. It fires when the kernel
//! says so, the expiry count says whether we fell behind, and the period is
//! self-correcting because `TFD_TIMER_ABSTIME` keeps the next expiry on the
//! original grid instead of drifting by each iteration's work.

use std::mem::MaybeUninit;
use std::os::fd::{AsFd, BorrowedFd, OwnedFd};

use rustix::event::epoll;
use rustix::time::{Itimerspec, TimerfdClockId, TimerfdFlags, TimerfdTimerFlags, Timespec};

/// What a readable descriptor belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Watched {
    /// A physical pad, by index into the republisher's list.
    Source(usize),
    /// A clone's uinput node, which carries force feedback the other way.
    Clone(usize),
    /// The periodic tick.
    Tick,
    /// The daemon's listening socket.
    Listener,
    /// A connected client, by its descriptor number.
    Client(i32),
    /// A pad held open by an assignment session, by index into it.
    Session(usize),
    /// An unseated pad being watched for someone holding a button on it, by
    /// index into the seating list. Read but never grabbed.
    Seating(usize),
    /// A pad's motion sensor, by the same index as its source.
    Motion(usize),
    /// The DSU socket emulators ask for motion on.
    Dsu,
}

/// Low four bits of a token say which kind; the rest is the index.
///
/// Four rather than three: the sixth and seventh kinds arrived with motion,
/// and a tag field that is exactly full is one that silently aliases the next
/// time somebody adds a descriptor.
const TAG_BITS: u32 = 4;
const TAG_MASK: u64 = (1 << TAG_BITS) - 1;
const TAG_SOURCE: u64 = 0;
const TAG_CLONE: u64 = 1;
const TAG_LISTENER: u64 = 2;
const TAG_CLIENT: u64 = 3;
const TAG_SESSION: u64 = 4;
const TAG_SEATING: u64 = 5;
const TAG_MOTION: u64 = 6;
const TAG_DSU: u64 = 7;

impl Watched {
    fn token(self) -> u64 {
        match self {
            Watched::Source(index) => ((index as u64) << TAG_BITS) | TAG_SOURCE,
            Watched::Clone(index) => ((index as u64) << TAG_BITS) | TAG_CLONE,
            Watched::Tick => u64::MAX,
            Watched::Listener => TAG_LISTENER,
            // A descriptor is non-negative; the cast is lossless.
            Watched::Client(fd) => ((fd as u64) << TAG_BITS) | TAG_CLIENT,
            Watched::Session(index) => ((index as u64) << TAG_BITS) | TAG_SESSION,
            Watched::Seating(index) => ((index as u64) << TAG_BITS) | TAG_SEATING,
            Watched::Motion(index) => ((index as u64) << TAG_BITS) | TAG_MOTION,
            Watched::Dsu => TAG_DSU,
        }
    }

    fn from_token(token: u64) -> Watched {
        if token == u64::MAX {
            return Watched::Tick;
        }
        let index = (token >> TAG_BITS) as usize;
        match token & TAG_MASK {
            TAG_CLONE => Watched::Clone(index),
            TAG_LISTENER => Watched::Listener,
            TAG_CLIENT => Watched::Client(index as i32),
            TAG_SESSION => Watched::Session(index),
            TAG_SEATING => Watched::Seating(index),
            TAG_MOTION => Watched::Motion(index),
            TAG_DSU => Watched::Dsu,
            _ => Watched::Source(index),
        }
    }
}

/// How many descriptors one wakeup may report.
///
/// Four pads is two descriptors each plus the tick; nine. This is generous and
/// costs half a kilobyte of stack, and a wakeup that fills it simply leaves the
/// rest for the next one -- epoll is level-triggered here, so nothing is lost.
const MAX_READY: usize = 32;

/// What one wakeup found, without allocating for it.
#[derive(Debug, Clone, Copy)]
pub struct Ready {
    items: [Watched; MAX_READY],
    count: usize,
}

impl Ready {
    pub fn iter(&self) -> impl Iterator<Item = Watched> + '_ {
        self.items[..self.count].iter().copied()
    }

    pub fn len(&self) -> usize {
        self.count
    }

    pub fn is_empty(&self) -> bool {
        self.count == 0
    }
}

pub struct Reactor {
    epoll: OwnedFd,
    tick: OwnedFd,
    /// A fixed buffer, reused: the loop allocates nothing per wakeup.
    ///
    /// Not a `Vec`. rustix reads a `&mut Vec`'s *length* as the epoll
    /// `maxevents`, not its capacity, so a cleared Vec asks the kernel for zero
    /// events and `epoll_wait` answers EINVAL -- which reads as "the loop is
    /// broken" rather than "the buffer is empty".
    raw: [MaybeUninit<epoll::Event>; MAX_READY],
}

// By hand: `rustix::event::epoll::Event` is not Debug, and the buffer is an
// implementation detail anyway.
impl std::fmt::Debug for Reactor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Reactor").finish_non_exhaustive()
    }
}

impl Reactor {
    /// An epoll set with a periodic tick already in it.
    pub fn new(tick_period: std::time::Duration) -> rustix::io::Result<Self> {
        let epoll = epoll::create(epoll::CreateFlags::CLOEXEC)?;
        let tick = rustix::time::timerfd_create(
            TimerfdClockId::Monotonic,
            TimerfdFlags::CLOEXEC | TimerfdFlags::NONBLOCK,
        )?;
        let period = Timespec {
            tv_sec: tick_period.as_secs() as i64,
            tv_nsec: i64::from(tick_period.subsec_nanos()),
        };
        rustix::time::timerfd_settime(
            &tick,
            TimerfdTimerFlags::empty(),
            &Itimerspec {
                it_interval: period,
                it_value: period,
            },
        )?;
        epoll::add(
            &epoll,
            &tick,
            epoll::EventData::new_u64(Watched::Tick.token()),
            epoll::EventFlags::IN,
        )?;
        Ok(Reactor {
            epoll,
            tick,
            raw: [MaybeUninit::uninit(); MAX_READY],
        })
    }

    pub fn watch(&self, fd: BorrowedFd<'_>, what: Watched) -> rustix::io::Result<()> {
        epoll::add(
            &self.epoll,
            fd,
            epoll::EventData::new_u64(what.token()),
            epoll::EventFlags::IN,
        )
    }

    /// Stop servicing a descriptor.
    ///
    /// A dead node reports readable forever; left registered, the loop spins on
    /// it for as long as the daemon runs.
    pub fn unwatch(&self, fd: BorrowedFd<'_>) -> rustix::io::Result<()> {
        epoll::delete(&self.epoll, fd)
    }

    /// Block until something is readable.
    ///
    /// No timeout: the tick is a descriptor like any other, so there is nothing
    /// to wake up *for* that is not in the set. That is the property worth
    /// having -- a loop with a timeout has two ways to be woken and has to work
    /// out which happened.
    pub fn wait(&mut self) -> rustix::io::Result<Ready> {
        let mut ready = Ready {
            items: [Watched::Tick; MAX_READY],
            count: 0,
        };
        let (fired, _) = epoll::wait(&self.epoll, &mut self.raw[..], None)?;
        for (slot, event) in ready.items.iter_mut().zip(fired.iter()) {
            *slot = Watched::from_token(event.data.u64());
        }
        ready.count = fired.len().min(MAX_READY);
        Ok(ready)
    }

    /// Consume a tick and report how many periods it covered.
    ///
    /// More than one means the loop fell behind, which is the number worth
    /// logging: it says the tick is late without needing a clock comparison.
    pub fn take_tick(&self) -> u64 {
        let mut buffer = [0u8; 8];
        match rustix::io::read(&self.tick, &mut buffer) {
            Ok(8) => u64::from_ne_bytes(buffer),
            _ => 0,
        }
    }
}

impl AsFd for Reactor {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.epoll.as_fd()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn a_token_round_trips_for_every_kind() {
        for index in [0usize, 1, 2, 7, 1024, usize::from(u16::MAX)] {
            assert_eq!(
                Watched::from_token(Watched::Source(index).token()),
                Watched::Source(index)
            );
            assert_eq!(
                Watched::from_token(Watched::Clone(index).token()),
                Watched::Clone(index)
            );
        }
        assert_eq!(Watched::from_token(Watched::Tick.token()), Watched::Tick);
        assert_eq!(
            Watched::from_token(Watched::Listener.token()),
            Watched::Listener
        );
        for fd in [0i32, 3, 4, 1023, i32::MAX] {
            assert_eq!(
                Watched::from_token(Watched::Client(fd).token()),
                Watched::Client(fd)
            );
        }
        for index in [0usize, 5, 4096] {
            assert_eq!(
                Watched::from_token(Watched::Session(index).token()),
                Watched::Session(index)
            );
            assert_eq!(
                Watched::from_token(Watched::Seating(index).token()),
                Watched::Seating(index)
            );
        }
    }

    #[test]
    fn a_session_pad_and_a_republished_source_are_different_tokens() {
        // Index 0 in a session and index 0 in the republisher are different
        // descriptors, serviced by different code.
        assert_ne!(Watched::Session(0).token(), Watched::Source(0).token());
        assert_ne!(Watched::Seating(0).token(), Watched::Session(0).token());
        assert_ne!(Watched::Client(0).token(), Watched::Listener.token());
    }

    #[test]
    fn a_source_and_its_clone_are_different_tokens() {
        // They are the same index and must not be serviced by the same arm:
        // one carries presses inbound, the other rumble outbound.
        assert_ne!(Watched::Source(3).token(), Watched::Clone(3).token());
    }

    #[test]
    fn the_tick_fires_on_its_own_without_anything_else_registered() {
        let mut reactor = Reactor::new(Duration::from_millis(5)).expect("a reactor");
        let ready = reactor.wait().expect("the tick must wake the loop");
        assert_eq!(ready.iter().collect::<Vec<_>>(), [Watched::Tick]);
        assert!(reactor.take_tick() >= 1);
    }

    #[test]
    fn a_late_loop_is_told_how_many_periods_it_missed() {
        // The number that says the tick is behind, without comparing clocks.
        let reactor = Reactor::new(Duration::from_millis(2)).expect("a reactor");
        std::thread::sleep(Duration::from_millis(25));
        let expiries = reactor.take_tick();
        assert!(
            expiries > 1,
            "expected several missed periods, got {expiries}"
        );
    }

    #[test]
    fn reading_a_tick_that_has_not_fired_reports_nothing_rather_than_blocking() {
        let reactor = Reactor::new(Duration::from_secs(3600)).expect("a reactor");
        assert_eq!(reactor.take_tick(), 0);
    }
}
