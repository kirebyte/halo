//! The ASUS LED command on interface 02 (from the `hid-asus-ally` driver).
//! InputPlumber hides this hidraw: the udev rule `packaging/udev/99-halo.rules` is required.
//! The MCU ignores it while LampArray has `AutonomousMode=0`.
//!
//! The same interface understands report `0x5D` from the Aura protocol of ROG keyboards (the
//! one asusctl uses): with it the MCU animates on its own, with nobody sending frames. Verified
//! on hardware: it accepts static, breathe (1 or 2 colors), cycle, rainbow wave (with direction)
//! and pulse; per zone (1-4) or all (0); and it remembers the program while LampArray paints
//! over it.

use super::Backend;
use crate::discover::{self, Interface};
use crate::{Frame, Rgb, hid};
use std::fs::{File, OpenOptions};
use std::io;

pub const REPORT_SIZE: usize = 64;
/// report id, code page, `xpad_cmd_set_leds`, `xpad_cmd_len_leds` (12 data bytes)
const SET_LEDS: [u8; 4] = [0x5a, 0xd1, 0x08, 0x0c];
const BRIGHTNESS: [u8; 4] = [0x5a, 0xba, 0xc5, 0xc4];

pub fn set_leds_report(frame: &Frame) -> [u8; REPORT_SIZE] {
    let mut r = [0; REPORT_SIZE];
    r[..4].copy_from_slice(&SET_LEDS);
    for (i, c) in frame.iter().enumerate() {
        r[4 + i * 3..7 + i * 3].copy_from_slice(&[c.r, c.g, c.b]);
    }
    r
}

/// MCU modes that actually animate (the rest of the protocol is ignored).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum HwMode {
    Static = 0,
    Breathe = 1,
    Cycle = 2,
    Wave = 3,
    Pulse = 10,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum HwSpeed {
    Slow = 0xe1,
    Medium = 0xeb,
    Fast = 0xf5,
}

/// One `5D B3`: what animates a zone (1-4, or 0 = all).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HwStep {
    pub zone: u8,
    pub mode: HwMode,
    pub color: Rgb,
    /// Second color (breathe alternates between the two); off = none.
    pub color2: Rgb,
    pub speed: HwSpeed,
    pub reverse: bool,
}

/// What stays programmed in the MCU: several `5D B3` and one `5D B4`.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct HwProgram {
    pub steps: Vec<HwStep>,
}

pub fn mode_report(s: &HwStep) -> [u8; REPORT_SIZE] {
    let mut r = [0; REPORT_SIZE];
    r[..13].copy_from_slice(&[
        0x5d, 0xb3, s.zone, s.mode as u8, s.color.r, s.color.g, s.color.b, s.speed as u8, s.reverse as u8, 0, s.color2.r, s.color2.g,
        s.color2.b,
    ]);
    r
}

pub fn apply_report() -> [u8; REPORT_SIZE] {
    let mut r = [0; REPORT_SIZE];
    r[..2].copy_from_slice(&[0x5d, 0xb4]);
    r
}

pub fn brightness_report(level: u8) -> [u8; REPORT_SIZE] {
    let mut r = [0; REPORT_SIZE];
    r[..4].copy_from_slice(&BRIGHTNESS);
    r[4] = level.min(3);
    r
}

pub struct Vendor {
    dev: File,
}

impl Vendor {
    pub fn open() -> io::Result<Self> {
        let path = discover::find(Interface::Vendor)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "the Ally's interface 02 (report ID 0x5A) is not present"))?;
        Ok(Self { dev: OpenOptions::new().read(true).write(true).open(path)? })
    }

    /// Programs the animation into the MCU. Visible while LampArray is in autonomous mode.
    pub fn program(&mut self, p: &HwProgram) -> io::Result<()> {
        for s in &p.steps {
            hid::set_feature(&self.dev, &mode_report(s))?;
        }
        hid::set_feature(&self.dev, &apply_report())
    }
}

impl Backend for Vendor {
    fn name(&self) -> &'static str {
        "vendor"
    }

    fn apply(&mut self, frame: &Frame) -> io::Result<()> {
        hid::set_feature(&self.dev, &set_leds_report(frame))
    }

    fn set_brightness(&mut self, level: u8) -> io::Result<()> {
        hid::set_feature(&self.dev, &brightness_report(level))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Rgb;

    #[test]
    fn set_leds_bytes() {
        let f = [Rgb::new(0xff, 0, 0), Rgb::new(0, 0xff, 0), Rgb::new(0, 0, 0xff), Rgb::new(0xff, 0xc0, 0)];
        let r = set_leds_report(&f);
        assert_eq!(&r[..16], &[0x5a, 0xd1, 0x08, 0x0c, 0xff, 0, 0, 0, 0xff, 0, 0, 0, 0xff, 0xff, 0xc0, 0]);
        assert!(r[16..].iter().all(|&b| b == 0));
    }

    #[test]
    fn mode_bytes() {
        // The bytes the prototype script sent; verified on hardware breathing between red and blue.
        let s = HwStep { zone: 0, mode: HwMode::Breathe, color: Rgb::new(0xff, 0, 0), color2: Rgb::new(0, 0, 0xff), speed: HwSpeed::Fast, reverse: false };
        assert_eq!(&mode_report(&s)[..13], &[0x5d, 0xb3, 0, 1, 0xff, 0, 0, 0xf5, 0, 0, 0, 0, 0xff]);
        let w = HwStep { zone: 2, mode: HwMode::Wave, color: Rgb::OFF, color2: Rgb::OFF, speed: HwSpeed::Medium, reverse: true };
        assert_eq!(&mode_report(&w)[..9], &[0x5d, 0xb3, 2, 3, 0, 0, 0, 0xeb, 1]);
        assert_eq!(&apply_report()[..3], &[0x5d, 0xb4, 0]);
    }

    #[test]
    fn brightness_bytes() {
        assert_eq!(&brightness_report(3)[..5], &[0x5a, 0xba, 0xc5, 0xc4, 3]);
        assert_eq!(brightness_report(9)[4], 3);
    }
}
