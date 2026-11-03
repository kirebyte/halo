//! Gamepad navigation. Reads every gamepad the user can open and turns it into key events for the
//! focused window, as if from a keyboard: d-pad and left stick send Up / Down / Left / Right (a
//! popup list or a slider takes them; otherwise `Main.qml` turns Up / Down into Backtab / Tab to
//! move the focus), A sends Space (activate), B Escape (back), LB / RB Back / Forward (change
//! page). Nothing is injected while no window of ours has the focus, so a game keeps its inputs.
//!
//! Two kinds of source. Any evdev gamepad in `/dev/input` (a real pad on a desktop, or the virtual
//! X-Box pad Steam Input creates while Steam runs). And the Steam Deck protocol over hidraw, which
//! is how the Ally's own pad reaches user space on Bazzite: InputPlumber grabs the real device and
//! re-exposes it as a virtual "Steam Deck" controller that only has a vendor HID report, so there
//! is no evdev node for it until Steam opens it. Reading hidraw is not exclusive, so while Steam is
//! up its virtual X-Box pad would repeat every press; it is ignored while a Deck source is open.
//!
//! Only libc: `/dev/input` and `/dev` are scanned at start and whenever inotify reports a new node
//! (udev sets the ACL a moment after creation, hence the short delay), and each pad gets a thread
//! that blocks in `poll` until it says something or is unplugged.

use std::collections::HashSet;
use std::ffi::CString;
use std::os::unix::ffi::OsStrExt;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const EV_KEY: u16 = 0x01;
const EV_ABS: u16 = 0x03;
const BTN_SOUTH: u16 = 0x130; // A
const BTN_EAST: u16 = 0x131; // B
const BTN_TL: u16 = 0x136; // LB
const BTN_TR: u16 = 0x137; // RB
const BTN_DPAD_UP: u16 = 0x220;
const BTN_DPAD_DOWN: u16 = 0x221;
const BTN_DPAD_LEFT: u16 = 0x222;
const BTN_DPAD_RIGHT: u16 = 0x223;
const ABS_X: u16 = 0x00;
const ABS_Y: u16 = 0x01;
const ABS_HAT0X: u16 = 0x10;
const ABS_HAT0Y: u16 = 0x11;

/// Valve's vendor id: Steam's virtual pads and the Deck-protocol devices.
const VENDOR_VALVE: u16 = 0x28de;
/// Steam Deck input report: `01 00 09 40`, then a sequence number, buttons, pads, IMU and sticks.
const DECK_REPORT: usize = 64;
const DECK_HEADER: [u8; 4] = [0x01, 0x00, 0x09, 0x40];
/// Buttons of the Deck report as (byte, bit, evdev code), from the kernel's hid-steam.
const DECK_BUTTONS: [(usize, u8, u16); 8] = [
    (8, 2, BTN_TR),
    (8, 3, BTN_TL),
    (8, 5, BTN_EAST),
    (8, 7, BTN_SOUTH),
    (9, 0, BTN_DPAD_UP),
    (9, 1, BTN_DPAD_RIGHT),
    (9, 2, BTN_DPAD_LEFT),
    (9, 3, BTN_DPAD_DOWN),
];
const DECK_STICK_X: usize = 48;
const DECK_STICK_Y: usize = 50;

/// How many Deck-protocol sources are open; Steam's virtual pads are muted while it is not zero.
static DECK_SOURCES: AtomicUsize = AtomicUsize::new(0);
/// `HALO_GUI_DEBUG_PAD` set: every decoded pad event and key sent goes to stderr.
static DEBUG: AtomicBool = AtomicBool::new(false);

// Qt::Key values (qnamespace.h).
const KEY_SPACE: i32 = 0x20;
const KEY_ESCAPE: i32 = 0x0100_0000;
const KEY_LEFT: i32 = 0x0100_0012;
const KEY_UP: i32 = 0x0100_0013;
const KEY_RIGHT: i32 = 0x0100_0014;
const KEY_DOWN: i32 = 0x0100_0015;
const KEY_BACK: i32 = 0x0100_0061;
const KEY_FORWARD: i32 = 0x0100_0062;

/// Stick travel (of ±32767) that counts as a press, and where it counts as released again.
const STICK_ON: i32 = 16000;
const STICK_OFF: i32 = 8000;
/// A held direction repeats after this long, then at this rate.
const REPEAT_DELAY: Duration = Duration::from_millis(400);
const REPEAT_RATE: Duration = Duration::from_millis(120);

#[repr(C)]
struct InputEvent {
    time: libc::timeval,
    type_: u16,
    code: u16,
    value: i32,
}

/// `_IOC(_IOC_READ, type, nr, size)`.
fn ioc_read(type_: u8, nr: u64, size: usize) -> libc::c_ulong {
    ((2u64 << 30) | ((size as u64) << 16) | (u64::from(type_) << 8) | nr) as libc::c_ulong
}

fn has_bit(bits: &[u8], bit: u16) -> bool {
    bits.get(usize::from(bit) / 8).is_some_and(|b| b & (1 << (bit % 8)) != 0)
}

/// The device's name if it looks like a gamepad: a south button and a hat (or a left stick).
fn gamepad_name(fd: libc::c_int) -> Option<String> {
    let mut keys = [0u8; 0x300 / 8];
    let mut abs = [0u8; 8];
    let mut name = [0u8; 128];
    // SAFETY: EVIOCGBIT and EVIOCGNAME fill at most the buffer size they are given.
    unsafe {
        if libc::ioctl(fd, ioc_read(b'E', 0x20 + u64::from(EV_KEY), keys.len()), keys.as_mut_ptr()) < 0
            || libc::ioctl(fd, ioc_read(b'E', 0x20 + u64::from(EV_ABS), abs.len()), abs.as_mut_ptr()) < 0
        {
            return None;
        }
        if libc::ioctl(fd, ioc_read(b'E', 0x06, name.len()), name.as_mut_ptr()) < 0 {
            name[0] = 0;
        }
    }
    if !has_bit(&keys, BTN_SOUTH) || !(has_bit(&abs, ABS_HAT0X) || has_bit(&abs, ABS_Y)) {
        return None;
    }
    let end = name.iter().position(|&b| b == 0).unwrap_or(name.len());
    Some(String::from_utf8_lossy(&name[..end]).into_owned())
}

/// The vendor id of an evdev device (EVIOCGID), 0 if it will not say.
fn evdev_vendor(fd: libc::c_int) -> u16 {
    let mut id = [0u16; 4]; // bustype, vendor, product, version
    // SAFETY: EVIOCGID writes exactly 8 bytes.
    if unsafe { libc::ioctl(fd, ioc_read(b'E', 0x02, 8), id.as_mut_ptr()) } < 0 {
        return 0;
    }
    id[1]
}

/// The name of a hidraw device if it speaks the Steam Deck protocol (Valve's vendor id and a
/// vendor-defined report; InputPlumber's "ROG Xbox Ally Controller" is one).
fn deck_name(fd: libc::c_int) -> Option<String> {
    let mut info = [0u32; 2]; // bustype, then vendor (low half) and product (high half)
    let mut name = [0u8; 128];
    // SAFETY: HIDIOCGRAWINFO writes 8 bytes; HIDIOCGRAWNAME at most the length given.
    unsafe {
        if libc::ioctl(fd, ioc_read(b'H', 0x03, 8), info.as_mut_ptr()) < 0 {
            return None;
        }
        if libc::ioctl(fd, ioc_read(b'H', 0x04, name.len()), name.as_mut_ptr()) < 0 {
            name[0] = 0;
        }
    }
    if (info[1] & 0xffff) as u16 != VENDOR_VALVE {
        return None;
    }
    let end = name.iter().position(|&b| b == 0).unwrap_or(name.len());
    Some(String::from_utf8_lossy(&name[..end]).into_owned())
}

fn send(key: i32) {
    if DEBUG.load(Ordering::Relaxed) {
        eprintln!("halo-gui: pad sends key {key:#x}");
    }
    #[cfg(test)]
    tests::SENT.lock().expect("sent keys").push(key);
    crate::halo::qobject::send_key(key, 0);
}

/// Starts the scanner thread; returns at once.
pub fn start() {
    DEBUG.store(std::env::var_os("HALO_GUI_DEBUG_PAD").is_some(), Ordering::Relaxed);
    let _ = std::thread::Builder::new().name("gamepad".into()).spawn(scanner);
}

fn scanner() {
    let open: Arc<Mutex<HashSet<String>>> = Arc::default();
    // SAFETY: plain libc calls with a valid C string.
    let inotify = unsafe {
        let fd = libc::inotify_init1(libc::IN_CLOEXEC);
        if fd >= 0 {
            for dir in ["/dev/input", "/dev"] {
                let dir = CString::new(dir).expect("no NUL");
                libc::inotify_add_watch(fd, dir.as_ptr(), libc::IN_CREATE | libc::IN_ATTRIB);
            }
        }
        fd
    };
    loop {
        scan(&open);
        if inotify < 0 {
            return;
        }
        let mut buf = [0u8; 4096];
        loop {
            // SAFETY: reads into a buffer of the stated size; blocks until a watched dir changes.
            let n = unsafe { libc::read(inotify, buf.as_mut_ptr().cast(), buf.len()) };
            if n < 0 {
                return;
            }
            if inotify_names(&buf[..n as usize]).any(|name| name.starts_with(b"event") || name.starts_with(b"hidraw")) {
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(300));
    }
}

/// The file names in a buffer of `inotify_event`s.
fn inotify_names(buf: &[u8]) -> impl Iterator<Item = &[u8]> {
    const HEADER: usize = 16; // wd, mask, cookie, len
    let mut at = 0;
    std::iter::from_fn(move || {
        let head = buf.get(at..at + HEADER)?;
        let len = u32::from_ne_bytes(head[12..16].try_into().ok()?) as usize;
        let name = buf.get(at + HEADER..at + HEADER + len)?;
        at += HEADER + len;
        let end = name.iter().position(|&b| b == 0).unwrap_or(name.len());
        Some(&name[..end])
    })
}

/// Every node that looks like a gamepad: its name, path and open descriptor with its source.
fn probe() -> Vec<(String, String, libc::c_int, Source)> {
    let mut found = Vec::new();
    for (dir, prefix) in [("/dev/input", &b"event"[..]), ("/dev", &b"hidraw"[..])] {
        let Ok(entries) = std::fs::read_dir(dir) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.file_name().is_some_and(|n| n.as_bytes().starts_with(prefix)) {
                continue;
            }
            let key = path.to_string_lossy().into_owned();
            let Ok(cpath) = CString::new(key.clone()) else { continue };
            // SAFETY: opens a path we own as a C string; the fd is closed by the caller or below.
            let fd = unsafe { libc::open(cpath.as_ptr(), libc::O_RDONLY | libc::O_NONBLOCK | libc::O_CLOEXEC) };
            if fd < 0 {
                continue;
            }
            let source = if prefix == b"event" {
                gamepad_name(fd).map(|name| (name, Source::Evdev { steam: evdev_vendor(fd) == VENDOR_VALVE }))
            } else {
                deck_name(fd).map(|name| (name, Source::Deck { last: [0; DECK_REPORT] }))
            };
            match source {
                Some((name, source)) => found.push((name, key, fd, source)),
                // SAFETY: the fd is ours and not shared.
                None => unsafe {
                    libc::close(fd);
                },
            }
        }
    }
    found
}

/// The gamepads the reader would use, as (name, path); for `--gamepads`.
pub fn list() -> Vec<(String, String)> {
    probe()
        .into_iter()
        .map(|(name, path, fd, _)| {
            // SAFETY: the fd is ours and not shared.
            unsafe { libc::close(fd) };
            (name, path)
        })
        .collect()
}

/// Opens every gamepad not yet open and gives it a reader thread.
fn scan(open: &Arc<Mutex<HashSet<String>>>) {
    for (name, key, fd, source) in probe() {
        if open.lock().expect("gamepad set").contains(&key) {
            // SAFETY: the fd is ours and not shared.
            unsafe { libc::close(fd) };
            continue;
        }
        eprintln!("halo-gui: gamepad {name} ({key})");
        open.lock().expect("gamepad set").insert(key.clone());
        let open = Arc::clone(open);
        let _ = std::thread::Builder::new().name("gamepad-reader".into()).spawn(move || {
            reader(fd, source);
            // SAFETY: fd is ours and no longer used after this.
            unsafe { libc::close(fd) };
            open.lock().expect("gamepad set").remove(&key);
        });
    }
}

/// What a reader thread is reading and how to turn it into (type, code, value) events.
enum Source {
    /// An evdev gamepad; `steam` marks Steam's virtual pads, muted while a Deck source is open.
    Evdev { steam: bool },
    /// Steam Deck reports over hidraw, diffed against the last one.
    Deck { last: [u8; DECK_REPORT] },
}

impl Source {
    fn decode(&mut self, raw: &[u8], out: &mut Vec<(u16, u16, i32)>) {
        match self {
            Source::Evdev { steam } => {
                if *steam && DECK_SOURCES.load(Ordering::Relaxed) > 0 {
                    return;
                }
                let size = std::mem::size_of::<InputEvent>();
                for chunk in raw.chunks_exact(size) {
                    // SAFETY: the kernel writes whole, aligned input_event structs; we read fields by copy.
                    let ev: InputEvent = unsafe { std::ptr::read_unaligned(chunk.as_ptr().cast()) };
                    out.push((ev.type_, ev.code, ev.value));
                }
            }
            Source::Deck { last } => {
                if raw.len() != DECK_REPORT || raw[..4] != DECK_HEADER {
                    return;
                }
                for (byte, bit, code) in DECK_BUTTONS {
                    let (now, before) = (raw[byte] >> bit & 1, last[byte] >> bit & 1);
                    if now != before {
                        out.push((EV_KEY, code, i32::from(now)));
                    }
                }
                for (at, code, sign) in [(DECK_STICK_X, ABS_X, 1), (DECK_STICK_Y, ABS_Y, -1)] {
                    let axis = |r: &[u8]| i32::from(i16::from_le_bytes([r[at], r[at + 1]])) * sign;
                    if axis(raw) != axis(last) {
                        out.push((EV_ABS, code, axis(raw)));
                    }
                }
                last.copy_from_slice(raw);
            }
        }
    }
}

/// What the pad is holding in each direction source; the first one set is what repeats.
#[derive(Default)]
struct Held {
    hat_x: Option<i32>,
    hat_y: Option<i32>,
    stick_x: Option<i32>,
    stick_y: Option<i32>,
    buttons: Option<i32>,
    since: Option<Instant>,
    last: Option<Instant>,
}

impl Held {
    fn current(&self) -> Option<i32> {
        self.hat_y.or(self.hat_x).or(self.buttons).or(self.stick_y).or(self.stick_x)
    }

    /// Sets one source; a direction that just appeared is sent at once and starts repeating.
    fn set(&mut self, slot: fn(&mut Held) -> &mut Option<i32>, key: Option<i32>) {
        let before = self.current();
        *slot(self) = key;
        let after = self.current();
        if after != before {
            if let Some(k) = after {
                send(k);
                let now = Instant::now();
                self.since = Some(now);
                self.last = Some(now);
            } else {
                self.since = None;
                self.last = None;
            }
        }
    }

    fn repeat(&mut self) {
        let (Some(key), Some(since), Some(last)) = (self.current(), self.since, self.last) else { return };
        let now = Instant::now();
        if now.duration_since(since) >= REPEAT_DELAY && now.duration_since(last) >= REPEAT_RATE {
            send(key);
            self.last = Some(now);
        }
    }
}

fn stick(value: i32, previous: Option<i32>, negative: i32, positive: i32) -> Option<i32> {
    if value <= -STICK_ON {
        Some(negative)
    } else if value >= STICK_ON {
        Some(positive)
    } else if value.abs() < STICK_OFF {
        None
    } else {
        previous
    }
}

fn handle((type_, code, value): (u16, u16, i32), held: &mut Held) {
    if DEBUG.load(Ordering::Relaxed) {
        eprintln!("halo-gui: pad event type {type_:#x} code {code:#x} value {value}");
    }
    match (type_, code) {
        (EV_KEY, BTN_SOUTH) if value == 1 => send(KEY_SPACE),
        (EV_KEY, BTN_EAST) if value == 1 => send(KEY_ESCAPE),
        (EV_KEY, BTN_TL) if value == 1 => send(KEY_BACK),
        (EV_KEY, BTN_TR) if value == 1 => send(KEY_FORWARD),
        (EV_KEY, BTN_DPAD_UP..=BTN_DPAD_RIGHT) => {
            let key = match code {
                BTN_DPAD_UP => KEY_UP,
                BTN_DPAD_DOWN => KEY_DOWN,
                BTN_DPAD_LEFT => KEY_LEFT,
                _ => KEY_RIGHT,
            };
            held.set(|h| &mut h.buttons, (value != 0).then_some(key));
        }
        (EV_ABS, ABS_HAT0Y) => held.set(|h| &mut h.hat_y, match value { v if v < 0 => Some(KEY_UP), v if v > 0 => Some(KEY_DOWN), _ => None }),
        (EV_ABS, ABS_HAT0X) => held.set(|h| &mut h.hat_x, match value { v if v < 0 => Some(KEY_LEFT), v if v > 0 => Some(KEY_RIGHT), _ => None }),
        (EV_ABS, ABS_Y) => {
            let key = stick(value, held.stick_y, KEY_UP, KEY_DOWN);
            held.set(|h| &mut h.stick_y, key);
        }
        (EV_ABS, ABS_X) => {
            let key = stick(value, held.stick_x, KEY_LEFT, KEY_RIGHT);
            held.set(|h| &mut h.stick_x, key);
        }
        _ => {}
    }
}

/// Reads one pad until it goes away.
fn reader(fd: libc::c_int, mut source: Source) {
    let deck = matches!(source, Source::Deck { .. });
    if deck {
        DECK_SOURCES.fetch_add(1, Ordering::Relaxed);
    }
    read_loop(fd, &mut source);
    if deck {
        DECK_SOURCES.fetch_sub(1, Ordering::Relaxed);
    }
}

fn read_loop(fd: libc::c_int, source: &mut Source) {
    let mut held = Held::default();
    let mut pfd = libc::pollfd { fd, events: libc::POLLIN, revents: 0 };
    let mut buf = [0u8; 16 * std::mem::size_of::<InputEvent>()];
    let mut events = Vec::new();
    loop {
        let timeout = if held.current().is_some() { 50 } else { -1 };
        // SAFETY: one pollfd, as declared.
        let n = unsafe { libc::poll(&mut pfd, 1, timeout) };
        if n < 0 {
            if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return;
        }
        if pfd.revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) != 0 {
            return;
        }
        if pfd.revents & libc::POLLIN != 0 {
            loop {
                // SAFETY: reads at most the buffer's length; evdev gives whole input_event structs,
                // hidraw one report per read.
                let got = unsafe { libc::read(fd, buf.as_mut_ptr().cast(), buf.len()) };
                if got < 0 {
                    let err = std::io::Error::last_os_error();
                    match err.kind() {
                        std::io::ErrorKind::WouldBlock => break,
                        std::io::ErrorKind::Interrupted => continue,
                        _ => return,
                    }
                }
                if got == 0 {
                    return;
                }
                events.clear();
                source.decode(&buf[..got as usize], &mut events);
                for ev in events.drain(..) {
                    handle(ev, &mut held);
                }
            }
        }
        held.repeat();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub static SENT: Mutex<Vec<i32>> = Mutex::new(Vec::new());

    /// The whole read path with a pipe standing in for hidraw: a report with A and d-pad down
    /// pressed, then the stick up, then everything released.
    #[test]
    fn deck_read_loop() {
        let mut fds = [0 as libc::c_int; 2];
        // SAFETY: a fresh pipe; both ends are closed below.
        assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
        let (rd, wr) = (fds[0], fds[1]);
        SENT.lock().expect("sent keys").clear();
        let reader = std::thread::spawn(move || read_loop(rd, &mut Source::Deck { last: [0; DECK_REPORT] }));
        let mut report = [0u8; DECK_REPORT];
        report[..4].copy_from_slice(&DECK_HEADER);
        let write = |r: &[u8]| {
            // SAFETY: writes one whole report from a live buffer.
            assert_eq!(unsafe { libc::write(wr, r.as_ptr().cast(), r.len()) }, r.len() as isize);
            std::thread::sleep(Duration::from_millis(50));
        };
        write(&report); // idle
        report[8] = 1 << 7;
        report[9] = 1 << 3;
        write(&report);
        report[8] = 0;
        report[9] = 0;
        report[50..52].copy_from_slice(&30000i16.to_le_bytes());
        write(&report);
        report[50..52].fill(0);
        write(&report);
        // SAFETY: closing our end makes the reader see EOF and return; it closes nothing itself.
        unsafe { libc::close(wr) };
        reader.join().expect("reader thread");
        // SAFETY: the reader is done with it.
        unsafe { libc::close(rd) };
        assert_eq!(*SENT.lock().expect("sent keys"), vec![KEY_SPACE, KEY_DOWN, KEY_UP]);
    }

    #[test]
    fn stick_hysteresis() {
        assert_eq!(stick(-20000, None, KEY_LEFT, KEY_RIGHT), Some(KEY_LEFT));
        assert_eq!(stick(-10000, Some(KEY_LEFT), KEY_LEFT, KEY_RIGHT), Some(KEY_LEFT));
        assert_eq!(stick(-10000, None, KEY_LEFT, KEY_RIGHT), None);
        assert_eq!(stick(3000, Some(KEY_LEFT), KEY_LEFT, KEY_RIGHT), None);
        assert_eq!(stick(32767, None, KEY_LEFT, KEY_RIGHT), Some(KEY_RIGHT));
    }

    #[test]
    fn bits() {
        let mut bits = [0u8; 4];
        bits[2] = 0b0000_0100; // bit 18
        assert!(has_bit(&bits, 18));
        assert!(!has_bit(&bits, 17));
        assert!(!has_bit(&bits, 200));
    }

    #[test]
    fn ioctl_numbers() {
        // EVIOCGNAME(128), EVIOCGBIT(EV_KEY, 96), EVIOCGID and HIDIOCGRAWINFO as the kernel
        // headers define them.
        assert_eq!(ioc_read(b'E', 0x06, 128), 0x8080_4506);
        assert_eq!(ioc_read(b'E', 0x21, 96), 0x8060_4521);
        assert_eq!(ioc_read(b'E', 0x02, 8), 0x8008_4502);
        assert_eq!(ioc_read(b'H', 0x03, 8), 0x8008_4803);
    }

    #[test]
    fn deck_reports() {
        let mut source = Source::Deck { last: [0; DECK_REPORT] };
        let mut out = Vec::new();
        let mut report = [0u8; DECK_REPORT];
        report[..4].copy_from_slice(&DECK_HEADER);
        // Idle: nothing changes.
        source.decode(&report, &mut out);
        assert!(out.is_empty());
        // A pressed, d-pad down, stick pushed up (positive raw Y means up on the Deck).
        report[8] = 1 << 7;
        report[9] = 1 << 3;
        report[50..52].copy_from_slice(&20000i16.to_le_bytes());
        source.decode(&report, &mut out);
        assert_eq!(out, vec![(EV_KEY, BTN_SOUTH, 1), (EV_KEY, BTN_DPAD_DOWN, 1), (EV_ABS, ABS_Y, -20000)]);
        // Released.
        out.clear();
        report[8] = 0;
        report[9] = 0;
        report[50..52].fill(0);
        source.decode(&report, &mut out);
        assert_eq!(out, vec![(EV_KEY, BTN_SOUTH, 0), (EV_KEY, BTN_DPAD_DOWN, 0), (EV_ABS, ABS_Y, 0)]);
        // Other report types and sizes are ignored.
        out.clear();
        source.decode(&[0x01, 0x00, 0x03, 0x40], &mut out);
        assert!(out.is_empty());
    }

    #[test]
    fn inotify_parsing() {
        let mut buf = Vec::new();
        for name in ["event17\0", "hidraw7\0\0\0\0\0"] {
            buf.extend_from_slice(&1i32.to_ne_bytes());
            buf.extend_from_slice(&0x100u32.to_ne_bytes());
            buf.extend_from_slice(&0u32.to_ne_bytes());
            buf.extend_from_slice(&(name.len() as u32).to_ne_bytes());
            buf.extend_from_slice(name.as_bytes());
        }
        let names: Vec<&[u8]> = inotify_names(&buf).collect();
        assert_eq!(names, vec![&b"event17"[..], &b"hidraw7"[..]]);
    }
}
