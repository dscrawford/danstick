//! Motion sample conversion: SDL frame (device-native) to DSU frame (emulator-native).

/// One reading in DSU's frame and units (g and degrees per second).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Motion {
    /// Acceleration in g.
    pub accel: [f32; 3],
    /// Gyro in degrees per second.
    pub gyro: [f32; 3],
    /// Monotonic microseconds; repeated stamp makes Cemu discard sample.
    pub timestamp_us: u64,
}

impl Motion {
    /// Convert SDL frame to DSU frame.
    pub fn from_sdl_frame(accel: [f32; 3], gyro: [f32; 3], timestamp_us: u64) -> Motion {
        Motion {
            accel: [-accel[0], -accel[1], -accel[2]],
            gyro: [gyro[0], -gyro[1], -gyro[2]],
            timestamp_us,
        }
    }

    /// All-zero reading means not yet read (gravity always present otherwise).
    pub fn is_silent(&self) -> bool {
        self.accel == [0.0; 3] && self.gyro == [0.0; 3]
    }
}

/// Kernel resolution: counts per g and per degree per second.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Scale {
    pub accel: f32,
    pub gyro: f32,
}

impl Scale {
    /// From kernel resolutions; zero resolution falls back to 1 (prevents divide-by-zero NaN).
    pub fn from_resolutions(accel: i32, gyro: i32) -> Scale {
        Scale {
            accel: if accel == 0 { 1.0 } else { accel as f32 },
            gyro: if gyro == 0 { 1.0 } else { gyro as f32 },
        }
    }

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
        let sample = Motion::from_sdl_frame([0.0, 1.0, 0.0], [0.0; 3], 0);
        assert_eq!(sample.accel, [-0.0, -1.0, -0.0]);
    }

    #[test]
    fn the_accelerometer_mirrors_and_the_gyroscope_does_not() {
        let sample = Motion::from_sdl_frame([1.0, 2.0, 3.0], [4.0, 5.0, 6.0], 99);
        assert_eq!(sample.accel, [-1.0, -2.0, -3.0]);
        assert_eq!(sample.gyro, [4.0, -5.0, -6.0]);
        assert_eq!(sample.timestamp_us, 99);
    }

    #[test]
    fn pitching_the_nose_up_is_positive_in_both_frames() {
        let sample = Motion::from_sdl_frame([0.0; 3], [10.0, 0.0, 0.0], 0);
        assert!(sample.gyro[0] > 0.0);
    }

    #[test]
    fn a_resolution_divides_counts_into_units() {
        let scale = Scale::from_resolutions(8192, 1024);
        let sample = scale.sample([4096, 0, 0], [0, 0, 2048], 0);
        assert_eq!(sample.accel[0], -0.5);
        assert_eq!(sample.gyro[2], -2.0);
    }

    #[test]
    fn a_driver_that_declares_no_resolution_does_not_divide_by_zero() {
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
