//! `sensors` (Armoury Crate's "Smart"): the left ring follows the chip temperature and the right
//! one the CPU usage, from green (cool / idle) to yellow and red (hot / busy).
//! It shifts the hue, not the brightness: each LED lights half a ring and dimming it would look off.
//!
//! Reads two files from /sys and /proc every [`EVERY`] seconds; there is no separate process.

use super::anim::hue;
use halo_core::{Frame, Rgb};
use std::path::{Path, PathBuf};

pub const EVERY: f64 = 2.0;

/// Temperatures (°C) that map to green and red. The Z2 A goes from ~45 idle to ~90 while gaming.
const COOL: f64 = 45.0;
const HOT: f64 = 90.0;

/// Zones: 0 bottom left, 1 top left, 2 top right, 3 bottom right.
const LEFT: [usize; 2] = [0, 1];
const RIGHT: [usize; 2] = [2, 3];

/// 0 = green (120°), 1 = red (0°), through yellow.
pub fn gradient(x: f64) -> Rgb {
    hue(120.0 * (1.0 - x.clamp(0.0, 1.0)))
}

pub fn frame(temp: Option<f64>, usage: Option<f64>) -> Frame {
    let mut f = [Rgb::OFF; 4];
    // Without a sensor, that half stays at the cool color: better than off or a false red.
    let t = gradient(temp.map_or(0.0, |c| (c - COOL) / (HOT - COOL)));
    let u = gradient(usage.unwrap_or(0.0));
    LEFT.iter().for_each(|&z| f[z] = t);
    RIGHT.iter().for_each(|&z| f[z] = u);
    f
}

/// The chip sensor: k10temp's Tctl, or amdgpu's edge, or the ACPI thermal zone.
pub fn find_sensor() -> Option<PathBuf> {
    let hwmons: Vec<PathBuf> = std::fs::read_dir("/sys/class/hwmon").ok()?.flatten().map(|e| e.path()).collect();
    ["k10temp", "amdgpu", "acpitz"].iter().find_map(|want| {
        hwmons
            .iter()
            .find(|h| std::fs::read_to_string(h.join("name")).is_ok_and(|n| n.trim() == *want))
            .map(|h| h.join("temp1_input"))
    })
}

pub fn read_temp(path: &Path) -> Option<f64> {
    Some(std::fs::read_to_string(path).ok()?.trim().parse::<f64>().ok()? / 1000.0)
}

/// (busy, total) in jiffies, from the `cpu` line of /proc/stat.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CpuTimes {
    busy: u64,
    total: u64,
}

pub fn parse_stat(stat: &str) -> Option<CpuTimes> {
    let line = stat.lines().find(|l| l.starts_with("cpu "))?;
    let v: Vec<u64> = line.split_whitespace().skip(1).filter_map(|x| x.parse().ok()).collect();
    if v.len() < 4 {
        return None;
    }
    // user nice system idle iowait irq softirq steal (guest is already counted in user).
    let idle = v[3] + v.get(4).copied().unwrap_or(0);
    let total: u64 = v.iter().take(8).sum();
    Some(CpuTimes { busy: total - idle, total })
}

pub fn read_cpu() -> Option<CpuTimes> {
    parse_stat(&std::fs::read_to_string("/proc/stat").ok()?)
}

/// Busy fraction between two reads, 0..1.
pub fn usage(prev: CpuTimes, now: CpuTimes) -> Option<f64> {
    let total = now.total.checked_sub(prev.total)?;
    let busy = now.busy.checked_sub(prev.busy)?;
    (total > 0).then(|| busy as f64 / total as f64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn green_to_red_through_yellow() {
        assert_eq!(gradient(0.0), Rgb::new(0, 255, 0));
        assert_eq!(gradient(0.5), Rgb::new(255, 255, 0));
        assert_eq!(gradient(1.0), Rgb::new(255, 0, 0));
        assert_eq!(gradient(7.0), gradient(1.0));
    }

    #[test]
    fn rings_are_temperature_and_usage() {
        let f = frame(Some(90.0), Some(0.0));
        assert_eq!([f[0], f[1]], [Rgb::new(255, 0, 0); 2]);
        assert_eq!([f[2], f[3]], [Rgb::new(0, 255, 0); 2]);
        // Without data, cool and never off.
        assert_eq!(frame(None, None), [Rgb::new(0, 255, 0); 4]);
    }

    #[test]
    fn cpu_usage_from_proc_stat() {
        let a = parse_stat("cpu  100 0 100 800 0 0 0 0 0 0\ncpu0 1 2 3 4\n").unwrap();
        let b = parse_stat("cpu  250 0 100 900 0 0 0 0 0 0\n").unwrap();
        assert_eq!(a, CpuTimes { busy: 200, total: 1000 });
        assert_eq!(usage(a, b), Some(0.6));
        assert_eq!(usage(b, b), None);
        assert_eq!(parse_stat("intr 1 2"), None);
    }
}
