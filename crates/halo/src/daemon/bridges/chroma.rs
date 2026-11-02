//! Razer Chroma: what the fake DLL (`shims/chroma`) sends from Wine/Proton.
//!
//! Each connection is one game and one layer. When the socket closes (the game exited or crashed)
//! the layer goes away. If the game paints several devices at once, only one is used, in this order:
//! ChromaLink (meant for ambient lighting), keyboard, headset, mousepad, mouse, keypad, generic.

use super::mapping::{self, average_lit};
use crate::daemon::compositor::{Layer, PRIORITY_GAME};
use crate::daemon::device::Core;
use halo_core::{Frame, Rgb, ZONES};
use std::collections::BTreeMap;
use std::io;
use std::sync::Arc;
use tokio::io::AsyncReadExt;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Mutex;

pub const PORT: u16 = 54236;
const MAX_MESSAGE: usize = 64 * 1024;
const OP_HELLO: u8 = 1;
const OP_APPLY: u8 = 2;

// Razer's DEVICE_INFO_TYPE::DeviceType (and 255 for the generic CreateEffect).
pub(super) const KEYBOARD: u8 = 1;
pub(super) const MOUSE: u8 = 2;
pub(super) const HEADSET: u8 = 3;
pub(super) const MOUSEPAD: u8 = 4;
pub(super) const KEYPAD: u8 = 5;
pub(super) const CHROMALINK: u8 = 8;
const GENERIC: u8 = 255;
/// Which device the colors are taken from when the game paints several.
const PREFERENCE: [u8; 7] = [CHROMALINK, KEYBOARD, HEADSET, MOUSEPAD, MOUSE, KEYPAD, GENERIC];

#[derive(Debug, PartialEq, Eq)]
pub enum Decoded {
    /// `CHROMA_NONE`: the game turned that device off.
    Off,
    Frame(Frame),
    /// An effect we cannot paint (breathing, wave...) or data of an odd length: ignored.
    Unsupported,
}

/// COLORREF = 0x00BBGGRR. The high byte is a flag (e.g. in `Key` of CUSTOM_KEY).
fn colorref(c: u32) -> Rgb {
    Rgb::new(c as u8, (c >> 8) as u8, (c >> 16) as u8)
}

fn words(data: &[u8]) -> Vec<u32> {
    data.as_chunks::<4>().0.iter().map(|w| u32::from_le_bytes(*w)).collect()
}

/// A grid of COLORREF to 4 zones.
fn grid(rows: usize, cols: usize, cells: &[u32]) -> Frame {
    let colors: Vec<Rgb> = cells.iter().map(|&c| colorref(c)).collect();
    mapping::grid(rows, cols, &colors)
}

/// A list of LEDs that wraps around (mousepad): first half to the left ring, second to the right.
fn halves(cells: &[u32]) -> Frame {
    let (l, r) = cells.split_at(cells.len() / 2);
    let left = average_lit(l.iter().map(|&c| colorref(c)));
    let right = average_lit(r.iter().map(|&c| colorref(c)));
    [left, left, right, right]
}

fn solid(c: Rgb) -> Frame {
    [c; ZONES]
}

pub fn decode(device: u8, kind: u32, data: &[u8]) -> Decoded {
    use Decoded::*;
    if kind == 0 {
        return Off;
    }
    let w = words(data);
    let all = |c: &[u32]| solid(average_lit(c.iter().map(|&x| colorref(x))));
    let frame = match (device, kind, w.len()) {
        (CHROMALINK, 2, 1) | (KEYBOARD, 4, 1) | (HEADSET, 1, 1) | (MOUSEPAD, 4, 1) | (KEYPAD, 5, 1) | (GENERIC, 4, 1) => {
            solid(colorref(w[0]))
        }
        (CHROMALINK, 1, 5) => {
            // Index 0 is the "background"; 1 to 4 go to the zones. If the game only uses 0, it goes everywhere.
            let zones = [1, 2, 3, 4].map(|i| colorref(w[i]));
            if zones.iter().all(|c| *c == Rgb::OFF) { solid(colorref(w[0])) } else { zones }
        }
        (KEYBOARD, 2, 132) => grid(6, 22, &w),
        (KEYBOARD, 8, 264) => {
            // CUSTOM_KEY: Key overrides Color wherever it carries the 0x01000000 flag.
            let (color, key) = w.split_at(132);
            let merged: Vec<u32> = color.iter().zip(key).map(|(&c, &k)| if k & 0x0100_0000 != 0 { k } else { c }).collect();
            grid(6, 22, &merged)
        }
        (KEYBOARD, 9, 324) => grid(8, 24, &w[..192]),
        (MOUSE, 6, 2) => solid(colorref(w[1])), // { RZLED LEDId; COLORREF Color; }
        (MOUSE, 3, 30) | (HEADSET, 4, 5) => all(&w),
        (MOUSE, 8, 63) => grid(9, 7, &w),
        (MOUSEPAD, 2, 15) | (MOUSEPAD, 6, 20) => halves(&w),
        (KEYPAD, 2, 20) => grid(4, 5, &w),
        _ => return Unsupported,
    };
    Frame(frame)
}

/// What a game has painted, per device.
#[derive(Default)]
pub(super) struct Session {
    devices: BTreeMap<u8, Frame>,
}

impl Session {
    /// `true` if what must be shown changed.
    pub(super) fn apply(&mut self, device: u8, kind: u32, data: &[u8]) -> bool {
        match decode(device, kind, data) {
            Decoded::Off => self.devices.remove(&device).is_some(),
            Decoded::Frame(f) => self.devices.insert(device, f) != Some(f),
            Decoded::Unsupported => false,
        }
    }

    pub(super) fn output(&self) -> Option<Frame> {
        PREFERENCE.iter().find_map(|d| self.devices.get(d).copied())
    }
}

pub async fn serve(core: Arc<Mutex<Core>>, port: u16) -> io::Result<()> {
    let listener = TcpListener::bind(("127.0.0.1", port)).await?;
    eprintln!("halo: Chroma bridge listening on 127.0.0.1:{port}");
    loop {
        let (stream, peer) = listener.accept().await?;
        let core = core.clone();
        tokio::spawn(async move {
            let mut layer = format!("chroma#{}", peer.port());
            if let Err(e) = session(&core, stream, &mut layer).await
                && e.kind() != io::ErrorKind::UnexpectedEof
            {
                eprintln!("halo: {layer}: {e}");
            }
            let mut core = core.lock().await;
            if core.comp.remove_layer(&layer) {
                let _ = core.push();
            }
            eprintln!("halo: {layer} ended");
        });
    }
}

async fn session(core: &Mutex<Core>, mut stream: TcpStream, layer: &mut String) -> io::Result<()> {
    let mut state = Session::default();
    let mut buf = Vec::new();
    loop {
        let len = stream.read_u32_le().await? as usize;
        if len == 0 || len > MAX_MESSAGE {
            return Err(io::Error::new(io::ErrorKind::InvalidData, format!("message of {len} bytes")));
        }
        buf.resize(len, 0);
        stream.read_exact(&mut buf).await?;
        match (buf[0], &buf[1..]) {
            (OP_HELLO, name) => {
                let name: String = String::from_utf8_lossy(name).chars().filter(|c| !c.is_control()).take(64).collect();
                let renamed = format!("chroma:{name}#{}", layer.rsplit('#').next().unwrap_or("?"));
                core.lock().await.comp.remove_layer(layer);
                eprintln!("halo: {renamed} connected");
                *layer = renamed;
            }
            (OP_APPLY, [device, k0, k1, k2, k3, data @ ..]) => {
                if !state.apply(*device, u32::from_le_bytes([*k0, *k1, *k2, *k3]), data) {
                    continue;
                }
                let mut core = core.lock().await;
                match state.output() {
                    Some(frame) => core.comp.set_layer(layer, Layer { priority: PRIORITY_GAME, zones: frame.map(Some) }),
                    None => {
                        core.comp.remove_layer(layer);
                    }
                }
                core.push()?;
            }
            (op, _) => eprintln!("halo: {layer}: unknown message {op}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RED: u32 = 0x0000ff;
    const GREEN: u32 = 0x00ff00;
    const BLUE: u32 = 0xff0000;

    fn bytes(w: &[u32]) -> Vec<u8> {
        w.iter().flat_map(|x| x.to_le_bytes()).collect()
    }

    fn rgb(c: u32) -> Rgb {
        colorref(c)
    }

    #[test]
    fn colorref_is_bgr() {
        assert_eq!(colorref(0x00_60_ff), Rgb::new(0xff, 0x60, 0));
        assert_eq!(colorref(0x0100_00ff), Rgb::new(0xff, 0, 0));
    }

    #[test]
    fn chromalink_custom_and_background() {
        let d = decode(CHROMALINK, 1, &bytes(&[0xffffff, RED, GREEN, BLUE, 0x00c0ff]));
        assert_eq!(d, Decoded::Frame([rgb(RED), rgb(GREEN), rgb(BLUE), Rgb::new(0xff, 0xc0, 0)]));
        let d = decode(CHROMALINK, 1, &bytes(&[GREEN, 0, 0, 0, 0]));
        assert_eq!(d, Decoded::Frame([rgb(GREEN); ZONES]));
    }

    #[test]
    fn keyboard_regions() {
        let mut g = vec![0u32; 132];
        for r in 0..6 {
            g[r * 22 + 2] = RED; // left
        }
        g[22 + 18] = BLUE; // right, top
        let Decoded::Frame(f) = decode(KEYBOARD, 2, &bytes(&g)) else { panic!() };
        assert_eq!(f, [rgb(RED), rgb(RED), rgb(BLUE), Rgb::OFF]);
    }

    #[test]
    fn custom_key_overrides_color() {
        let mut w = vec![RED; 132];
        w.extend(vec![0u32; 132]);
        for c in 11..22 {
            for r in 0..6 {
                w[132 + r * 22 + c] = 0x0100_0000 | GREEN;
            }
        }
        let Decoded::Frame(f) = decode(KEYBOARD, 8, &bytes(&w)) else { panic!() };
        assert_eq!(f, [rgb(RED), rgb(RED), rgb(GREEN), rgb(GREEN)]);
    }

    #[test]
    fn statics_none_and_unsupported() {
        assert_eq!(decode(KEYBOARD, 4, &bytes(&[BLUE])), Decoded::Frame([rgb(BLUE); ZONES]));
        assert_eq!(decode(MOUSE, 6, &bytes(&[0xffff, RED])), Decoded::Frame([rgb(RED); ZONES]));
        assert_eq!(decode(MOUSEPAD, 0, &[]), Decoded::Off);
        assert_eq!(decode(KEYBOARD, 1, &[]), Decoded::Unsupported); // breathing
        assert_eq!(decode(KEYBOARD, 2, &bytes(&[RED; 10])), Decoded::Unsupported); // odd length
    }

    #[test]
    fn session_prefers_chromalink_and_falls_back() {
        let mut s = Session::default();
        assert!(s.apply(KEYBOARD, 4, &bytes(&[RED])));
        assert!(!s.apply(KEYBOARD, 4, &bytes(&[RED])), "same color: no change");
        assert!(s.apply(CHROMALINK, 2, &bytes(&[GREEN])));
        assert_eq!(s.output(), Some([rgb(GREEN); ZONES]));
        assert!(s.apply(CHROMALINK, 0, &[]));
        assert_eq!(s.output(), Some([rgb(RED); ZONES]));
        assert!(s.apply(KEYBOARD, 0, &[]));
        assert_eq!(s.output(), None);
    }
}
