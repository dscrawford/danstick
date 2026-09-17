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

pub fn now() -> f64 {
    static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    START
        .get_or_init(std::time::Instant::now)
        .elapsed()
        .as_secs_f64()
}
