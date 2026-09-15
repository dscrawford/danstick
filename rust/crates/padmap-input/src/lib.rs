//! Everything that touches a device node.
//!
//! `padmap-core` decides things; this crate does them. The split is what makes
//! the decisions testable without a controller plugged in, and it is also where
//! the `unsafe` boundary would sit if any were needed -- none is, because the
//! ioctls come from the `evdev` and `rustix` crates rather than from here.

pub mod artefacts;
pub mod assignments;
pub mod clone;
pub mod lizard;
pub mod pad;
pub mod profiles;
pub mod reactor;
pub mod republish;
pub mod runtime;
pub mod triton;

pub use clone::{IdentityMode, VirtualPad};
pub use pad::{discover, Filter, Pad};
pub use reactor::{Reactor, Watched};
pub use republish::Republisher;
