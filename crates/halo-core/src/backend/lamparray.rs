//! HID LampArray (usage page 0x59) on interface 00. Format in `docs/lamparray.md`.
//!
//! With `AutonomousMode=0` the MCU only obeys this path: neither sysfs (InputPlumber) nor the
//! ASUS command override it. Hardware brightness still belongs to the vendor channel, so it is
//! sent that way when accessible.

use super::{Backend, Vendor};
use crate::discover::{self, Interface};
use crate::{Frame, ZONES, hid};
use std::fs::{File, OpenOptions};
use std::io;

const ATTRIBUTES: u8 = 1;
const MULTI_UPDATE: u8 = 4;
const CONTROL: u8 = 6;
/// Lamps per LampMultiUpdate according to the descriptor.
const MULTI_SLOTS: usize = 8;
pub const MULTI_UPDATE_SIZE: usize = 3 + MULTI_SLOTS * 2 + MULTI_SLOTS * 4;
const UPDATE_COMPLETE: u8 = 0x01;

pub fn control_report(autonomous: bool) -> [u8; 2] {
    [CONTROL, autonomous as u8]
}

pub fn multi_update_report(frame: &Frame) -> [u8; MULTI_UPDATE_SIZE] {
    let mut r = [0; MULTI_UPDATE_SIZE];
    r[0] = MULTI_UPDATE;
    r[1] = ZONES as u8;
    r[2] = UPDATE_COMPLETE;
    for (i, c) in frame.iter().enumerate() {
        r[3 + i * 2..5 + i * 2].copy_from_slice(&(i as u16).to_le_bytes());
        let o = 3 + MULTI_SLOTS * 2 + i * 4;
        r[o..o + 4].copy_from_slice(&[c.r, c.g, c.b, 0xff]);
    }
    r
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Attributes {
    pub lamp_count: u16,
    pub kind: u32,
    pub min_update_us: u32,
}

pub struct LampArray {
    dev: File,
    /// `AutonomousMode=0` has already been sent through this fd.
    claimed: bool,
}

impl LampArray {
    pub fn open() -> io::Result<Self> {
        let path = discover::find(Interface::LampArray)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "the Ally's LampArray interface (0x59) is not present"))?;
        Ok(Self { dev: OpenOptions::new().read(true).write(true).open(path)?, claimed: false })
    }

    pub fn attributes(&self) -> io::Result<Attributes> {
        let mut b = [0u8; 23];
        b[0] = ATTRIBUTES;
        hid::get_feature(&self.dev, &mut b)?;
        let u32_at = |o: usize| u32::from_le_bytes(b[o..o + 4].try_into().unwrap());
        Ok(Attributes {
            lamp_count: u16::from_le_bytes([b[1], b[2]]),
            kind: u32_at(15),
            min_update_us: u32_at(19),
        })
    }
}

impl Backend for LampArray {
    fn name(&self) -> &'static str {
        "lamparray"
    }

    fn apply(&mut self, frame: &Frame) -> io::Result<()> {
        if !self.claimed {
            hid::set_feature(&self.dev, &control_report(false))?;
            self.claimed = true;
        }
        hid::set_feature(&self.dev, &multi_update_report(frame))
    }

    fn set_brightness(&mut self, level: u8) -> io::Result<()> {
        Vendor::open()?.set_brightness(level)
    }

    /// Hands control back to the firmware (and to the vendor and sysfs paths).
    fn release(&mut self) -> io::Result<()> {
        self.claimed = false;
        hid::set_feature(&self.dev, &control_report(true))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Rgb;

    #[test]
    fn multi_update_bytes() {
        let f = [Rgb::new(0xff, 0, 0), Rgb::new(0, 0xff, 0), Rgb::new(0, 0, 0xff), Rgb::new(0xff, 0xc0, 0)];
        let r = multi_update_report(&f);
        // The same bytes the prototype script sent, verified on hardware.
        let expected = "04 04 01 00 00 01 00 02 00 03 00 00 00 00 00 00 00 00 00 ff 00 00 ff 00 ff 00 ff 00 00 ff ff ff c0 00 ff 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00";
        let got: Vec<String> = r.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(got.join(" "), expected);
    }

    #[test]
    fn control_bytes() {
        assert_eq!(control_report(false), [6, 0]);
        assert_eq!(control_report(true), [6, 1]);
    }
}
