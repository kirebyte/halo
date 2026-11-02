//! SteelSeries GameSense: the game reads `coreProps.json` (written by `halo proton install`) and
//! sends JSON over HTTP to `127.0.0.1:54237`. Reference: github.com/SteelSeries/gamesense-sdk (doc/api).
//!
//! Games register handlers ("paint zone X with a gradient based on health") and then send values.
//! Each active game is a layer. After 15 s without events (or whatever its metadata asks for), or
//! when `stop_game` arrives, it goes off, as in SteelSeries Engine. Handlers are saved to disk,
//! because the game registers them at startup and does not repeat them.
//!
//! Flashing (`rate`) does not use a fixed loop: the timeout task wakes exactly at each on/off
//! transition, and only while something is flashing.

use super::http::{self, Request, Response};
use super::mapping;
use crate::daemon::compositor::{Layer, PRIORITY_GAME, Zones};
use crate::daemon::device::Core;
use halo_core::{Rgb, ZONES};
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use std::{fs, io};
use tokio::net::TcpListener;
use tokio::sync::{Mutex, Notify};

pub const PORT: u16 = 54237;
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(15);
const ALL: [bool; ZONES] = [true; ZONES];
const LEFT: [bool; ZONES] = [true, true, false, false];
const RIGHT: [bool; ZONES] = [false, false, true, true];
const NUMBERS: [&str; 4] = ["one", "two", "three", "four"];
/// Faster is not distinguishable on an LED, and every change wakes the daemon.
const MAX_HZ: f64 = 30.0;

/// Which one paints on top when two handlers land on the same zone: what is meant for zoned
/// devices wins over the bitmap, and that over what is meant for keyboard/mouse/headset.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Class {
    Generic,
    Bitmap,
    Zoned,
}

#[derive(Clone, Debug)]
enum Paint {
    Color(Value),
    Context(String),
    Bitmap,
}

#[derive(Clone, Debug)]
struct Handler {
    mask: [bool; ZONES],
    class: Class,
    paint: Paint,
    rate: Option<Value>,
}

/// A flash in progress: `freq` cycles per second (half on, half black) since `start`, and with
/// `limit`, only that many cycles before staying on.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Flash {
    freq: f64,
    limit: Option<u64>,
    start: Instant,
}

impl Flash {
    /// Half cycles elapsed; `None` once the limit is reached.
    fn halves(&self, now: Instant) -> Option<u64> {
        let h = (now.saturating_duration_since(self.start).as_secs_f64() * self.freq * 2.0) as u64;
        match self.limit {
            Some(n) if h >= 2 * n => None,
            _ => Some(h),
        }
    }

    fn lit(&self, now: Instant) -> bool {
        self.halves(now).is_none_or(|h| h % 2 == 0)
    }

    /// The next on/off transition (1 ms later, to avoid landing right on the edge).
    fn next(&self, now: Instant) -> Option<Instant> {
        let h = self.halves(now)?;
        Some(self.start + Duration::from_secs_f64((h + 1) as f64 / (2.0 * self.freq)) + Duration::from_millis(1))
    }
}

/// A fixed number or one per `low`/`high` range of the value (as in `frequency` and `repeat_limit`).
fn ranged(spec: &Value, key: &str, v: i64) -> Option<f64> {
    if let Some(ranges) = spec.as_array() {
        let r = ranges.iter().find(|r| r["low"].as_i64().is_some_and(|lo| lo <= v) && r["high"].as_i64().is_some_and(|hi| v <= hi))?;
        return ranged(&r[key], key, v);
    }
    spec.as_f64()
}

/// Frequency and limit that apply for value `v`; `None` if it does not flash.
fn resolve_rate(rate: &Value, v: i64) -> Option<(f64, Option<u64>)> {
    let freq = ranged(&rate["frequency"], "frequency", v)?.min(MAX_HZ);
    if freq.is_nan() || freq <= 0.0 {
        return None;
    }
    let limit = rate.get("repeat_limit").and_then(|l| ranged(l, "repeat_limit", v)).map(|n| n.max(0.0) as u64);
    Some((freq, limit))
}

#[derive(Default)]
struct Event {
    handlers: Vec<Handler>,
    /// Each handler's flash (same order).
    flashes: Vec<Option<Flash>>,
    /// As they arrived, for saving.
    raw: Vec<Value>,
    value: i64,
    frame: Value,
    /// Sequence of the last `game_event` (0 = none arrived yet).
    seq: u64,
}

struct Game {
    display: Option<String>,
    timeout: Duration,
    events: BTreeMap<String, Event>,
    /// `Some` while the game is active.
    deadline: Option<Instant>,
}

impl Default for Game {
    fn default() -> Self {
        Self { display: None, timeout: DEFAULT_TIMEOUT, events: BTreeMap::new(), deadline: None }
    }
}

#[derive(Default)]
pub struct GameSense {
    games: BTreeMap<String, Game>,
    seq: u64,
    store: Option<PathBuf>,
}

fn valid_name(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
}

/// `data` and the handlers may come as an object or as JSON inside a string.
fn object(v: &Value) -> Option<Value> {
    match v {
        Value::String(s) => serde_json::from_str(s).ok(),
        Value::Object(_) => Some(v.clone()),
        _ => None,
    }
}

fn zone_mask(device: &str, zone: Option<&str>) -> Option<([bool; ZONES], Class)> {
    let zones = match device {
        "rgb-zoned-device" => 4,
        d if d.starts_with("rgb-") && d.ends_with("-zone") => d[4..d.len() - 5].parse().ok()?,
        "keyboard" | "rgb-per-key-zones" | "mouse" | "headset" | "indicator" => return Some((ALL, Class::Generic)),
        _ => return None, // tactile, screened...
    };
    let idx = NUMBERS.iter().position(|n| Some(*n) == zone)?;
    let mask = match (zones, idx) {
        (1, 0) => ALL,
        (1, _) => return None,
        (2, 0) => LEFT,
        (2, 1) => RIGHT,
        (2, _) => return None,
        (_, i) => {
            let mut m = [false; ZONES];
            m[i] = true;
            m
        }
    };
    Some((mask, Class::Zoned))
}

fn parse_handler(v: &Value) -> Option<Handler> {
    let v = object(v)?;
    let device = v["device-type"].as_str()?;
    let mode = v["mode"].as_str().unwrap_or("color");
    if mode == "bitmap" || mode == "partial-bitmap" {
        return (device == "rgb-per-key-zones").then_some(Handler { mask: ALL, class: Class::Bitmap, paint: Paint::Bitmap, rate: None });
    }
    let (mask, class) = zone_mask(device, v["zone"].as_str())?;
    let paint = match mode {
        "context-color" => Paint::Context(v["context-frame-key"].as_str()?.to_string()),
        _ => Paint::Color(v.get("color")?.clone()), // color, percent and count look the same on 4 LEDs
    };
    Some(Handler { mask, class, paint, rate: v.get("rate").cloned() })
}

fn static_color(c: &Value) -> Option<Rgb> {
    let ch = |k: &str| c[k].as_u64().map(|v| v.min(255) as u8);
    Some(Rgb::new(ch("red")?, ch("green")?, ch("blue")?))
}

fn lerp(a: Rgb, b: Rgb, v: i64) -> Rgb {
    let t = v.clamp(0, 100) as i32;
    let mix = |x: u8, y: u8| (x as i32 + (y as i32 - x as i32) * t / 100) as u8;
    Rgb::new(mix(a.r, b.r), mix(a.g, b.g), mix(a.b, b.b))
}

/// The color of a definition for the event's value (static, gradient or ranges).
fn compute(spec: &Value, v: i64, top: bool) -> Rgb {
    if let Some(ranges) = spec.as_array() {
        return ranges
            .iter()
            .find(|r| r["low"].as_i64().is_some_and(|lo| lo <= v) && r["high"].as_i64().is_some_and(|hi| v <= hi))
            .map(|r| compute(&r["color"], v, false))
            .unwrap_or(Rgb::OFF);
    }
    if let Some(g) = spec.get("gradient") {
        return match (static_color(&g["zero"]), static_color(&g["hundred"])) {
            (Some(a), Some(b)) => lerp(a, b, v),
            _ => Rgb::OFF,
        };
    }
    match static_color(spec) {
        Some(_) if top && v == 0 => Rgb::OFF, // static: black at value 0
        Some(c) => c,
        None => Rgb::OFF,
    }
}

fn bitmap(frame: &Value) -> Option<halo_core::Frame> {
    let cells = frame["bitmap"].as_array()?;
    if cells.len() != 132 {
        return None;
    }
    let colors: Vec<Rgb> = cells
        .iter()
        .map(|c| {
            let ch = |i: usize| c[i].as_u64().unwrap_or(0).min(255) as u8;
            Rgb::new(ch(0), ch(1), ch(2))
        })
        .collect();
    Some(mapping::grid(6, 22, &colors))
}

fn err(code: u8, msg: &str) -> Response {
    Response::error(400, &format!("{code}: {msg}"))
}

impl GameSense {
    pub fn load(store: Option<PathBuf>) -> Self {
        let mut gs = Self { store, ..Default::default() };
        let Some(saved) = gs.store.as_ref().and_then(|p| fs::read_to_string(p).ok()).and_then(|t| serde_json::from_str::<Value>(&t).ok())
        else {
            return gs;
        };
        for (name, g) in saved["games"].as_object().into_iter().flatten() {
            let game = gs.games.entry(name.clone()).or_default();
            game.display = g["display"].as_str().map(String::from);
            if let Some(ms) = g["timeout_ms"].as_u64() {
                game.timeout = Duration::from_millis(ms);
            }
            for (ev, e) in g["events"].as_object().into_iter().flatten() {
                let raw: Vec<Value> = e["handlers"].as_array().cloned().unwrap_or_default();
                let event = game.events.entry(ev.clone()).or_default();
                event.handlers = raw.iter().filter_map(parse_handler).collect();
                event.raw = raw;
            }
        }
        gs
    }

    fn save(&self) {
        let Some(path) = &self.store else { return };
        let games: Map<String, Value> = self
            .games
            .iter()
            .map(|(name, g)| {
                let events: Map<String, Value> = g.events.iter().map(|(n, e)| (n.clone(), json!({ "handlers": e.raw }))).collect();
                (name.clone(), json!({ "display": g.display, "timeout_ms": g.timeout.as_millis() as u64, "events": events }))
            })
            .collect();
        let text = json!({ "games": games }).to_string();
        if let Err(e) = path.parent().map(fs::create_dir_all).unwrap_or(Ok(())).and_then(|_| fs::write(path, text)) {
            eprintln!("halo: GameSense: could not save the handlers: {e}");
        }
    }

    /// Handles an endpoint. `Ok(Some(game))` if what that game paints may have changed.
    pub fn handle(&mut self, path: &str, body: &[u8], now: Instant) -> Result<Option<String>, Response> {
        let body: Value = serde_json::from_slice(body).map_err(|_| err(0, "Game or event string not specified"))?;
        if path == "/supports_multiple_game_events" {
            return Ok(None);
        }
        let game_name = body["game"].as_str().ok_or_else(|| err(1, "Game string not specified"))?;
        if !valid_name(game_name) {
            return Err(err(3, "Game string contains disallowed characters"));
        }
        let game_name = game_name.to_string();
        let event_name = || -> Result<String, Response> {
            let e = body["event"].as_str().ok_or_else(|| err(0, "Game or event string not specified"))?;
            if valid_name(e) { Ok(e.to_string()) } else { Err(err(2, "Game or event string contains disallowed characters")) }
        };
        match path {
            "/game_metadata" => {
                let g = self.games.entry(game_name).or_default();
                if let Some(d) = body["game_display_name"].as_str() {
                    g.display = Some(d.chars().take(64).collect());
                }
                if let Some(ms) = body["deinitialize_timer_length_ms"].as_u64() {
                    g.timeout = Duration::from_millis(ms.clamp(1000, 60_000));
                }
                self.save();
                Ok(None)
            }
            "/register_game_event" => {
                let ev = event_name()?;
                self.games.entry(game_name).or_default().events.entry(ev).or_default();
                self.save();
                Ok(None)
            }
            "/bind_game_event" => {
                let ev = event_name()?;
                let raw = body["handlers"].as_array().filter(|h| !h.is_empty()).ok_or_else(|| err(6, "One or more handlers must be specified for binding"))?;
                let event = self.games.entry(game_name.clone()).or_default().events.entry(ev).or_default();
                event.handlers = raw.iter().filter_map(parse_handler).collect();
                event.flashes.clear();
                event.raw = raw.clone();
                let painted = event.seq > 0;
                self.save();
                Ok(painted.then_some(game_name))
            }
            "/game_event" => {
                let ev = event_name()?;
                self.update(&game_name, ev, &body["data"], now)?;
                Ok(Some(game_name))
            }
            "/multiple_game_events" => {
                let events = body["events"].as_array().ok_or_else(|| err(4, "GameEvent data member is empty"))?;
                for e in events {
                    let ev = e["event"].as_str().filter(|s| valid_name(s)).ok_or_else(|| err(2, "Game or event string contains disallowed characters"))?;
                    self.update(&game_name, ev.to_string(), &e["data"], now)?;
                }
                Ok(Some(game_name))
            }
            "/game_heartbeat" => {
                if let Some(g) = self.games.get_mut(&game_name)
                    && g.deadline.is_some()
                {
                    g.deadline = Some(now + g.timeout);
                }
                Ok(None)
            }
            "/stop_game" => {
                if let Some(g) = self.games.get_mut(&game_name) {
                    g.deadline = None;
                }
                Ok(Some(game_name))
            }
            "/remove_game_event" => {
                let ev = event_name()?;
                let removed = self.games.get_mut(&game_name).and_then(|g| g.events.remove(&ev));
                removed.ok_or_else(|| err(9, "That event is not registered"))?;
                self.save();
                Ok(Some(game_name))
            }
            "/remove_game" => {
                self.games.remove(&game_name).ok_or_else(|| err(10, "That game is not registered"))?;
                self.save();
                Ok(Some(game_name))
            }
            // GoLisp handlers, OLED screens, etc.: accepted and do nothing.
            "/load_golisp_handlers" | "/register_game_event_icon" => Ok(None),
            _ => Err(Response::error(404, "unknown endpoint")),
        }
    }

    fn update(&mut self, game: &str, event: String, data: &Value, now: Instant) -> Result<(), Response> {
        let data = object(data).ok_or_else(|| err(4, "GameEvent data member is empty"))?;
        self.seq += 1;
        let g = self.games.entry(game.to_string()).or_default();
        g.deadline = Some(now + g.timeout);
        let e = g.events.entry(event).or_default();
        if let Some(v) = data["value"].as_i64().or_else(|| data["value"].as_f64().map(|f| f as i64)) {
            e.value = v;
        }
        if data.get("frame").is_some() {
            e.frame = data["frame"].clone();
        }
        e.seq = self.seq;
        // The cycle restarts only if the applicable flash changed: a game that repeats the same value
        // many times per second must not keep it stuck in the lit half.
        let value = e.value;
        e.flashes = e
            .handlers
            .iter()
            .enumerate()
            .map(|(i, h)| match (h.rate.as_ref().and_then(|r| resolve_rate(r, value)), e.flashes.get(i).copied().flatten()) {
                (Some((freq, limit)), Some(old)) if old.freq == freq && old.limit == limit => Some(old),
                (Some((freq, limit)), _) => Some(Flash { freq, limit, start: now }),
                (None, _) => None,
            })
            .collect();
        Ok(())
    }

    /// What the game paints at `now`; `None` if it is off or covers no zone.
    pub fn render(&self, game: &str, now: Instant) -> Option<Zones> {
        let g = self.games.get(game).filter(|g| g.deadline.is_some())?;
        let mut paints: Vec<(&Handler, &Event, Option<Flash>)> = g
            .events
            .values()
            .filter(|e| e.seq > 0)
            .flat_map(|e| e.handlers.iter().enumerate().map(move |(i, h)| (h, e, e.flashes.get(i).copied().flatten())))
            .collect();
        paints.sort_by_key(|(h, e, _)| (h.class, e.seq));
        let mut out: Zones = [None; ZONES];
        for (h, e, flash) in paints {
            let colors: Option<halo_core::Frame> = match &h.paint {
                _ if flash.is_some_and(|f| !f.lit(now)) => Some([Rgb::OFF; ZONES]),
                Paint::Color(spec) => Some([compute(spec, e.value, true); ZONES]),
                Paint::Context(key) => static_color(&e.frame[key.as_str()]).map(|c| [c; ZONES]),
                Paint::Bitmap => bitmap(&e.frame),
            };
            if let Some(colors) = colors {
                for z in 0..ZONES {
                    if h.mask[z] {
                        out[z] = Some(colors[z]);
                    }
                }
            }
        }
        out.iter().any(Option::is_some).then_some(out)
    }

    /// Turns off the games that ran out of events; returns which ones.
    pub fn expire(&mut self, now: Instant) -> Vec<String> {
        let mut out = Vec::new();
        for (name, g) in &mut self.games {
            if g.deadline.is_some_and(|d| d <= now) {
                g.deadline = None;
                out.push(name.clone());
            }
        }
        out
    }

    pub fn next_deadline(&self) -> Option<Instant> {
        self.games.values().filter_map(|g| g.deadline).min()
    }

    fn active(&self) -> impl Iterator<Item = (&String, &Game)> {
        self.games.iter().filter(|(_, g)| g.deadline.is_some())
    }

    /// The next transition of any flash in an active game.
    fn next_flash(&self, now: Instant) -> Option<Instant> {
        self.active()
            .flat_map(|(_, g)| g.events.values().filter(|e| e.seq > 0))
            .flat_map(|e| e.flashes.iter().flatten())
            .filter_map(|f| f.next(now))
            .min()
    }

    /// When the timer task must wake: a timeout or a flash transition.
    pub fn next_wake(&self, now: Instant) -> Option<Instant> {
        self.next_deadline().into_iter().chain(self.next_flash(now)).min()
    }
}

fn store_path() -> PathBuf {
    halo_core::state::path().with_file_name("gamesense.json")
}

fn layer_name(game: &str) -> String {
    format!("gamesense:{game}")
}

async fn apply(state: &GameSense, core: &Mutex<Core>, game: &str) {
    let mut core = core.lock().await;
    match state.render(game, Instant::now()) {
        Some(zones) => core.comp.set_layer(&layer_name(game), Layer { priority: PRIORITY_GAME, zones }),
        None => {
            core.comp.remove_layer(&layer_name(game));
        }
    }
    if let Err(e) = core.push() {
        eprintln!("halo: GameSense {game}: {e}");
    }
}

async fn handle_http(state: &Mutex<GameSense>, core: &Mutex<Core>, wake: &Notify, req: Request) -> Response {
    if req.origin.is_some() {
        return Response::error(403, "requests from web pages are not accepted");
    }
    if req.method != "POST" {
        return Response::error(404, "GameSense only uses POST");
    }
    let mut st = state.lock().await;
    match st.handle(&req.path, &req.body, Instant::now()) {
        Ok(changed) => {
            if let Some(game) = changed {
                apply(&st, core, &game).await;
            }
            wake.notify_one(); // let the timer recompute when to wake
            Response::ok("{}")
        }
        Err(resp) => resp,
    }
}

/// Sleeps until the next timeout or flash transition; with no active games it never wakes.
async fn timer(state: Arc<Mutex<GameSense>>, core: Arc<Mutex<Core>>, wake: Arc<Notify>) {
    loop {
        let next = state.lock().await.next_wake(Instant::now());
        match next {
            None => wake.notified().await,
            Some(t) => {
                tokio::select! {
                    _ = tokio::time::sleep_until(t.into()) => {}
                    _ = wake.notified() => continue,
                }
            }
        }
        let mut st = state.lock().await;
        for game in st.expire(Instant::now()) {
            eprintln!("halo: GameSense {game}: no events, turning off");
            apply(&st, &core, &game).await;
        }
        let blinking: Vec<String> = st.active().map(|(name, _)| name.clone()).collect();
        for game in blinking {
            apply(&st, &core, &game).await; // push does not resend if the frame did not change
        }
    }
}

pub async fn serve(core: Arc<Mutex<Core>>, port: u16, persist: bool) -> io::Result<()> {
    let listener = TcpListener::bind(("127.0.0.1", port)).await?;
    let state = Arc::new(Mutex::new(GameSense::load(persist.then(store_path))));
    let wake = Arc::new(Notify::new());
    tokio::spawn(timer(state.clone(), core.clone(), wake.clone()));
    eprintln!("halo: GameSense bridge listening on 127.0.0.1:{port}");
    loop {
        let (stream, _) = listener.accept().await?;
        let (state, core, wake) = (state.clone(), core.clone(), wake.clone());
        tokio::spawn(async move {
            let _ = http::serve(stream, false, |req| {
                let (state, core, wake) = (state.clone(), core.clone(), wake.clone());
                async move { handle_http(&state, &core, &wake, req).await }
            })
            .await;
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RED: Rgb = Rgb::new(255, 0, 0);
    const GREEN: Rgb = Rgb::new(0, 255, 0);

    fn post(gs: &mut GameSense, path: &str, body: Value, now: Instant) -> Result<Option<String>, u16> {
        gs.handle(path, body.to_string().as_bytes(), now).map_err(|r| r.status)
    }

    fn health_gradient() -> Value {
        json!({
            "game": "ADVENTURE", "event": "HEALTH",
            "handlers": [{
                "device-type": "keyboard", "zone": "function-keys", "mode": "percent",
                "color": {"gradient": {"zero": {"red": 255, "green": 0, "blue": 0}, "hundred": {"red": 0, "green": 255, "blue": 0}}}
            }]
        })
    }

    #[test]
    fn gradient_follows_value_and_times_out() {
        let mut gs = GameSense::default();
        let t0 = Instant::now();
        assert_eq!(post(&mut gs, "/bind_game_event", health_gradient(), t0), Ok(None));
        assert_eq!(gs.render("ADVENTURE", t0), None, "no events: nothing painted");
        post(&mut gs, "/game_event", json!({"game": "ADVENTURE", "event": "HEALTH", "data": {"value": 100}}), t0).unwrap();
        assert_eq!(gs.render("ADVENTURE", t0), Some([Some(GREEN); ZONES]));
        post(&mut gs, "/game_event", json!({"game": "ADVENTURE", "event": "HEALTH", "data": "{\"value\": 0}"}), t0).unwrap();
        assert_eq!(gs.render("ADVENTURE", t0), Some([Some(RED); ZONES]));
        assert_eq!(gs.next_deadline(), Some(t0 + DEFAULT_TIMEOUT));
        assert!(gs.expire(t0 + Duration::from_secs(5)).is_empty());
        assert_eq!(gs.expire(t0 + DEFAULT_TIMEOUT), vec!["ADVENTURE".to_string()]);
        assert_eq!(gs.render("ADVENTURE", t0), None);
    }

    #[test]
    fn zoned_wins_over_keyboard_and_ranges() {
        let mut gs = GameSense::default();
        let t = Instant::now();
        post(&mut gs, "/bind_game_event", health_gradient(), t).unwrap();
        post(
            &mut gs,
            "/bind_game_event",
            json!({"game": "ADVENTURE", "event": "AMMO", "handlers": [{
                "device-type": "rgb-2-zone", "zone": "two", "mode": "color",
                "color": [{"low": 0, "high": 10, "color": {"red": 255, "green": 0, "blue": 0}},
                          {"low": 11, "high": 100, "color": {"red": 0, "green": 0, "blue": 255}}]
            }]}),
            t,
        )
        .unwrap();
        post(&mut gs, "/game_event", json!({"game": "ADVENTURE", "event": "AMMO", "data": {"value": 5}}), t).unwrap();
        post(&mut gs, "/game_event", json!({"game": "ADVENTURE", "event": "HEALTH", "data": {"value": 100}}), t).unwrap();
        // Health (keyboard) paints everything, but ammo (2 zones, "two" = right) wins on the right.
        assert_eq!(gs.render("ADVENTURE", t), Some([Some(GREEN), Some(GREEN), Some(RED), Some(RED)]));
    }

    #[test]
    fn static_zero_context_and_bitmap() {
        let mut gs = GameSense::default();
        let t = Instant::now();
        post(&mut gs, "/bind_game_event", json!({"game": "G", "event": "HIT", "handlers": [
            {"device-type": "rgb-1-zone", "zone": "one", "mode": "color", "color": {"red": 255, "green": 0, "blue": 0}}]}), t).unwrap();
        post(&mut gs, "/game_event", json!({"game": "G", "event": "HIT", "data": {"value": 0}}), t).unwrap();
        assert_eq!(gs.render("G", t), Some([Some(Rgb::OFF); ZONES]), "static at value 0 = black");

        post(&mut gs, "/bind_game_event", json!({"game": "G", "event": "CTX", "handlers": [
            {"device-type": "rgb-zoned-device", "zone": "three", "mode": "context-color", "context-frame-key": "c"}]}), t).unwrap();
        post(&mut gs, "/game_event", json!({"game": "G", "event": "CTX", "data": {"frame": {"c": {"red": 0, "green": 255, "blue": 0}}}}), t).unwrap();
        assert_eq!(gs.render("G", t).unwrap()[2], Some(GREEN));

        let mut bmp = vec![json!([0, 0, 0]); 132];
        bmp[0] = json!([255, 0, 0]); // top left
        post(&mut gs, "/bind_game_event", json!({"game": "B", "event": "BMP", "handlers": [{"device-type": "rgb-per-key-zones", "mode": "bitmap"}]}), t).unwrap();
        post(&mut gs, "/game_event", json!({"game": "B", "event": "BMP", "data": {"frame": {"bitmap": bmp}}}), t).unwrap();
        assert_eq!(gs.render("B", t), Some([Some(Rgb::OFF), Some(RED), Some(Rgb::OFF), Some(Rgb::OFF)]));
    }

    #[test]
    fn errors_and_lifecycle() {
        let mut gs = GameSense::default();
        let t = Instant::now();
        assert_eq!(post(&mut gs, "/game_event", json!({"game": "lower", "event": "X", "data": {}}), t), Err(400));
        assert_eq!(post(&mut gs, "/bind_game_event", json!({"game": "G", "event": "X", "handlers": []}), t), Err(400));
        assert_eq!(post(&mut gs, "/game_event", json!({"game": "G", "event": "X"}), t), Err(400));
        assert_eq!(post(&mut gs, "/nothing", json!({"game": "G"}), t), Err(404));
        assert_eq!(gs.handle("/game_event", b"not json", t).map_err(|r| r.status), Err(400));
        post(&mut gs, "/game_event", json!({"game": "G", "event": "X", "data": {"value": 1}}), t).unwrap();
        post(&mut gs, "/stop_game", json!({"game": "G"}), t).unwrap();
        assert_eq!(gs.next_deadline(), None);
        assert_eq!(post(&mut gs, "/remove_game", json!({"game": "G"}), t), Ok(Some("G".into())));
        assert_eq!(post(&mut gs, "/remove_game", json!({"game": "G"}), t), Err(400));
    }

    fn ms(t: Instant, n: u64) -> Instant {
        t + Duration::from_millis(n)
    }

    #[test]
    fn flash_with_repeat_limit() {
        let mut gs = GameSense::default();
        let t = Instant::now();
        // 2 Hz: 250 ms on, 250 ms black; twice, then it stays on.
        post(&mut gs, "/bind_game_event", json!({"game": "G", "event": "HIT", "handlers": [{
            "device-type": "rgb-1-zone", "zone": "one", "color": {"red": 255, "green": 0, "blue": 0},
            "rate": {"frequency": 2, "repeat_limit": 2}}]}), t).unwrap();
        post(&mut gs, "/game_event", json!({"game": "G", "event": "HIT", "data": {"value": 1}}), t).unwrap();
        let on = Some([Some(RED); ZONES]);
        let off = Some([Some(Rgb::OFF); ZONES]);
        assert_eq!(gs.render("G", ms(t, 100)), on);
        assert_eq!(gs.render("G", ms(t, 300)), off);
        assert_eq!(gs.render("G", ms(t, 600)), on);
        assert_eq!(gs.render("G", ms(t, 800)), off);
        assert_eq!(gs.render("G", ms(t, 1100)), on, "done: stays on");
        assert_eq!(gs.next_flash(ms(t, 100)), Some(ms(t, 251)));
        assert_eq!(gs.next_flash(ms(t, 800)), Some(ms(t, 1001)), "the last transition is the end");
        assert_eq!(gs.next_flash(ms(t, 1100)), None, "no flash: no need to wake");
        assert_eq!(gs.next_wake(ms(t, 1100)), Some(t + DEFAULT_TIMEOUT));

        // The same value again does not restart the cycle.
        post(&mut gs, "/game_event", json!({"game": "G", "event": "HIT", "data": {"value": 1}}), ms(t, 300)).unwrap();
        assert_eq!(gs.render("G", ms(t, 1100)), on);
    }

    #[test]
    fn flash_frequency_by_range() {
        let mut gs = GameSense::default();
        let t = Instant::now();
        post(&mut gs, "/bind_game_event", json!({"game": "G", "event": "HEALTH", "handlers": [{
            "device-type": "rgb-2-zone", "zone": "one", "color": {"red": 0, "green": 255, "blue": 0},
            "rate": {"frequency": [{"low": 1, "high": 10, "frequency": 10}, {"low": 11, "high": 20, "frequency": 5}]}}]}), t).unwrap();
        post(&mut gs, "/game_event", json!({"game": "G", "event": "HEALTH", "data": {"value": 50}}), t).unwrap();
        assert_eq!(gs.next_flash(t), None, "outside the ranges it does not flash");
        post(&mut gs, "/game_event", json!({"game": "G", "event": "HEALTH", "data": {"value": 15}}), t).unwrap();
        assert_eq!(gs.render("G", ms(t, 50)).unwrap()[0], Some(GREEN));
        assert_eq!(gs.render("G", ms(t, 150)).unwrap()[0], Some(Rgb::OFF), "5 Hz: black from 100 to 200 ms");
        // Moving to another range restarts the cycle with the new frequency.
        post(&mut gs, "/game_event", json!({"game": "G", "event": "HEALTH", "data": {"value": 5}}), ms(t, 150)).unwrap();
        assert_eq!(gs.render("G", ms(t, 160)).unwrap()[0], Some(GREEN));
        assert_eq!(gs.render("G", ms(t, 210)).unwrap()[0], Some(Rgb::OFF), "10 Hz: black from 50 to 100 ms");
        assert_eq!(gs.render("G", ms(t, 210)).unwrap()[2], None, "the right ring is not its");
    }

    #[test]
    fn handlers_survive_restart() {
        let path = std::env::temp_dir().join(format!("halo-gs-{}.json", std::process::id()));
        let _ = fs::remove_file(&path);
        let t = Instant::now();
        let mut gs = GameSense::load(Some(path.clone()));
        post(&mut gs, "/bind_game_event", health_gradient(), t).unwrap();
        post(&mut gs, "/game_metadata", json!({"game": "ADVENTURE", "deinitialize_timer_length_ms": 5000}), t).unwrap();
        let mut again = GameSense::load(Some(path.clone()));
        post(&mut again, "/game_event", json!({"game": "ADVENTURE", "event": "HEALTH", "data": {"value": 100}}), t).unwrap();
        assert_eq!(again.render("ADVENTURE", t), Some([Some(GREEN); ZONES]));
        assert_eq!(again.next_deadline(), Some(t + Duration::from_secs(5)));
        let _ = fs::remove_file(path);
    }
}
