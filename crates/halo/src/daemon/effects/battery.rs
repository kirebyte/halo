//! `battery`: the rings show the charge. Red at 10 % or less, yellow at half, green when full.
//! While charging, a white flash walks the zones in the comet's order. It shifts the hue and never
//! dims: each LED lights half a ring.
//!
//! Reads `/sys/class/power_supply/BAT*` directly (the same thing UPower uses), no D-Bus.

use super::anim::{hue, mix};
use halo_core::{Frame, Rgb, ZONES};
use std::path::PathBuf;

/// Seconds between reads while not charging, and how long the flash stays on each zone while charging.
pub const EVERY: f64 = 10.0;
pub const STEP: f64 = 0.75;

const EMPTY: f64 = 10.0;
/// How close to white the flashing zone gets.
const FLASH: f64 = 0.6;
const WHITE: Rgb = Rgb::new(255, 255, 255);

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct State {
    pub percent: f64,
    pub charging: bool,
}

/// Red (0°) at `EMPTY` % or less, green (120°) at 100 %.
pub fn color(percent: f64) -> Rgb {
    hue(120.0 * ((percent - EMPTY) / (100.0 - EMPTY)).clamp(0.0, 1.0))
}

/// `order` is the walk order; `t` in seconds.
pub fn frame(state: Option<State>, t: f64, order: [usize; ZONES]) -> Frame {
    let Some(s) = state else {
        return [color(100.0); ZONES]; // no battery (plugged into the wall?): green
    };
    let base = color(s.percent);
    let mut f = [base; ZONES];
    if s.charging {
        let zone = order[(t / STEP) as usize % ZONES];
        f[zone] = mix(base, WHITE, FLASH);
    }
    f
}

fn find() -> Option<PathBuf> {
    std::fs::read_dir("/sys/class/power_supply").ok()?.flatten().map(|e| e.path()).find(|p| {
        std::fs::read_to_string(p.join("type")).is_ok_and(|t| t.trim() == "Battery")
    })
}

/// "Full" with the charger plugged in does not count as charging: nothing left to show.
pub fn read() -> Option<State> {
    let dir = find()?;
    let percent = std::fs::read_to_string(dir.join("capacity")).ok()?.trim().parse().ok()?;
    let status = std::fs::read_to_string(dir.join("status")).unwrap_or_default();
    Some(State { percent, charging: status.trim() == "Charging" })
}

#[cfg(test)]
mod tests {
    use super::*;

    const ORDER: [usize; ZONES] = [1, 0, 2, 3];

    #[test]
    fn charge_to_color() {
        assert_eq!(color(5.0), Rgb::new(255, 0, 0));
        assert_eq!(color(55.0), Rgb::new(255, 255, 0));
        assert_eq!(color(100.0), Rgb::new(0, 255, 0));
    }

    #[test]
    fn discharging_is_steady() {
        let s = Some(State { percent: 100.0, charging: false });
        assert_eq!(frame(s, 0.0, ORDER), frame(s, 3.3, ORDER));
        assert_eq!(frame(s, 0.0, ORDER), [Rgb::new(0, 255, 0); ZONES]);
    }

    #[test]
    fn charging_flash_walks_the_rings() {
        let s = Some(State { percent: 100.0, charging: true });
        let lit = |t| frame(s, t, ORDER).iter().position(|&c| c != Rgb::new(0, 255, 0));
        assert_eq!([0.0, 0.8, 1.6, 2.3, 3.0].map(lit), [Some(1), Some(0), Some(2), Some(3), Some(1)]);
        // The flash brightens, never darkens.
        assert_eq!(frame(s, 0.0, ORDER)[1], Rgb::new(153, 255, 153));
    }
}
