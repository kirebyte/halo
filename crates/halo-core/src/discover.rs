//! Find the controller's hidraw nodes. The number changes across boots, so they are matched by descriptor.

use std::fs;
use std::path::{Path, PathBuf};

pub const VID_PID: &str = "0B05:1B4C";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Interface {
    /// Interface 00: HID LampArray (usage page 0x59).
    LampArray,
    /// Interface 02: ASUS commands (report ID 0x5A).
    Vendor,
}

impl Interface {
    fn matches(self, descriptor: &[u8]) -> bool {
        match self {
            // Usage Page 0x59 encoded with 1 or 2 data bytes.
            Interface::LampArray => contains(descriptor, &[0x05, 0x59]) || contains(descriptor, &[0x06, 0x59, 0x00]),
            Interface::Vendor => contains(descriptor, &[0x85, 0x5a]),
        }
    }
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

/// The Ally's `/dev/hidrawN` for that interface, if connected.
pub fn find(iface: Interface) -> Option<PathBuf> {
    find_in(Path::new("/sys/class/hidraw"), iface)
}

fn find_in(class: &Path, iface: Interface) -> Option<PathBuf> {
    let mut entries: Vec<_> = fs::read_dir(class).ok()?.flatten().map(|e| e.path()).collect();
    entries.sort();
    entries.into_iter().find_map(|p| {
        let dev = fs::canonicalize(p.join("device")).ok()?;
        if !dev.to_string_lossy().contains(VID_PID) {
            return None;
        }
        let desc = fs::read(p.join("device/report_descriptor")).ok()?;
        let name = p.file_name()?;
        iface.matches(&desc).then(|| Path::new("/dev").join(name))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descriptor_matching() {
        assert!(Interface::LampArray.matches(&[0x06, 0x59, 0x00, 0x09, 0x01]));
        assert!(!Interface::LampArray.matches(&[0x05, 0x01, 0x09, 0x05]));
        assert!(Interface::Vendor.matches(&[0x06, 0x31, 0xff, 0x85, 0x5a]));
    }
}
