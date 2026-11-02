//! `clock` and `clock12`: the time on the rings, one LED per digit (24 or 12 h). Left = hour,
//! right = minutes; tens on top and units at the bottom. Each digit is a color of the visible
//! spectrum, from red (~700 nm) to violet (~410 nm); 0 is off.
//!
//! It also breathes with the seconds: low brightness on every even second and high on every odd
//! one, with a smooth transition between them. The low end never goes dark, so the time stays legible.

use halo_core::{Frame, Rgb};

/// Color of each digit. Chosen to be distinguishable on the LEDs, not for physical accuracy.
pub const DIGITS: [Rgb; 10] = [
    Rgb::new(0, 0, 0),       // 0 off
    Rgb::new(255, 0, 0),     // 1 red         ~700 nm
    Rgb::new(255, 64, 0),    // 2 orange      ~610 nm
    Rgb::new(255, 170, 0),   // 3 yellow      ~580 nm
    Rgb::new(160, 255, 0),   // 4 lime        ~560 nm
    Rgb::new(0, 255, 0),     // 5 green       ~530 nm
    Rgb::new(0, 255, 160),   // 6 turquoise   ~500 nm
    Rgb::new(0, 160, 255),   // 7 sky blue    ~480 nm
    Rgb::new(0, 0, 255),     // 8 blue        ~450 nm
    Rgb::new(140, 0, 255),   // 9 violet      ~410 nm
];

/// Zones: 0 bottom left, 1 top left, 2 top right, 3 bottom right.
const HOUR_TENS: usize = 1;
const HOUR_UNITS: usize = 0;
const MIN_TENS: usize = 2;
const MIN_UNITS: usize = 3;

/// Brightness on even seconds (odd ones go to 100 %).
const LOW: f64 = 0.15;

/// The 0-23 hour in 12 h format: 0 → 12, 13 → 1.
pub fn twelve(hour: u32) -> u32 {
    (hour + 11) % 12 + 1
}

pub fn frame(hour: u32, minute: u32) -> Frame {
    let mut f = [Rgb::OFF; 4];
    f[HOUR_TENS] = DIGITS[(hour / 10 % 10) as usize];
    f[HOUR_UNITS] = DIGITS[(hour % 10) as usize];
    f[MIN_TENS] = DIGITS[(minute / 10 % 10) as usize];
    f[MIN_UNITS] = DIGITS[(minute % 10) as usize];
    f
}

pub struct Now {
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
    /// Fraction of the current second, 0..1.
    pub frac: f64,
}

/// Local time (system time zone), with a fraction of a second.
pub fn now() -> Now {
    // SAFETY: clock_gettime and localtime_r only write to `ts` and `tm`, which are ours.
    unsafe {
        let mut ts: libc::timespec = std::mem::zeroed();
        libc::clock_gettime(libc::CLOCK_REALTIME, &mut ts);
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&ts.tv_sec, &mut tm);
        Now { hour: tm.tm_hour as u32, minute: tm.tm_min as u32, second: tm.tm_sec as u32, frac: ts.tv_nsec as f64 / 1e9 }
    }
}

/// Breathing brightness: `LOW` at the start of an even second, 1 at the start of an odd one.
pub fn breath(second: u32, frac: f64) -> f64 {
    let phase = ((second % 2) as f64 + frac) / 2.0;
    let w = (1.0 - (std::f64::consts::TAU * phase).cos()) / 2.0;
    LOW + (1.0 - LOW) * w * w
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digits_to_zones() {
        // 19:05 → top left red (1), bottom left violet (9), top right off (0), bottom right green (5).
        assert_eq!(frame(19, 5), [DIGITS[9], DIGITS[1], DIGITS[0], DIGITS[5]]);
        assert_eq!(frame(0, 0), [Rgb::OFF; 4]);
        assert_eq!(frame(23, 59), [DIGITS[3], DIGITS[2], DIGITS[5], DIGITS[9]]);
        assert_eq!([0, 1, 11, 12, 13, 23].map(twelve), [12, 1, 11, 12, 1, 11]);
    }

    #[test]
    fn distinct_colors() {
        for (i, a) in DIGITS.iter().enumerate() {
            for b in &DIGITS[i + 1..] {
                assert_ne!(a, b);
            }
        }
    }

    #[test]
    fn breathes_with_the_seconds() {
        assert!((breath(10, 0.0) - LOW).abs() < 1e-9);
        assert!((breath(11, 0.0) - 1.0).abs() < 1e-9);
        assert!((breath(59, 0.999) - LOW).abs() < 1e-3); // back down just before the even one
        assert!(breath(10, 0.5) > LOW && breath(10, 0.5) < 1.0);
    }
}
