//! A kernel IMU node, read as motion samples.
//!
//! A controller with a gyroscope publishes it as a device of its own beside
//! the joypad -- "Nintendo Switch Pro Controller IMU", "Sony Interactive
//! Entertainment DualSense Motion Sensors". [`crate::pad::motion_sibling`]
//! finds it; this opens it.
//!
//! The kernel states its own units. `absinfo.resolution` is counts per g on
//! `ABS_X..ABS_Z` and counts per degree per second on `ABS_RX..ABS_RZ`, and
//! SDL divides by exactly that with no per-device table
//! (`SDL_sysjoystick.c`). So there is nothing here to keep up to date as new
//! controllers appear, which is the whole reason to read the node rather than
//! decode each protocol.
//!
//! Never grabbed. A gyro node carries no buttons, so nothing leaks through by
//! leaving it readable -- and grabbing it would take motion away from anything
//! else on the machine that wanted it.

use std::io;
use std::os::fd::{AsFd, BorrowedFd};
use std::path::{Path, PathBuf};

use evdev::{AbsoluteAxisCode, Device, EventType};
use log::debug;
use padmap_core::motion::{Motion, Scale};

/// Kernel codes, in the order the packet wants them.
const ACCEL_AXES: [AbsoluteAxisCode; 3] = [
    AbsoluteAxisCode::ABS_X,
    AbsoluteAxisCode::ABS_Y,
    AbsoluteAxisCode::ABS_Z,
];
const GYRO_AXES: [AbsoluteAxisCode; 3] = [
    AbsoluteAxisCode::ABS_RX,
    AbsoluteAxisCode::ABS_RY,
    AbsoluteAxisCode::ABS_RZ,
];

/// One controller's motion sensor.
#[derive(Debug)]
pub struct Sensor {
    device: Device,
    path: PathBuf,
    scale: Scale,
    /// Raw counts, in the kernel's frame, as last reported.
    accel: [i32; 3],
    gyro: [i32; 3],
    /// Microseconds, from the event timestamps the kernel stamps each frame
    /// with. Taken from the device rather than from a clock read here, because
    /// a consumer integrates the gap between samples and the gap that matters
    /// is when the sensor was read, not when the daemon got round to it.
    timestamp_us: u64,
    /// Whether anything has arrived. Until it has there is nothing to publish,
    /// and publishing zeroes would tell a consumer the pad is in freefall.
    seen: bool,
}

impl Sensor {
    /// Open a motion node, non-blocking and ungrabbed.
    pub fn open(path: &Path) -> io::Result<Sensor> {
        let device = Device::open(path)?;
        // Non-blocking for the same reason every source is: the reactor
        // guarantees only that the *first* read will not block, and a second
        // one on a quiet device wedges the thread that forwards every player's
        // input.
        device.set_nonblocking(true)?;
        let scale = scale_of(&device);
        debug!(
            "{}: motion sensor, {} counts/g and {} counts/deg/s",
            path.display(),
            scale.accel,
            scale.gyro
        );
        Ok(Sensor {
            device,
            path: path.to_path_buf(),
            scale,
            accel: [0; 3],
            gyro: [0; 3],
            timestamp_us: 0,
            seen: false,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn as_fd(&self) -> BorrowedFd<'_> {
        self.device.as_fd()
    }

    /// The latest sample, or `None` until the sensor has said something.
    pub fn motion(&self) -> Option<Motion> {
        if !self.seen {
            return None;
        }
        Some(self.scale.sample(self.accel, self.gyro, self.timestamp_us))
    }

    /// Take everything waiting. `Ok(false)` when the device is gone.
    ///
    /// A `WouldBlock` is success with nothing in it: the reactor is
    /// level-triggered and a spurious wake-up is not a fault.
    pub fn read(&mut self) -> io::Result<bool> {
        let events = match self.device.fetch_events() {
            Ok(events) => events,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(true),
            Err(error) if error.raw_os_error() == Some(rustix::io::Errno::NODEV.raw_os_error()) => {
                return Ok(false)
            }
            Err(error) => return Err(error),
        };
        for event in events {
            if event.event_type() != EventType::ABSOLUTE {
                continue;
            }
            let code = AbsoluteAxisCode(event.code());
            if let Some(index) = ACCEL_AXES.iter().position(|axis| *axis == code) {
                self.accel[index] = event.value();
                self.seen = true;
            } else if let Some(index) = GYRO_AXES.iter().position(|axis| *axis == code) {
                self.gyro[index] = event.value();
                self.seen = true;
            } else {
                continue;
            }
            self.timestamp_us = stamp_us(&event);
        }
        Ok(true)
    }
}

/// An event's own timestamp, in microseconds.
fn stamp_us(event: &evdev::InputEvent) -> u64 {
    let stamp = event.timestamp();
    match stamp.duration_since(std::time::UNIX_EPOCH) {
        Ok(since) => since.as_micros() as u64,
        // A clock before the epoch is not something to fail over; the sample
        // is still good and only its label is wrong.
        Err(_) => 0,
    }
}

/// The counts-per-unit the driver declared.
fn scale_of(device: &Device) -> Scale {
    let mut accel = 0;
    let mut gyro = 0;
    if let Ok(axes) = device.get_absinfo() {
        for (code, info) in axes {
            if code == AbsoluteAxisCode::ABS_X {
                accel = info.resolution();
            } else if code == AbsoluteAxisCode::ABS_RX {
                gyro = info.resolution();
            }
        }
    }
    Scale::from_resolutions(accel, gyro)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_node_is_an_error_not_a_panic() {
        assert!(Sensor::open(Path::new("/nonexistent-padmap-imu/event0")).is_err());
    }

    #[test]
    fn the_axis_order_is_the_one_the_packet_wants() {
        // ABS_X..ABS_Z is the accelerometer and ABS_RX..ABS_RZ the gyroscope,
        // which is the kernel's convention for an INPUT_PROP_ACCELEROMETER
        // device and the order SDL reads them in. Swapping the two families
        // sends rotation where a consumer expects gravity, and the result is
        // a world that tumbles rather than one that is merely tilted.
        assert_eq!(ACCEL_AXES[0], AbsoluteAxisCode::ABS_X);
        assert_eq!(ACCEL_AXES[2], AbsoluteAxisCode::ABS_Z);
        assert_eq!(GYRO_AXES[0], AbsoluteAxisCode::ABS_RX);
        assert_eq!(GYRO_AXES[2], AbsoluteAxisCode::ABS_RZ);
    }
}
