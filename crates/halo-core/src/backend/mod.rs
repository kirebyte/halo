//! Paths for sending a [`Frame`] to the MCU.

mod dummy;
mod lamparray;
mod vendor;

pub use dummy::Dummy;
pub use lamparray::LampArray;
pub use vendor::{HwMode, HwProgram, HwSpeed, HwStep, Vendor};

use crate::Frame;
use std::io;

pub trait Backend: Send {
    fn name(&self) -> &'static str;
    fn apply(&mut self, frame: &Frame) -> io::Result<()>;
    /// Hardware brightness 0-3 (`5A BA C5 C4`). At 0 the rings stay off whichever path is used.
    fn set_brightness(&mut self, level: u8) -> io::Result<()>;
    /// Hands control of the rings back to the firmware (only has an effect on LampArray).
    fn release(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// LampArray if present and openable; otherwise the ASUS command.
pub fn open_auto() -> io::Result<Box<dyn Backend>> {
    match LampArray::open() {
        Ok(b) => Ok(Box::new(b)),
        Err(la) => Vendor::open().map(|v| Box::new(v) as Box<dyn Backend>).map_err(|v| {
            io::Error::new(v.kind(), format!("LampArray: {la}; vendor: {v}"))
        }),
    }
}
