//! Halo core: colors, frames and backends for the RGB rings of the ROG Xbox Ally.
//!
//! The controller's MCU (USB `0B05:1B4C`) is driven through two paths:
//! - [`backend::LampArray`]: standard HID LampArray on interface 00, no root needed (see `docs/lamparray.md`).
//! - [`backend::Vendor`]: the ASUS command `5A D1 08 0C` on interface 02 (needs the udev rule).

pub mod backend;
pub mod color;
pub mod discover;
pub mod hid;
pub mod i18n;
pub mod state;

pub use color::Rgb;

/// Zones of the Xbox Ally: 0-1 left ring, 2-3 right ring.
pub const ZONES: usize = 4;

/// One color per zone.
pub type Frame = [Rgb; ZONES];
