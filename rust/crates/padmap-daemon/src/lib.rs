//! The padmap daemon.
//!
//! One process owning everything with a lifetime longer than a single
//! command: the assignment session, the virtual pads, and the generated
//! RetroArch launch config. Front-ends connect over a unix socket and drive
//! it; the wire format is `padmap_core::wire`.
//!
//! Why a daemon rather than a per-launch command: the virtual pads must exist
//! continuously. If they came and went per game, RetroArch would see a
//! different set of devices each launch and pad indices would move underneath
//! it -- which is the exact failure this project exists to fix.
//!
//! Single-threaded, one epoll loop over four kinds of descriptor: the
//! listening socket and each client, the physical pads while a session is
//! assigning, the pads and their clones while republishing, and a periodic
//! tick. The tick drives hold timers, which nothing else would notice: a held
//! button emits no further events.
//!
//! The one invariant every part of this crate serves: **the daemon must not
//! be killable by the things it exists to serve.** When it dies its uinput
//! nodes go with it, so the machine does not degrade -- it simply has no
//! controllers, mid-game, with nothing on screen to say why. No message from
//! a client, no device going away, no file on disk may end the process.

pub mod calibration;
pub mod confirm;
pub mod events;
pub mod hotplug;
pub mod publish;
pub mod seating;
pub mod server;
pub mod session;

/// How often the tick runs.
pub const TICK: std::time::Duration = std::time::Duration::from_millis(20);

/// Strip control characters some adapters prefix to their device name.
pub fn clean(name: &str) -> String {
    padmap_core::sdl::clean_name(name)
}

/// Seconds since an arbitrary point, monotonic. Every timer here takes the
/// clock as a value, so a test can hand in whatever time it likes.
pub fn now() -> f64 {
    static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    START
        .get_or_init(std::time::Instant::now)
        .elapsed()
        .as_secs_f64()
}
