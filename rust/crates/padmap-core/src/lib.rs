//! padmap's device-independent logic.
//!
//! Everything here is a pure function over plain data: no device nodes, no
//! filesystem, no sockets, no clock. That is not tidiness, it is what makes the
//! interesting decisions testable. The Python equivalents of these modules were
//! exercised by 62 hand-run scripts under `tools/` that each needed a machine
//! with controllers plugged into it; the same decisions are reachable here from
//! `cargo test`.
//!
//! The crates above this one supply the I/O: `padmap-hid` decodes controller
//! reports, `padmap-input` owns evdev and uinput, `padmap-daemon` owns the loop.

#![forbid(unsafe_code)]

pub mod binding;
pub mod calibration;
pub mod capture;
pub mod control;
pub mod fields;
pub mod layout;
pub mod retroarch;
pub mod scope;
pub mod sdl;

pub use binding::{Binding, BindingKind};
pub use calibration::AxisCalibration;
pub use control::Control;
pub use fields::Fields;
pub use layout::Layout;
