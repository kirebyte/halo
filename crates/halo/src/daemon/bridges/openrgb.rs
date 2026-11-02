//! OpenRGB SDK server (`127.0.0.1:6742`, protocol 5) with a single device: the rings. This lets
//! Artemis, the OpenRGB client and scripts like openrgb-python paint them.
//! Reference: `Documentation/OpenRGBSDK.md` in the OpenRGB repo.
//!
//! What a client sends is a priority-150 layer (above the equalizer and below the games) that is
//! removed when it disconnects. Modes, profiles and plugins do not exist here: there is only one
//! "Direct" mode.

use crate::daemon::compositor::{Layer, PRIORITY_EXTERNAL, Zones};
use crate::daemon::device::Core;
use halo_core::{Frame, Rgb, ZONES};
use std::io;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Mutex;

pub const PORT: u16 = 6742;
const PROTOCOL: u32 = 5;
const MAGIC: &[u8; 4] = b"ORGB";
const MAX_PACKET: usize = 64 * 1024;

const REQUEST_CONTROLLER_COUNT: u32 = 0;
const REQUEST_CONTROLLER_DATA: u32 = 1;
const REQUEST_PROTOCOL_VERSION: u32 = 40;
const SET_CLIENT_NAME: u32 = 50;
const REQUEST_PROFILE_LIST: u32 = 150;
const REQUEST_PLUGIN_LIST: u32 = 200;
const UPDATE_LEDS: u32 = 1050;
const UPDATE_ZONE_LEDS: u32 = 1051;
const UPDATE_SINGLE_LED: u32 = 1052;

const DEVICE_TYPE_GAMEPAD: i32 = 10;
const ZONE_TYPE_LINEAR: i32 = 1;
const MODE_FLAG_HAS_PER_LED_COLOR: u32 = 1 << 5;
const MODE_COLORS_PER_LED: u32 = 1;

/// (name, first LED, count). The LEDs follow the zone order of `docs/lamparray.md`.
const RINGS: [(&str, usize, usize); 2] = [("Left ring", 0, 2), ("Right ring", 2, 2)];
const LEDS: [&str; ZONES] = ["Left bottom", "Left top", "Right top", "Right bottom"];

/// Little-endian writer with OpenRGB strings (`u16` length that includes the `\0`).
#[derive(Default)]
struct W(Vec<u8>);

impl W {
    fn u16(&mut self, v: u16) -> &mut Self {
        self.0.extend(v.to_le_bytes());
        self
    }
    fn u32(&mut self, v: u32) -> &mut Self {
        self.0.extend(v.to_le_bytes());
        self
    }
    fn i32(&mut self, v: i32) -> &mut Self {
        self.0.extend(v.to_le_bytes());
        self
    }
    fn str(&mut self, s: &str) -> &mut Self {
        self.u16(s.len() as u16 + 1);
        self.0.extend(s.as_bytes());
        self.0.push(0);
        self
    }
    fn color(&mut self, c: Rgb) -> &mut Self {
        self.0.extend([c.r, c.g, c.b, 0]);
        self
    }
    /// Prepends `data_size`, which in OpenRGB counts its own 4 bytes.
    fn sized(self) -> Vec<u8> {
        let mut out = ((self.0.len() + 4) as u32).to_le_bytes().to_vec();
        out.extend(self.0);
        out
    }
}

/// The device description (`RGBController::GetDeviceDescription`) for protocol version `proto`.
fn device_data(proto: u32, colors: &Frame) -> Vec<u8> {
    let mut w = W::default();
    w.i32(DEVICE_TYPE_GAMEPAD).str("ROG Xbox Ally");
    if proto >= 1 {
        w.str("ASUS");
    }
    w.str("Joystick RGB rings, via Halo").str(env!("CARGO_PKG_VERSION")).str("").str("HID LampArray");
    // Modes: only "Direct".
    w.u16(1).i32(0).str("Direct");
    if proto < 6 {
        w.i32(0); // mode_value
    }
    w.u32(MODE_FLAG_HAS_PER_LED_COLOR).u32(0).u32(0);
    if proto >= 3 {
        w.u32(0).u32(0);
    }
    w.u32(0).u32(0).u32(0);
    if proto >= 3 {
        w.u32(0);
    }
    w.u32(0).u32(MODE_COLORS_PER_LED).u16(0);
    w.u16(RINGS.len() as u16);
    for (name, _, n) in RINGS {
        w.str(name).i32(ZONE_TYPE_LINEAR).u32(n as u32).u32(n as u32).u32(n as u32).u16(0);
        if proto >= 4 {
            w.u16(0); // segments
        }
        if proto >= 5 {
            w.u32(0); // zone_flags
        }
    }
    w.u16(ZONES as u16);
    for (i, name) in LEDS.iter().enumerate() {
        w.str(name).u32(i as u32);
    }
    w.u16(ZONES as u16);
    for c in colors {
        w.color(*c);
    }
    if proto >= 5 {
        w.u16(0).u32(0); // alternate LED names, flags
    }
    w.sized()
}

fn header(dev: u32, id: u32, payload: &[u8]) -> Vec<u8> {
    let mut out = MAGIC.to_vec();
    for v in [dev, id, payload.len() as u32] {
        out.extend(v.to_le_bytes());
    }
    out.extend(payload);
    out
}

fn u32_at(d: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(d.get(at..at + 4)?.try_into().ok()?))
}

fn u16_at(d: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(d.get(at..at + 2)?.try_into().ok()?))
}

fn color_at(d: &[u8], at: usize) -> Option<Rgb> {
    let c = d.get(at..at + 4)?;
    Some(Rgb::new(c[0], c[1], c[2]))
}

/// `u16 num_colors` and the colors, starting at `at`.
fn colors_at(d: &[u8], at: usize) -> Option<Vec<Rgb>> {
    let n = u16_at(d, at)? as usize;
    (0..n).map(|i| color_at(d, at + 2 + 4 * i)).collect()
}

#[derive(Default)]
struct Session {
    zones: Zones,
}

impl Session {
    /// Applies an `RGBCONTROLLER_UPDATE*` packet; `true` if what is painted changed.
    fn update(&mut self, id: u32, d: &[u8]) -> bool {
        let before = self.zones;
        match id {
            UPDATE_LEDS => {
                for (z, c) in colors_at(d, 4).unwrap_or_default().into_iter().take(ZONES).enumerate() {
                    self.zones[z] = Some(c);
                }
            }
            UPDATE_ZONE_LEDS => {
                let Some(&(_, start, n)) = u32_at(d, 4).and_then(|z| RINGS.get(z as usize)) else { return false };
                for (i, c) in colors_at(d, 8).unwrap_or_default().into_iter().take(n).enumerate() {
                    self.zones[start + i] = Some(c);
                }
            }
            UPDATE_SINGLE_LED => {
                if let (Some(led), Some(c)) = (u32_at(d, 0), color_at(d, 4))
                    && (led as usize) < ZONES
                {
                    self.zones[led as usize] = Some(c);
                }
            }
            _ => {}
        }
        self.zones != before
    }
}

pub async fn serve(core: Arc<Mutex<Core>>, port: u16) -> io::Result<()> {
    let listener = TcpListener::bind(("127.0.0.1", port)).await?;
    eprintln!("halo: OpenRGB server listening on 127.0.0.1:{port}");
    loop {
        let (stream, peer) = listener.accept().await?;
        let core = core.clone();
        tokio::spawn(async move {
            let mut layer = format!("openrgb#{}", peer.port());
            if let Err(e) = session(&core, stream, &mut layer).await
                && e.kind() != io::ErrorKind::UnexpectedEof
            {
                eprintln!("halo: {layer}: {e}");
            }
            let mut core = core.lock().await;
            if core.comp.remove_layer(&layer) {
                let _ = core.push();
            }
        });
    }
}

async fn session(core: &Mutex<Core>, mut stream: TcpStream, layer: &mut String) -> io::Result<()> {
    let mut state = Session::default();
    let mut head = [0u8; 16];
    let mut data = Vec::new();
    loop {
        stream.read_exact(&mut head).await?;
        if &head[..4] != MAGIC {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "not the OpenRGB protocol"));
        }
        let (dev, id, size) = (u32_at(&head, 4).unwrap(), u32_at(&head, 8).unwrap(), u32_at(&head, 12).unwrap() as usize);
        if size > MAX_PACKET {
            return Err(io::Error::new(io::ErrorKind::InvalidData, format!("packet of {size} bytes")));
        }
        data.resize(size, 0);
        stream.read_exact(&mut data).await?;
        let reply = match id {
            REQUEST_CONTROLLER_COUNT => Some(1u32.to_le_bytes().to_vec()),
            REQUEST_CONTROLLER_DATA if dev == 0 => {
                let proto = u32_at(&data, 0).unwrap_or(0).min(PROTOCOL);
                Some(device_data(proto, &core.lock().await.comp.output()))
            }
            REQUEST_PROTOCOL_VERSION => Some(PROTOCOL.to_le_bytes().to_vec()),
            REQUEST_PROFILE_LIST | REQUEST_PLUGIN_LIST => {
                let mut w = W::default();
                w.u16(0);
                Some(w.sized())
            }
            SET_CLIENT_NAME => {
                let name: String =
                    String::from_utf8_lossy(&data).trim_end_matches('\0').chars().filter(|c| !c.is_control()).take(64).collect();
                let renamed = format!("openrgb:{name}#{}", layer.rsplit('#').next().unwrap_or("?"));
                let mut core = core.lock().await;
                if let Some(zones) = core.comp.remove_layer(layer).then_some(state.zones) {
                    core.comp.set_layer(&renamed, Layer { priority: PRIORITY_EXTERNAL, zones });
                }
                eprintln!("halo: {renamed} connected");
                *layer = renamed;
                None
            }
            UPDATE_LEDS | UPDATE_ZONE_LEDS | UPDATE_SINGLE_LED if dev == 0 => {
                if state.update(id, &data) {
                    let mut core = core.lock().await;
                    core.comp.set_layer(layer, Layer { priority: PRIORITY_EXTERNAL, zones: state.zones });
                    core.push()?;
                }
                None
            }
            // Modes, save, zone resizing, profiles: not applicable to 4 fixed LEDs.
            _ => None,
        };
        if let Some(payload) = reply {
            stream.write_all(&header(dev, id, &payload)).await?;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FRAME: Frame = [Rgb::new(1, 2, 3), Rgb::new(4, 5, 6), Rgb::new(7, 8, 9), Rgb::new(10, 11, 12)];

    /// Minimal reader to check that the description can be walked end to end.
    struct R<'a>(&'a [u8], usize);
    impl R<'_> {
        fn take(&mut self, n: usize) -> &[u8] {
            let s = &self.0[self.1..self.1 + n];
            self.1 += n;
            s
        }
        fn u16(&mut self) -> u16 {
            u16::from_le_bytes(self.take(2).try_into().unwrap())
        }
        fn u32(&mut self) -> u32 {
            u32::from_le_bytes(self.take(4).try_into().unwrap())
        }
        fn str(&mut self) -> String {
            let n = self.u16() as usize;
            let s = self.take(n);
            assert_eq!(s[n - 1], 0);
            String::from_utf8(s[..n - 1].to_vec()).unwrap()
        }
    }

    fn walk(proto: u32) -> (String, Vec<String>, Vec<u32>) {
        let d = device_data(proto, &FRAME);
        let mut r = R(&d, 0);
        assert_eq!(r.u32() as usize, d.len());
        assert_eq!(r.u32(), DEVICE_TYPE_GAMEPAD as u32);
        let name = r.str();
        if proto >= 1 {
            assert_eq!(r.str(), "ASUS");
        }
        for _ in 0..4 {
            r.str();
        }
        assert_eq!(r.u16(), 1);
        assert_eq!(r.u32(), 0);
        assert_eq!(r.str(), "Direct");
        // value, flags, speed min/max, colors min/max, speed, direction, color_mode (+ 3 for brightness)
        let ints = 9 + if proto >= 3 { 3 } else { 0 };
        let mode: Vec<u32> = (0..ints).map(|_| r.u32()).collect();
        assert_eq!(r.u16(), 0);
        let mut zones = Vec::new();
        for _ in 0..r.u16() {
            zones.push(r.str());
            r.take(16);
            assert_eq!(r.u16(), 0);
            if proto >= 4 {
                assert_eq!(r.u16(), 0);
            }
            if proto >= 5 {
                r.u32();
            }
        }
        for _ in 0..r.u16() {
            r.str();
            r.u32();
        }
        assert_eq!(r.u16(), 4);
        assert_eq!(r.take(16), [1, 2, 3, 0, 4, 5, 6, 0, 7, 8, 9, 0, 10, 11, 12, 0]);
        if proto >= 5 {
            r.take(6);
        }
        assert_eq!(r.1, d.len(), "leftover bytes");
        (name, zones, mode)
    }

    #[test]
    fn device_data_every_protocol() {
        for proto in 0..=PROTOCOL {
            let (name, zones, mode) = walk(proto);
            assert_eq!(name, "ROG Xbox Ally");
            assert_eq!(zones, ["Left ring", "Right ring"]);
            assert_eq!(mode[1], MODE_FLAG_HAS_PER_LED_COLOR);
            assert_eq!(*mode.last().unwrap(), MODE_COLORS_PER_LED);
        }
    }

    fn colors(cs: &[Rgb]) -> Vec<u8> {
        let mut w = W::default();
        w.u16(cs.len() as u16);
        for c in cs {
            w.color(*c);
        }
        w.0
    }

    #[test]
    fn updates_paint_zones() {
        let (red, blue) = (Rgb::new(255, 0, 0), Rgb::new(0, 0, 255));
        let mut s = Session::default();
        let mut zone = 1u32.to_le_bytes().to_vec();
        zone.extend(colors(&[blue, blue]));
        let mut pkt = ((zone.len() + 4) as u32).to_le_bytes().to_vec();
        pkt.extend(zone);
        assert!(s.update(UPDATE_ZONE_LEDS, &pkt));
        assert_eq!(s.zones, [None, None, Some(blue), Some(blue)]);

        let mut single = 0u32.to_le_bytes().to_vec();
        single.extend([255, 0, 0, 0]);
        assert!(s.update(UPDATE_SINGLE_LED, &single));
        assert!(!s.update(UPDATE_SINGLE_LED, &single), "same color: no repaint");
        assert_eq!(s.zones[0], Some(red));

        let mut all = 0u32.to_le_bytes().to_vec(); // data_size (unused)
        all.extend(colors(&[red; 4]));
        assert!(s.update(UPDATE_LEDS, &all));
        assert_eq!(s.zones, [Some(red); 4]);
        assert!(!s.update(UPDATE_ZONE_LEDS, &9u32.to_le_bytes()), "zone that does not exist");
        assert!(!s.update(UPDATE_LEDS, &[1, 2]), "truncated packet");
    }

    #[test]
    fn header_layout() {
        assert_eq!(header(0, 40, &5u32.to_le_bytes()), b"ORGB\0\0\0\0\x28\0\0\0\x04\0\0\0\x05\0\0\0");
    }
}
