//! One motion sample, and the frame it is measured in.
//!
//! padmap reads motion from three kinds of device -- a kernel IMU node beside
//! a pad, a Steam Controller over hidraw, a Switch Pro -- and sends it to
//! emulators that agree on one frame. This module is the single place the
//! conversion happens, so a controller added later cannot invent a fourth
//! convention.
//!
//! **The frame everything converts *from* is SDL's**, because SDL documents
//! it and because padmap's decoders are ports of SDL's: X to the right, Y up,
//! Z out of the front of the pad towards whoever is holding it, and a positive
//! rotation is counter-clockwise looked at from the positive axis. The Linux
//! kernel's IMU nodes are in it too -- SDL passes `ABS_X..ABS_RZ` straight
//! through with no remap (`SDL_sysjoystick.c`).
//!
//! **The frame it converts *to* is DSU's**, which is neither the same nor a
//! rotation of it. Taken from the pair of files in Dolphin that name both:
//! `SDLGamepad.h` maps SDL's axes to `Accel Up/Left/Forward`, and
//! `DualShockUDPClient.cpp` maps DSU's fields to the same six names. Reading
//! one against the other gives the conversion with no guesswork:
//!
//! | | SDL | DSU |
//! |---|---|---|
//! | accel X | right | left |
//! | accel Y | up | down |
//! | accel Z | towards the player | away from the player |
//! | gyro 0 | pitch up | pitch up |
//! | gyro 1 | yaw left | yaw right |
//! | gyro 2 | roll left | roll right |
//!
//! So the accelerometer negates all three and the gyroscope negates two. That
//! asymmetry is the thing worth knowing: the frames are mirror images, and
//! angular velocity is a pseudo-vector, so it does not follow the mirror.
//!
//! Units are **g** for the accelerometer and **degrees per second** for the
//! gyroscope. Cemu confirms the latter by multiplying by 0.0174533 on receipt
//! (`DSUControllerProvider.cpp`), which is degrees to radians.

/// One reading, in DSU's frame and units.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Motion {
    /// `x`, `y`, `z` in g.
    pub accel: [f32; 3],
    /// `pitch`, `yaw`, `roll` in degrees per second.
    pub gyro: [f32; 3],
    /// Microseconds, monotonic, from any origin the sender likes. Consumers
    /// integrate the *difference* between two samples, so the origin does not
    /// matter and a stall does: a repeated timestamp makes Cemu discard the
    /// sample (`integrate_motion`).
    pub timestamp_us: u64,
}

impl Motion {
    /// A sample given in SDL's frame, converted.
    ///
    /// `accel` in g and `gyro` in degrees per second -- the scaling is the
    /// caller's, because only the caller knows what the device's counts mean.
    pub fn from_sdl_frame(accel: [f32; 3], gyro: [f32; 3], timestamp_us: u64) -> Motion {
        Motion {
            accel: [-accel[0], -accel[1], -accel[2]],
            gyro: [gyro[0], -gyro[1], -gyro[2]],
            timestamp_us,
        }
    }

    /// Whether this sample says anything. An all-zero reading is what a device
    /// that has not been read yet looks like, and also what a genuinely
    /// weightless one would -- which cannot happen, because gravity is always
    /// one of the three.
    pub fn is_silent(&self) -> bool {
        self.accel == [0.0; 3] && self.gyro == [0.0; 3]
    }
}

/// How a device's raw counts become g and degrees per second.
///
/// A kernel IMU node states this itself: `absinfo.resolution` is counts per g
/// on `ABS_X..ABS_Z` and counts per degree per second on `ABS_RX..ABS_RZ`.
/// SDL divides by exactly that (`SDL_sysjoystick.c`), which is what makes this
/// a reading of the kernel's contract rather than a table of device quirks.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Scale {
    /// Counts per g.
    pub accel: f32,
    /// Counts per degree per second.
    pub gyro: f32,
}

impl Scale {
    /// The kernel's, from the two resolutions.
    ///
    /// A resolution of zero means the driver did not say. Falling back to 1
    /// rather than dividing by zero: the reading is then wrong by a scale
    /// factor, where the alternative is an infinity that reaches an emulator
    /// and spins its orientation off to NaN, from which it never recovers
    /// without a restart.
    pub fn from_resolutions(accel: i32, gyro: i32) -> Scale {
        Scale {
            accel: if accel == 0 { 1.0 } else { accel as f32 },
            gyro: if gyro == 0 { 1.0 } else { gyro as f32 },
        }
    }

    /// Raw counts in the kernel's axis order to a sample in DSU's frame.
    pub fn sample(&self, accel: [i32; 3], gyro: [i32; 3], timestamp_us: u64) -> Motion {
        let a = accel.map(|v| v as f32 / self.accel);
        let g = gyro.map(|v| v as f32 / self.gyro);
        Motion::from_sdl_frame(a, g, timestamp_us)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gravity_on_a_pad_lying_flat_points_down_in_dsu() {
        // Face up on a table, an accelerometer measures the table pushing back:
        // +1g along SDL's +Y, which is up. DSU's Y is down, so it reads -1.
        // Getting this backwards puts the world upside down in every game that
        // uses tilt, which is the most visible way this can be wrong.
        let sample = Motion::from_sdl_frame([0.0, 1.0, 0.0], [0.0; 3], 0);
        assert_eq!(sample.accel, [-0.0, -1.0, -0.0]);
    }

    #[test]
    fn the_accelerometer_mirrors_and_the_gyroscope_does_not() {
        // The asymmetry this module exists for. Negating all three axes is a
        // reflection, and angular velocity is a pseudo-vector, so pitch keeps
        // its sign where the two axes that flip with it do not.
        let sample = Motion::from_sdl_frame([1.0, 2.0, 3.0], [4.0, 5.0, 6.0], 99);
        assert_eq!(sample.accel, [-1.0, -2.0, -3.0]);
        assert_eq!(sample.gyro, [4.0, -5.0, -6.0]);
        assert_eq!(sample.timestamp_us, 99);
    }

    #[test]
    fn pitching_the_nose_up_is_positive_in_both_frames() {
        // Dolphin names SDL axis 0 "Pitch Up" and DSU's `pitch` "Gyro Pitch
        // Up". The one axis that agrees.
        let sample = Motion::from_sdl_frame([0.0; 3], [10.0, 0.0, 0.0], 0);
        assert!(sample.gyro[0] > 0.0);
    }

    #[test]
    fn a_resolution_divides_counts_into_units() {
        // A DualShock 4's IMU node states 8192 counts per g and 1024 per
        // degree per second. Half a g is 4096 counts.
        let scale = Scale::from_resolutions(8192, 1024);
        let sample = scale.sample([4096, 0, 0], [0, 0, 2048], 0);
        assert_eq!(sample.accel[0], -0.5);
        assert_eq!(sample.gyro[2], -2.0);
    }

    #[test]
    fn a_driver_that_declares_no_resolution_does_not_divide_by_zero() {
        // An infinity here reaches the emulator and turns its orientation into
        // NaN, which no amount of holding the pad still recovers from.
        let scale = Scale::from_resolutions(0, 0);
        let sample = scale.sample([1, 2, 3], [4, 5, 6], 0);
        assert!(sample.accel.iter().all(|v| v.is_finite()));
        assert!(sample.gyro.iter().all(|v| v.is_finite()));
    }

    #[test]
    fn a_sample_nobody_has_written_says_so() {
        assert!(Motion::default().is_silent());
        assert!(!Motion::from_sdl_frame([0.0, 1.0, 0.0], [0.0; 3], 0).is_silent());
    }
}
