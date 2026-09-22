//! The padmap daemon.

pub mod calibration;
pub mod confirm;
pub mod dsu;
pub mod events;
pub mod hotplug;
pub mod publish;
pub mod seating;
pub mod server;
pub mod session;

pub const TICK: std::time::Duration = std::time::Duration::from_millis(20);

pub fn clean(name: &str) -> String {
    padmap_core::sdl::clean_name(name)
}

/// How long a hold must run to claim a seat, for a daemon nobody tells.
pub const ENV_HOLD: &str = "PADMAP_HOLD_SECONDS";

/// The hold length this daemon starts with: the environment's, or the default.
pub fn configured_hold() -> f64 {
    padmap_core::assign::hold_from(std::env::var(ENV_HOLD).ok().as_deref())
}

pub fn now() -> f64 {
    static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    START
        .get_or_init(std::time::Instant::now)
        .elapsed()
        .as_secs_f64()
}
