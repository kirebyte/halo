//! Logitech LED Illumination SDK: what the fake LogitechLed.dll (`shims/logitech`) sends from
//! Wine/Proton, over TCP to `127.0.0.1:54238`.
//!
//! Each connection is one game and one priority-200 layer, removed when the socket closes. Unlike
//! Chroma, the SDK has timed effects (`FlashLighting`, `PulseLighting`) that the daemon draws. The
//! session only wakes while one is active: the pulse at ~30 fps and the flash exactly at each on/off
//! transition.

use super::mapping::{self, average_lit};
use crate::daemon::compositor::{Layer, PRIORITY_GAME};
use crate::daemon::device::Core;
use halo_core::{Frame, Rgb, ZONES};
use std::collections::BTreeMap;
use std::io;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::AsyncReadExt;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Mutex, mpsc};

pub const PORT: u16 = 54238;
const MAX_MESSAGE: usize = 64 * 1024;
const OP_HELLO: u8 = 1;
const OP_CALL: u8 = 2;

// Functions the DLL sends (`shims/logitech/logitech.c`).
const F_SET_LIGHTING: u8 = 1;
const F_BITMAP: u8 = 2;
const F_KEY: u8 = 3;
const F_FLASH: u8 = 4;
const F_PULSE: u8 = 5;
const F_STOP_EFFECTS: u8 = 6;
const F_SAVE: u8 = 7;
const F_RESTORE: u8 = 8;
const F_TARGET_ZONE: u8 = 9;

/// The SDK's bitmap: 21x6 keys, BGRA.
const BITMAP_COLS: usize = 21;
const BITMAP_ROWS: usize = 6;
const PULSE_FRAME: Duration = Duration::from_millis(33);
const MIN_INTERVAL: Duration = Duration::from_millis(50);

/// The SDK sends percentages (0-100).
fn pct(r: i32, g: i32, b: i32) -> Rgb {
    let c = |p: i32| ((p.clamp(0, 100) * 255 + 50) / 100) as u8;
    Rgb::new(c(r), c(g), c(b))
}

fn scale(c: Rgb, level: f64) -> Rgb {
    let s = |x: u8| (x as f64 * level.clamp(0.0, 1.0)).round() as u8;
    Rgb::new(s(c.r), s(c.g), s(c.b))
}

/// What the game left painted (without effects).
#[derive(Clone, Default, Debug, PartialEq)]
struct Lighting {
    /// From `SetLighting`, the bitmap or a zone.
    frame: Option<Frame>,
    /// Individual keys, by (code type, code). Without each key's position they are only used when
    /// there is nothing else: their average on the 4 zones.
    keys: BTreeMap<(i32, i32), Rgb>,
}

impl Lighting {
    fn render(&self) -> Option<Frame> {
        self.frame.or_else(|| (!self.keys.is_empty()).then(|| [average_lit(self.keys.values().copied()); ZONES]))
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Anim {
    /// On for half of each interval, black for the other half.
    Flash,
    /// Fades to black and back, once per interval.
    Pulse,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Effect {
    anim: Anim,
    color: Rgb,
    start: Instant,
    /// `None`: until the game stops it (`LOGI_LED_DURATION_INFINITE`).
    end: Option<Instant>,
    interval: Duration,
}

impl Effect {
    fn new(anim: Anim, color: Rgb, duration_ms: i32, interval_ms: i32, now: Instant) -> Self {
        let end = (duration_ms > 0).then(|| now + Duration::from_millis(duration_ms as u64));
        let interval = Duration::from_millis(interval_ms.max(0) as u64).max(MIN_INTERVAL);
        Self { anim, color, start: now, end, interval }
    }

    fn running(&self, now: Instant) -> bool {
        self.end.is_none_or(|e| now < e)
    }

    /// Fraction (0-1) of the current interval.
    fn phase(&self, now: Instant) -> f64 {
        let t = now.saturating_duration_since(self.start).as_secs_f64();
        (t / self.interval.as_secs_f64()).fract()
    }

    fn color(&self, now: Instant) -> Rgb {
        let phase = self.phase(now);
        match self.anim {
            Anim::Flash if phase < 0.5 => self.color,
            Anim::Flash => Rgb::OFF,
            Anim::Pulse => scale(self.color, (1.0 - 2.0 * phase).abs()),
        }
    }

    fn next(&self, now: Instant) -> Option<Instant> {
        if !self.running(now) {
            return None;
        }
        let next = match self.anim {
            Anim::Pulse => now + PULSE_FRAME,
            Anim::Flash => {
                let half = self.interval.as_secs_f64() / 2.0;
                let halves = (now.saturating_duration_since(self.start).as_secs_f64() / half) as u32;
                self.start + Duration::from_secs_f64(half * (halves + 1) as f64) + Duration::from_millis(1)
            }
        };
        Some(self.end.map_or(next, |e| next.min(e)))
    }
}

#[derive(Default)]
struct Session {
    light: Lighting,
    saved: Option<Lighting>,
    effect: Option<Effect>,
}

impl Session {
    fn call(&mut self, func: u8, args: &[i32], blob: &[u8], now: Instant) {
        let arg = |i: usize| args.get(i).copied().unwrap_or(0);
        match func {
            F_SET_LIGHTING => {
                self.light = Lighting { frame: Some([pct(arg(0), arg(1), arg(2)); ZONES]), keys: BTreeMap::new() };
            }
            F_BITMAP if blob.len() == BITMAP_COLS * BITMAP_ROWS * 4 => {
                let cells: Vec<Rgb> = blob.as_chunks::<4>().0.iter().map(|[b, g, r, _]| Rgb::new(*r, *g, *b)).collect();
                self.light = Lighting { frame: Some(mapping::grid(BITMAP_ROWS, BITMAP_COLS, &cells)), keys: BTreeMap::new() };
            }
            F_KEY => {
                self.light.keys.insert((arg(0), arg(1)), pct(arg(2), arg(3), arg(4)));
            }
            // Neither the device type nor the zone number says where it lands on the rings.
            F_TARGET_ZONE => {
                self.light.frame = Some([pct(arg(2), arg(3), arg(4)); ZONES]);
            }
            F_FLASH => self.effect = Some(Effect::new(Anim::Flash, pct(arg(0), arg(1), arg(2)), arg(3), arg(4), now)),
            F_PULSE => self.effect = Some(Effect::new(Anim::Pulse, pct(arg(0), arg(1), arg(2)), arg(3), arg(4), now)),
            F_STOP_EFFECTS => self.effect = None,
            F_SAVE => self.saved = Some(self.light.clone()),
            F_RESTORE => {
                if let Some(saved) = &self.saved {
                    self.light = saved.clone();
                }
                self.effect = None;
            }
            _ => {}
        }
    }

    /// What is visible at `now`: the effect over what was painted; `None` if the game painted nothing.
    fn render(&mut self, now: Instant) -> Option<Frame> {
        if self.effect.is_some_and(|e| !e.running(now)) {
            self.effect = None; // when it ends, the previous lighting returns, as in LGS
        }
        match self.effect {
            Some(e) => Some([e.color(now); ZONES]),
            None => self.light.render(),
        }
    }

    fn next_tick(&self, now: Instant) -> Option<Instant> {
        self.effect.and_then(|e| e.next(now))
    }
}

enum Msg {
    Hello(String),
    Call(u8, Vec<i32>, Vec<u8>),
}

fn parse(buf: &[u8]) -> Option<Msg> {
    match buf {
        [OP_HELLO, name @ ..] => {
            Some(Msg::Hello(String::from_utf8_lossy(name).chars().filter(|c| !c.is_control()).take(64).collect()))
        }
        [OP_CALL, func, n, rest @ ..] => {
            let n = *n as usize;
            let args = rest.get(..n * 4)?.as_chunks::<4>().0.iter().map(|w| i32::from_le_bytes(*w)).collect();
            Some(Msg::Call(*func, args, rest[n * 4..].to_vec()))
        }
        _ => None,
    }
}

async fn read_messages(mut stream: TcpStream, tx: mpsc::Sender<Msg>) -> io::Result<()> {
    let mut buf = Vec::new();
    loop {
        let len = stream.read_u32_le().await? as usize;
        if len == 0 || len > MAX_MESSAGE {
            return Err(io::Error::new(io::ErrorKind::InvalidData, format!("message of {len} bytes")));
        }
        buf.resize(len, 0);
        stream.read_exact(&mut buf).await?;
        match parse(&buf) {
            Some(msg) => {
                if tx.send(msg).await.is_err() {
                    return Ok(());
                }
            }
            None => eprintln!("halo: logitech: unknown message {}", buf[0]),
        }
    }
}

async fn session(core: &Mutex<Core>, stream: TcpStream, layer: &mut String) -> io::Result<()> {
    // The socket is read in another task, so the effect clock never cuts a read in half.
    let (tx, mut rx) = mpsc::channel(64);
    let reader = tokio::spawn(read_messages(stream, tx));
    let mut state = Session::default();
    let mut shown: Option<Frame> = None;
    loop {
        let next = state.next_tick(Instant::now());
        let wait = async {
            match next {
                Some(t) => tokio::time::sleep_until(t.into()).await,
                None => std::future::pending().await,
            }
        };
        tokio::select! {
            msg = rx.recv() => match msg {
                None => break,
                Some(Msg::Hello(name)) => {
                    let renamed = format!("logitech:{name}#{}", layer.rsplit('#').next().unwrap_or("?"));
                    core.lock().await.comp.remove_layer(layer);
                    eprintln!("halo: {renamed} connected");
                    *layer = renamed;
                    shown = None;
                }
                Some(Msg::Call(func, args, blob)) => state.call(func, &args, &blob, Instant::now()),
            },
            _ = wait => {}
        }
        let frame = state.render(Instant::now());
        if frame != shown || (frame.is_some() && shown.is_none()) {
            let mut core = core.lock().await;
            match frame {
                Some(f) => core.comp.set_layer(layer, Layer { priority: PRIORITY_GAME, zones: f.map(Some) }),
                None => {
                    core.comp.remove_layer(layer);
                }
            }
            core.push()?;
            shown = frame;
        }
    }
    reader.await.unwrap_or(Ok(()))
}

pub async fn serve(core: Arc<Mutex<Core>>, port: u16) -> io::Result<()> {
    let listener = TcpListener::bind(("127.0.0.1", port)).await?;
    eprintln!("halo: Logitech bridge listening on 127.0.0.1:{port}");
    loop {
        let (stream, peer) = listener.accept().await?;
        let core = core.clone();
        tokio::spawn(async move {
            let mut layer = format!("logitech#{}", peer.port());
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

#[cfg(test)]
mod tests {
    use super::*;

    const RED: Rgb = Rgb::new(255, 0, 0);
    const BLUE: Rgb = Rgb::new(0, 0, 255);

    fn ms(t: Instant, n: u64) -> Instant {
        t + Duration::from_millis(n)
    }

    #[test]
    fn solid_bitmap_and_keys() {
        let t = Instant::now();
        let mut s = Session::default();
        assert_eq!(s.render(t), None);
        s.call(F_KEY, &[3, 0x11, 0, 0, 100], &[], t); // W in blue
        assert_eq!(s.render(t), Some([BLUE; ZONES]), "keys only: their average");
        s.call(F_SET_LIGHTING, &[100, 0, 0], &[], t);
        assert_eq!(s.render(t), Some([RED; ZONES]));
        assert_eq!(pct(50, 0, 0), Rgb::new(128, 0, 0));

        let mut bitmap = vec![0u8; BITMAP_COLS * BITMAP_ROWS * 4];
        bitmap[..4].copy_from_slice(&[0, 0, 255, 255]); // top left, BGRA red
        s.call(F_BITMAP, &[], &bitmap, t);
        assert_eq!(s.render(t), Some([Rgb::OFF, RED, Rgb::OFF, Rgb::OFF]));
        s.call(F_BITMAP, &[], &bitmap[..8], t);
        assert_eq!(s.render(t), Some([Rgb::OFF, RED, Rgb::OFF, Rgb::OFF]), "short bitmap: ignored");
    }

    #[test]
    fn save_restore() {
        let t = Instant::now();
        let mut s = Session::default();
        s.call(F_SET_LIGHTING, &[0, 0, 100], &[], t);
        s.call(F_SAVE, &[], &[], t);
        s.call(F_SET_LIGHTING, &[100, 0, 0], &[], t);
        s.call(F_RESTORE, &[], &[], t);
        assert_eq!(s.render(t), Some([BLUE; ZONES]));
    }

    #[test]
    fn flash_ends_and_returns() {
        let t = Instant::now();
        let mut s = Session::default();
        s.call(F_SET_LIGHTING, &[0, 0, 100], &[], t);
        // Red, 1 s, 200 ms interval: 100 ms on, 100 ms black.
        s.call(F_FLASH, &[100, 0, 0, 1000, 200], &[], t);
        assert_eq!(s.render(ms(t, 50)), Some([RED; ZONES]));
        assert_eq!(s.render(ms(t, 150)), Some([Rgb::OFF; ZONES]));
        assert_eq!(s.next_tick(ms(t, 50)), Some(ms(t, 101)));
        assert_eq!(s.next_tick(ms(t, 950)), Some(ms(t, 1000)), "does not go past the end");
        assert_eq!(s.render(ms(t, 1000)), Some([BLUE; ZONES]), "when it ends, the previous lighting returns");
        assert_eq!(s.next_tick(ms(t, 1000)), None, "no effect: no need to wake");
    }

    #[test]
    fn pulse_infinite_until_stopped() {
        let t = Instant::now();
        let mut s = Session::default();
        s.call(F_PULSE, &[100, 0, 0, 0, 1000], &[], t);
        assert_eq!(s.render(t), Some([RED; ZONES]));
        assert_eq!(s.render(ms(t, 500)), Some([Rgb::OFF; ZONES]), "halfway: black");
        assert_eq!(s.render(ms(t, 250)).unwrap()[0], Rgb::new(128, 0, 0));
        assert_eq!(s.next_tick(ms(t, 60_000)), Some(ms(t, 60_000) + PULSE_FRAME), "infinite");
        s.call(F_STOP_EFFECTS, &[], &[], t);
        assert_eq!(s.render(t), None);
    }

    #[test]
    fn parses_the_wire_format() {
        let mut msg = vec![OP_CALL, F_SET_LIGHTING, 3];
        for v in [100i32, 50, 0] {
            msg.extend(v.to_le_bytes());
        }
        match parse(&msg) {
            Some(Msg::Call(F_SET_LIGHTING, args, blob)) => assert_eq!((args, blob.len()), (vec![100, 50, 0], 0)),
            _ => panic!("did not parse"),
        }
        assert!(parse(&[OP_CALL, F_FLASH, 5, 0, 0]).is_none(), "missing arguments");
        assert!(matches!(parse(b"\x01Game"), Some(Msg::Hello(n)) if n == "Game"));
    }
}
