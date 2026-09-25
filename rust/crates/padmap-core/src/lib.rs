//! Device-independent logic: pure functions over plain data, testable without hardware.

#![forbid(unsafe_code)]

pub mod announce;
pub mod ares;
pub mod assign;
pub mod binding;
pub mod calibration;
pub mod capability;
pub mod capture;
pub mod cemu;
pub mod command;
pub mod control;
pub mod dolphin;
pub mod dsu;
pub mod dsupad;
pub mod emit;
pub mod fields;
pub mod guess;
pub mod hide;
pub mod icons;
pub mod keyboard;
pub mod launch;
pub mod layout;
pub mod motion;
pub mod profile;
pub mod retroarch;
pub mod ryujinx;
pub mod scope;
pub mod sdl;
pub mod slots;
pub mod standard;
pub mod state;
pub mod tuning;
pub mod twins;
pub mod userconfig;
pub mod wire;
pub mod xbox;

pub use binding::{Binding, BindingKind};
pub use calibration::AxisCalibration;
pub use control::Control;
pub use fields::Fields;
pub use layout::Layout;
