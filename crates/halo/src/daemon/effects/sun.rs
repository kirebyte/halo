//! `sun`: the color follows the sun's elevation at the configured place (`latitude`/`longitude` in
//! halo.conf). Dark purple at solar midnight, cool colors while the sun is below the horizon, red
//! orange at sunrise and sunset, and bright yellow at solar noon.
//!
//! The elevation is normalized against the sun's lowest and highest points that day, so midnight
//! and noon are always the extremes, in any season and at any latitude.

use halo_core::Rgb;
use std::f64::consts::TAU;

/// (position, color) from -1 (solar midnight) through 0 (horizon) to 1 (solar noon).
const STOPS: &[(f64, Rgb)] = &[
    (-1.0, Rgb::new(45, 0, 90)),    // midnight: dark purple
    (-0.5, Rgb::new(10, 20, 170)),  // night: deep blue
    (-0.15, Rgb::new(0, 110, 230)), // before dawn / after dusk: light blue
    (-0.04, Rgb::new(120, 60, 200)), // twilight: lavender
    (0.0, Rgb::new(255, 50, 20)),   // sunrise or sunset: red orange
    (0.35, Rgb::new(255, 130, 0)),  // morning and afternoon: orange
    (1.0, Rgb::new(255, 210, 0)),   // noon: bright yellow
];

/// Seconds between recalculations: the color changes slowly.
pub const EVERY: f64 = 30.0;

pub fn color(x: f64) -> Rgb {
    let x = x.clamp(-1.0, 1.0);
    let i = STOPS.windows(2).position(|w| x <= w[1].0).unwrap_or(STOPS.len() - 2);
    let ((x0, a), (x1, b)) = (STOPS[i], STOPS[i + 1]);
    let f = (x - x0) / (x1 - x0);
    let mix = |p: u8, q: u8| (p as f64 + (q as f64 - p as f64) * f).round() as u8;
    Rgb::new(mix(a.r, b.r), mix(a.g, b.g), mix(a.b, b.b))
}

/// Sun elevation in degrees and the day's extremes (min, max), with the NOAA formulas
/// (minute-level accuracy, plenty for this).
pub fn elevation(unix: f64, lat: f64, lon: f64) -> (f64, f64, f64) {
    let days = unix / 86400.0;
    let hour_utc = days.fract() * 24.0;
    // Day of the year (0 = January 1), approximated without a calendar: the error is hours, irrelevant.
    let year_day = (days - 365.2425 * (days / 365.2425).floor()).floor();
    let g = TAU / 365.0 * (year_day + (hour_utc - 12.0) / 24.0);
    let eqtime = 229.18
        * (0.000075 + 0.001868 * g.cos() - 0.032077 * g.sin() - 0.014615 * (2.0 * g).cos() - 0.040849 * (2.0 * g).sin());
    let decl = 0.006918 - 0.399912 * g.cos() + 0.070257 * g.sin() - 0.006758 * (2.0 * g).cos() + 0.000907 * (2.0 * g).sin()
        - 0.002697 * (3.0 * g).cos()
        + 0.00148 * (3.0 * g).sin();
    let solar_minutes = hour_utc * 60.0 + eqtime + 4.0 * lon;
    let ha = (solar_minutes / 4.0 - 180.0).to_radians();
    let phi = lat.to_radians();
    let cos_zenith = phi.sin() * decl.sin() + phi.cos() * decl.cos() * ha.cos();
    let elev = 90.0 - cos_zenith.clamp(-1.0, 1.0).acos().to_degrees();
    let max = 90.0 - (phi - decl).abs().to_degrees();
    let min = (phi + decl).abs().to_degrees() - 90.0;
    (elev, min, max)
}

/// -1..1: how close the sun is to midnight (-1) or noon (1); 0 is the horizon.
pub fn position(elev: f64, min: f64, max: f64) -> f64 {
    if elev >= 0.0 {
        if max > 0.0 { elev / max } else { 0.0 }
    } else if min < 0.0 {
        -(elev / min)
    } else {
        0.0
    }
}

pub fn unix_now() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

/// No coordinates in halo.conf: the equator, at the longitude matching the time zone
/// (15° per hour). Sunrise may be off by a few minutes.
pub fn default_place() -> (f64, f64) {
    // SAFETY: localtime_r only writes to `tm`, which is ours.
    let offset = unsafe {
        let t = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&t, &mut tm);
        tm.tm_gmtoff as f64
    };
    (0.0, offset / 3600.0 * 15.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2026-06-21 00:00 UTC.
    const SOLSTICE: f64 = 1_781_913_600.0;
    const MERIDA: (f64, f64) = (20.97, -89.62);

    #[test]
    fn extremes_are_purple_and_yellow() {
        assert_eq!(color(-1.0), Rgb::new(45, 0, 90));
        assert_eq!(color(1.0), Rgb::new(255, 210, 0));
        assert_eq!(color(0.0), Rgb::new(255, 50, 20));
        assert_eq!(color(5.0), color(1.0));
        // Between stops it blends.
        assert_eq!(color(-0.75), Rgb::new(28, 10, 130));
    }

    #[test]
    fn merida_solstice() {
        // Solar noon in Merida ≈ 12:00 + 89.62° × 4 min ≈ 17:58 UTC: the sun almost at the zenith.
        let (elev, min, max) = elevation(SOLSTICE + 17.97 * 3600.0, MERIDA.0, MERIDA.1);
        assert!((max - 87.5).abs() < 0.5, "max {max}");
        assert!((elev - max).abs() < 0.5, "elev {elev}");
        assert!(position(elev, min, max) > 0.99);
        // Twelve hours later, solar midnight: the lowest point.
        let (elev, min, max) = elevation(SOLSTICE + (17.97 + 12.0) * 3600.0, MERIDA.0, MERIDA.1);
        assert!((elev - min).abs() < 0.5, "elev {elev} min {min}");
        assert!(position(elev, min, max) < -0.99);
        // Sunrise in Merida in June: ~11:15 UTC (5:15 local), sun near the horizon.
        let (elev, ..) = elevation(SOLSTICE + 11.25 * 3600.0, MERIDA.0, MERIDA.1);
        assert!(elev.abs() < 3.0, "elev {elev}");
    }

    #[test]
    fn polar_cases_do_not_divide_by_zero() {
        assert_eq!(position(-5.0, -5.0, -1.0), -1.0);
        assert_eq!(position(5.0, 2.0, 0.0), 0.0);
        assert_eq!(position(-1.0, 1.0, 10.0), 0.0);
    }
}
