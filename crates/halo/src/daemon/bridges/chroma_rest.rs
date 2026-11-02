//! Razer Chroma over REST (`http://localhost:54235/razer/chromasdk`): used mostly by web apps and
//! some games. Reference: assets.razerzone.com/dev_portal/REST/html.
//!
//! The JSON is translated into the same `(device, effect type, structs)` the DLL sends, so the
//! mapping to the 4 zones is the one in `chroma.rs`. Each session is a game layer. It ends with its
//! `DELETE` or after 15 s without requests (the `heartbeat` keeps it alive), as in Synapse.
//!
//! Unlike GameSense, web pages are accepted here (open CORS): that is what Razer made this API for.
//! Whatever they paint sits under the `halo games` switch, like any game.

use super::chroma::{self, CHROMALINK, HEADSET, KEYBOARD, KEYPAD, MOUSE, MOUSEPAD};
use super::http::{self, Request, Response};
use crate::daemon::compositor::{Layer, PRIORITY_GAME};
use crate::daemon::device::Core;
use halo_core::Frame;
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap};
use std::io;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::net::TcpListener;
use tokio::sync::{Mutex, Notify};

pub const PORT: u16 = 54235;
const TIMEOUT: Duration = Duration::from_secs(15);
const MAX_SESSIONS: usize = 32;
const MAX_EFFECTS: usize = 4096;
/// `RzErrors.h`.
const SUCCESS: i64 = 0;
const NOT_SUPPORTED: i64 = 50;
const INVALID_PARAMETER: i64 = 87;
const NOT_FOUND: i64 = 1168;
/// A validly named effect that is not painted (breathing, wave...): accepted and ignored, as in the DLL.
const IGNORED: u32 = u32::MAX;

#[derive(Clone, Debug, PartialEq)]
struct Effect {
    device: u8,
    kind: u32,
    words: Vec<u32>,
}

struct Session {
    layer: String,
    chroma: chroma::Session,
    effects: HashMap<String, Effect>,
    deadline: Instant,
    ticks: u64,
}

/// A layer to set (`Some`) or remove (`None`).
pub type Change = (String, Option<Frame>);

#[derive(Default)]
pub struct ChromaRest {
    port: u16,
    sessions: BTreeMap<u32, Session>,
    next_sid: u32,
    next_effect: u64,
}

fn device(name: &str) -> Option<u8> {
    Some(match name {
        "keyboard" => KEYBOARD,
        "mouse" => MOUSE,
        "headset" => HEADSET,
        "mousepad" => MOUSEPAD,
        "keypad" => KEYPAD,
        "chromalink" => CHROMALINK,
        _ => return None,
    })
}

/// The value of Razer's effect enum for that device (from `RzChromaSDKTypes.h`).
fn kind(device: u8, name: &str) -> Option<u32> {
    Some(match (device, name) {
        (_, "CHROMA_NONE") => 0,
        (CHROMALINK, "CHROMA_CUSTOM") => 1,
        (CHROMALINK, "CHROMA_STATIC") => 2,
        (KEYBOARD, "CHROMA_CUSTOM") => 2,
        (KEYBOARD, "CHROMA_STATIC") => 4,
        (KEYBOARD, "CHROMA_CUSTOM_KEY") => 8,
        (KEYBOARD, "CHROMA_CUSTOM2") => 9,
        (MOUSE, "CHROMA_CUSTOM") => 3,
        (MOUSE, "CHROMA_STATIC") => 6,
        (MOUSE, "CHROMA_CUSTOM2") => 8,
        (HEADSET, "CHROMA_STATIC") => 1,
        (HEADSET, "CHROMA_CUSTOM") => 4,
        (MOUSEPAD, "CHROMA_CUSTOM") => 2,
        (MOUSEPAD, "CHROMA_STATIC") => 4,
        (MOUSEPAD, "CHROMA_CUSTOM2") => 6,
        (KEYPAD, "CHROMA_CUSTOM") => 2,
        (KEYPAD, "CHROMA_STATIC") => 5,
        (_, n) if n.starts_with("CHROMA_") => IGNORED,
        _ => return None,
    })
}

/// Numbers or arrays of numbers (of any depth), in order.
fn flatten(v: &Value, out: &mut Vec<u32>) -> Option<()> {
    match v {
        Value::Number(n) => out.push(n.as_u64()? as u32),
        Value::Array(a) => a.iter().try_for_each(|x| flatten(x, out))?,
        _ => return None,
    }
    Some(())
}

/// An effect's JSON to what the DLL sends. If the length is not what `chroma::decode` expects, it
/// is discarded there.
fn parse_effect(device: u8, v: &Value) -> Option<Effect> {
    let name = v["effect"].as_str()?;
    let mut kind = kind(device, name)?;
    let p = &v["param"];
    let mut words = Vec::new();
    match (device, name) {
        _ if kind == 0 || kind == IGNORED => {}
        (_, "CHROMA_STATIC") => {
            let color = p["color"].as_u64()? as u32;
            words = if device == MOUSE { vec![0, color] } else { vec![color] }; // { LEDId; Color }
        }
        (KEYBOARD, "CHROMA_CUSTOM_KEY") => {
            flatten(&p["color"], &mut words)?;
            flatten(&p["key"], &mut words)?;
        }
        (KEYBOARD, "CHROMA_CUSTOM2") => {
            // The C struct has Color[8][24] and Key[6][22]; the mapping only uses Color.
            flatten(&p["color"], &mut words)?;
            words.resize(192 + 132, 0);
        }
        _ => flatten(p, &mut words)?,
    }
    if device == MOUSEPAD && kind == 2 && words.len() == 20 {
        kind = 6; // the REST docs call the 20-LED one CHROMA_CUSTOM (CUSTOM2 in C)
    }
    Some(Effect { device, kind, words })
}

fn reply(v: Value) -> Response {
    Response::ok(v.to_string())
}

fn result(code: i64) -> Response {
    reply(json!({ "result": code }))
}

impl ChromaRest {
    pub fn new(port: u16) -> Self {
        Self { port, ..Default::default() }
    }

    /// Handles a request. Returns the response and, if it changed, the session's layer.
    pub fn handle(&mut self, method: &str, path: &str, body: &[u8], now: Instant) -> (Response, Option<Change>) {
        let path = path.trim_end_matches('/');
        if method == "OPTIONS" {
            return (Response::ok(""), None); // CORS preflight
        }
        let body: Value = if body.is_empty() { Value::Null } else { serde_json::from_slice(body).unwrap_or(Value::Null) };
        if path == "/razer/chromasdk" {
            return match method {
                "GET" => (reply(json!({ "core": "3.36.0", "device": "3.36.0", "version": "3.36.0" })), None),
                "POST" => (self.init(&body, now), None),
                _ => (result(NOT_SUPPORTED), None),
            };
        }
        let Some((sid, tail)) = path.strip_prefix("/chromasdk/").map(|r| r.split_once('/').unwrap_or((r, "")))
        else {
            return (Response { status: 404, body: json!({ "result": NOT_FOUND }).to_string() }, None);
        };
        let Some(sid) = sid.parse::<u32>().ok().filter(|s| self.sessions.contains_key(s)) else {
            return (Response { status: 404, body: json!({ "result": NOT_FOUND }).to_string() }, None);
        };
        if tail.is_empty() && method == "DELETE" {
            let s = self.sessions.remove(&sid).unwrap();
            eprintln!("halo: {} ended", s.layer);
            return (result(SUCCESS), Some((s.layer, None)));
        }
        self.next_effect += 1;
        let next_effect = self.next_effect;
        let s = self.sessions.get_mut(&sid).unwrap();
        s.deadline = now + TIMEOUT;
        let before = s.chroma.output();
        let resp = match (method, tail) {
            ("PUT", "heartbeat") => {
                s.ticks += 1;
                reply(json!({ "tick": s.ticks }))
            }
            ("PUT" | "DELETE", "effect") => {
                let single = body.get("id").is_some();
                let ids: Vec<&str> = match &body["ids"] {
                    Value::Array(a) => a.iter().filter_map(Value::as_str).collect(),
                    _ => body["id"].as_str().into_iter().collect(),
                };
                let results: Vec<i64> = ids
                    .iter()
                    .map(|id| {
                        let effect = if method == "PUT" { s.effects.get(*id).cloned() } else { s.effects.remove(*id) };
                        match effect {
                            Some(e) if method == "PUT" => {
                                s.chroma.apply(e.device, e.kind, &words_to_bytes(&e.words));
                                SUCCESS
                            }
                            Some(_) => SUCCESS,
                            None => NOT_FOUND,
                        }
                    })
                    .collect();
                if single {
                    result(results.first().copied().unwrap_or(INVALID_PARAMETER))
                } else {
                    reply(json!({ "results": results.iter().map(|r| json!({ "result": r })).collect::<Vec<_>>() }))
                }
            }
            ("PUT" | "POST", dev) => match device(dev) {
                None => result(NOT_SUPPORTED), // /devid=... and anything we do not know
                Some(dev) => {
                    let mut store = |e: Effect, n: u64| -> Value {
                        if s.effects.len() >= MAX_EFFECTS {
                            return json!({ "result": INVALID_PARAMETER });
                        }
                        let id = format!("{sid:08X}-0000-4000-8000-{n:012X}");
                        s.effects.insert(id.clone(), e);
                        json!({ "result": SUCCESS, "id": id })
                    };
                    if let (Some(list), "POST") = (body["effects"].as_array(), method) {
                        let results: Vec<Value> = list
                            .iter()
                            .enumerate()
                            .map(|(i, v)| match parse_effect(dev, v) {
                                Some(e) => store(e, next_effect << 12 | i as u64),
                                None => json!({ "result": INVALID_PARAMETER }),
                            })
                            .collect();
                        reply(json!({ "results": results }))
                    } else {
                        match (parse_effect(dev, &body), method) {
                            (None, _) => result(INVALID_PARAMETER),
                            (Some(e), "POST") => reply(store(e, next_effect << 12)),
                            (Some(e), _) => {
                                s.chroma.apply(e.device, e.kind, &words_to_bytes(&e.words));
                                result(SUCCESS)
                            }
                        }
                    }
                }
            },
            _ => result(NOT_SUPPORTED),
        };
        let s = &self.sessions[&sid];
        let after = s.chroma.output();
        (resp, (after != before).then(|| (s.layer.clone(), after)))
    }

    fn init(&mut self, body: &Value, now: Instant) -> Response {
        if self.sessions.len() >= MAX_SESSIONS {
            return result(INVALID_PARAMETER);
        }
        let title: String = body["title"].as_str().unwrap_or("REST").chars().filter(|c| !c.is_control()).take(64).collect();
        self.next_sid += 1;
        let sid = self.next_sid;
        let layer = format!("chroma:{title}#rest{sid}");
        eprintln!("halo: {layer} connected");
        self.sessions.insert(
            sid,
            Session { layer, chroma: chroma::Session::default(), effects: HashMap::new(), deadline: now + TIMEOUT, ticks: 0 },
        );
        reply(json!({ "sessionid": sid, "uri": format!("http://localhost:{}/chromasdk/{sid}", self.port) }))
    }

    /// Closes the sessions with no requests in 15 s; returns their layers.
    pub fn expire(&mut self, now: Instant) -> Vec<String> {
        let dead: Vec<u32> = self.sessions.iter().filter(|(_, s)| s.deadline <= now).map(|(id, _)| *id).collect();
        dead.into_iter().filter_map(|id| self.sessions.remove(&id)).map(|s| s.layer).collect()
    }

    pub fn next_deadline(&self) -> Option<Instant> {
        self.sessions.values().map(|s| s.deadline).min()
    }
}

fn words_to_bytes(words: &[u32]) -> Vec<u8> {
    words.iter().flat_map(|w| w.to_le_bytes()).collect()
}

async fn apply(core: &Mutex<Core>, (layer, frame): Change) {
    let mut core = core.lock().await;
    match frame {
        Some(f) => core.comp.set_layer(&layer, Layer { priority: PRIORITY_GAME, zones: f.map(Some) }),
        None => {
            if !core.comp.remove_layer(&layer) {
                return;
            }
        }
    }
    if let Err(e) = core.push() {
        eprintln!("halo: {layer}: {e}");
    }
}

async fn handle_http(state: &Mutex<ChromaRest>, core: &Mutex<Core>, wake: &Notify, req: Request) -> Response {
    let (resp, change) = state.lock().await.handle(&req.method, &req.path, &req.body, Instant::now());
    if let Some(change) = change {
        apply(core, change).await;
    }
    wake.notify_one();
    resp
}

/// Sleeps until the nearest session expires; with no sessions it never wakes.
async fn timer(state: Arc<Mutex<ChromaRest>>, core: Arc<Mutex<Core>>, wake: Arc<Notify>) {
    loop {
        let next = state.lock().await.next_deadline();
        match next {
            None => wake.notified().await,
            Some(t) => {
                tokio::select! {
                    _ = tokio::time::sleep_until(t.into()) => {}
                    _ = wake.notified() => continue,
                }
            }
        }
        let dead = state.lock().await.expire(Instant::now());
        for layer in dead {
            eprintln!("halo: {layer}: no heartbeat, closing");
            apply(&core, (layer, None)).await;
        }
    }
}

pub async fn serve(core: Arc<Mutex<Core>>, port: u16) -> io::Result<()> {
    let listener = TcpListener::bind(("127.0.0.1", port)).await?;
    let state = Arc::new(Mutex::new(ChromaRest::new(port)));
    let wake = Arc::new(Notify::new());
    tokio::spawn(timer(state.clone(), core.clone(), wake.clone()));
    eprintln!("halo: Chroma REST bridge listening on 127.0.0.1:{port}");
    loop {
        let (stream, _) = listener.accept().await?;
        let (state, core, wake) = (state.clone(), core.clone(), wake.clone());
        tokio::spawn(async move {
            let _ = http::serve(stream, true, |req| {
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
    use halo_core::Rgb;

    const RED: Rgb = Rgb::new(255, 0, 0);
    const BLUE: Rgb = Rgb::new(0, 0, 255);

    fn call(r: &mut ChromaRest, method: &str, path: &str, body: Value, now: Instant) -> (Value, Option<Change>) {
        let (resp, change) = r.handle(method, path, body.to_string().as_bytes(), now);
        (serde_json::from_str(&resp.body).unwrap_or(Value::Null), change)
    }

    fn session(r: &mut ChromaRest, t: Instant) -> String {
        let (v, _) = call(r, "POST", "/razer/chromasdk", json!({"title": "Web", "device_supported": ["chromalink"]}), t);
        let uri = v["uri"].as_str().unwrap();
        assert!(uri.starts_with("http://localhost:54235/chromasdk/"), "{uri}");
        uri.trim_start_matches("http://localhost:54235").to_string()
    }

    #[test]
    fn put_paints_and_delete_ends() {
        let mut r = ChromaRest::new(PORT);
        let t = Instant::now();
        let base = session(&mut r, t);
        // ChromaLink: 0 is the background, 1-4 the zones. COLORREF 0x00BBGGRR: 255 = red.
        let (v, change) = call(&mut r, "PUT", &format!("{base}/chromalink"), json!({"effect": "CHROMA_CUSTOM", "param": [0, 255, 255, 16711680, 16711680]}), t);
        assert_eq!(v["result"], 0);
        assert_eq!(change, Some(("chroma:Web#rest1".into(), Some([RED, RED, BLUE, BLUE]))));
        let (_, change) = call(&mut r, "PUT", &format!("{base}/chromalink"), json!({"effect": "CHROMA_CUSTOM", "param": [0, 255, 255, 16711680, 16711680]}), t);
        assert_eq!(change, None, "same effect: nothing to repaint");
        let (v, change) = call(&mut r, "DELETE", &base, Value::Null, t);
        assert_eq!(v["result"], 0);
        assert_eq!(change, Some(("chroma:Web#rest1".into(), None)));
        let (v, _) = call(&mut r, "PUT", &format!("{base}/heartbeat"), Value::Null, t);
        assert_eq!(v["result"], NOT_FOUND);
    }

    #[test]
    fn post_creates_and_effect_applies() {
        let mut r = ChromaRest::new(PORT);
        let t = Instant::now();
        let base = session(&mut r, t);
        let (v, change) = call(&mut r, "POST", &format!("{base}/keyboard"), json!({"effect": "CHROMA_STATIC", "param": {"color": 16711680}}), t);
        assert_eq!(change, None, "POST only creates");
        let id = v["id"].as_str().unwrap().to_string();
        let (v, change) = call(&mut r, "PUT", &format!("{base}/effect"), json!({"id": id}), t);
        assert_eq!(v["result"], 0);
        assert_eq!(change.unwrap().1, Some([BLUE; 4]));
        let (v, _) = call(&mut r, "DELETE", &format!("{base}/effect"), json!({"ids": [id, "MISSING"]}), t);
        assert_eq!(v["results"], json!([{"result": 0}, {"result": NOT_FOUND}]));

        // Several at once, with one effect that is not painted and one invalid.
        let (v, _) = call(&mut r, "POST", &format!("{base}/mouse"), json!({"effects": [
            {"effect": "CHROMA_STATIC", "param": {"color": 255}}, {"effect": "CHROMA_BREATHING"}, {"effect": "NOTHING"}]}), t);
        let results = v["results"].as_array().unwrap();
        assert_eq!((results[0]["result"].as_i64(), results[1]["result"].as_i64(), results[2]["result"].as_i64()), (Some(0), Some(0), Some(INVALID_PARAMETER)));
        assert_ne!(results[0]["id"], results[1]["id"]);
        let (_, change) = call(&mut r, "PUT", &format!("{base}/effect"), json!({"ids": [results[0]["id"], results[1]["id"]]}), t);
        // Deleting the keyboard id does not clear what it painted: it still wins over the mouse (and
        // breathing is ignored), so there is nothing to repaint. With CHROMA_NONE the keyboard goes off
        // and the mouse shows.
        assert_eq!(change, None);
        let (_, change) = call(&mut r, "PUT", &format!("{base}/keyboard"), json!({"effect": "CHROMA_NONE"}), t);
        assert_eq!(change.unwrap().1, Some([RED; 4]));
    }

    #[test]
    fn keyboard_grid_and_mousepad_alias() {
        let mut r = ChromaRest::new(PORT);
        let t = Instant::now();
        let base = session(&mut r, t);
        let mut rows = vec![vec![0u32; 22]; 6];
        rows[0][0] = 255; // top left
        let (_, change) = call(&mut r, "PUT", &format!("{base}/keyboard"), json!({"effect": "CHROMA_CUSTOM", "param": rows}), t);
        assert_eq!(change.unwrap().1, Some([Rgb::OFF, RED, Rgb::OFF, Rgb::OFF]));
        assert_eq!(parse_effect(MOUSEPAD, &json!({"effect": "CHROMA_CUSTOM", "param": vec![255; 20]})).unwrap().kind, 6);
        assert_eq!(parse_effect(MOUSEPAD, &json!({"effect": "CHROMA_CUSTOM", "param": vec![255; 15]})).unwrap().kind, 2);
        assert_eq!(parse_effect(KEYBOARD, &json!({"effect": "CHROMA_STATIC", "param": {}})), None);
    }

    #[test]
    fn heartbeat_keeps_alive_and_timeout_ends() {
        let mut r = ChromaRest::new(PORT);
        let t = Instant::now();
        let base = session(&mut r, t);
        let (v, _) = call(&mut r, "GET", "/razer/chromasdk", Value::Null, t);
        assert!(v["version"].is_string());
        let later = t + Duration::from_secs(10);
        assert_eq!(call(&mut r, "PUT", &format!("{base}/heartbeat"), Value::Null, later).0["tick"], 1);
        assert!(r.expire(t + TIMEOUT).is_empty(), "the heartbeat extended it");
        assert_eq!(r.expire(later + TIMEOUT), vec!["chroma:Web#rest1".to_string()]);
        assert_eq!(r.next_deadline(), None);
        assert_eq!(r.handle("OPTIONS", &base, b"", t).0.status, 200);
        assert_eq!(r.handle("PUT", "/chromasdk/99/heartbeat", b"", t).0.status, 404);
    }
}
