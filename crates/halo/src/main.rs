//! `halo`: the lights of the ROG Xbox Ally's joystick rings.
//!
//! If the daemon (`halo daemon`) is running, the CLI talks to it over D-Bus; otherwise it writes
//! to the hardware directly.

mod daemon;
mod proton;
mod scan;

use daemon::dbus::{NAME, PATH};
use halo_core::backend::{self, Backend, Dummy, LampArray, Vendor};
use halo_core::discover::{self, Interface};
use halo_core::state::{self, State};
use halo_core::{Rgb, ZONES, color, tr};
use std::path::Path;
use std::process::{Command, ExitCode};
use std::{fs, io};
use zbus::Connection;

fn main() -> ExitCode {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let mut backend_name = None;
    if let Some(i) = args.iter().position(|a| a == "--backend") {
        if i + 1 >= args.len() {
            return fail(&tr!("--backend needs a value"));
        }
        backend_name = Some(args.remove(i + 1));
        args.remove(i);
    }
    if args.first().map(String::as_str) != Some("daemon") {
        // Like any terminal tool: `halo status | head` ends quietly instead of panicking. The
        // daemon keeps SIGPIPE ignored (clients that hang up sockets).
        // SAFETY: at the start of main, with no other threads yet.
        unsafe { libc::signal(libc::SIGPIPE, libc::SIG_DFL) };
    }
    let rt = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
        Ok(rt) => rt,
        Err(e) => return fail(&tr!("Could not create the runtime: {error}", error = e)),
    };
    match rt.block_on(run(backend_name, &args)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => fail(&e),
    }
}

fn fail(msg: &str) -> ExitCode {
    eprintln!("halo: {msg}");
    ExitCode::FAILURE
}

fn usage() -> String {
    tr!("usage")
}

fn needs_daemon() -> String {
    tr!("This needs the daemon: systemctl --user start halo")
}

async fn run(backend_name: Option<String>, args: &[String]) -> Result<(), String> {
    let Some((cmd, rest)) = args.split_first() else {
        println!("{}", usage());
        return Ok(());
    };
    let direct = backend_name.is_some();
    let backend_name = backend_name.unwrap_or_else(|| "auto".into());
    let daemon = if direct || cmd == "daemon" { None } else { daemon_connection().await };

    match (cmd.as_str(), rest) {
        ("daemon", []) => daemon::run(&backend_name).await,
        ("set", colors) if !colors.is_empty() => {
            let frame = parse_frame(colors)?;
            match &daemon {
                Some(c) => call(c, "SetZones", &(hex(&frame),)).await,
                None => set(&backend_name, frame, None),
            }
        }
        ("pick", []) => {
            let c = pick(&daemon).await?;
            match &daemon {
                Some(conn) => call(conn, "SetZones", &(vec![c.to_string()],)).await,
                None => set(&backend_name, [c; ZONES], None),
            }
        }
        ("brightness", [n]) => {
            let n = n.parse::<u8>().ok().filter(|n| *n <= 3).ok_or_else(|| tr!("Brightness goes from 0 to 3"))?;
            match &daemon {
                Some(c) => call(c, "SetBrightness", &(n,)).await,
                None => set(&backend_name, current().frame, Some(n)),
            }
        }
        ("restore", []) => match &daemon {
            Some(c) => call(c, "Restore", &()).await,
            None => restore(&backend_name),
        },
        ("release", []) => match &daemon {
            Some(c) => call(c, "Release", &()).await,
            None => open(&backend_name)?.release().map_err(|e| e.to_string()),
        }
        .map(|_| println!("{}", tr!("Control handed back to the firmware until the next change"))),
        ("layer", [name, prio, colors @ ..]) if !colors.is_empty() => {
            let prio: i32 = prio.parse().map_err(|_| tr!("Invalid priority: {value}", value = prio))?;
            let c = daemon.as_ref().ok_or_else(needs_daemon)?;
            call(c, "SetLayer", &(name, prio, colors.to_vec())).await
        }
        ("unlayer", [name]) => {
            let c = daemon.as_ref().ok_or_else(needs_daemon)?;
            call(c, "ClearLayer", &(name,)).await
        }
        ("effect", [sub, name, values @ ..]) if sub == "colors" && !values.is_empty() => {
            let c = daemon.as_ref().ok_or_else(needs_daemon)?;
            let value = values.join(" ");
            set_config(c, &format!("colors_{name}"), &value).await?;
            if value == "auto" {
                println!("{}", tr!("{effect}: default colors", effect = name));
            } else {
                println!("{}", tr!("{effect}: colors {colors}", effect = name, colors = value));
            }
            Ok(())
        }
        ("effect", [sub, name, dir]) if sub == "direction" => {
            let c = daemon.as_ref().ok_or_else(needs_daemon)?;
            set_config(c, &format!("reverse_{name}"), dir).await?;
            println!("{}", tr!("{effect}: direction {direction}", effect = name, direction = dir));
            Ok(())
        }
        ("effect", args) if args.len() <= 2 => {
            let c = daemon.as_ref().ok_or_else(needs_daemon)?;
            let (mut name, mut speed) = (None, None);
            for a in args {
                if daemon::effects::anim::Speed::parse(a).is_some() {
                    speed = Some(a.as_str());
                } else {
                    name = Some(a.as_str());
                }
            }
            if let Some(v) = speed {
                call(c, "SetEffectSpeed", &(v,)).await?;
            }
            match name {
                Some("off" | "none") => {
                    call(c, "StopEffect", &()).await?;
                }
                Some(n) => call(c, "StartEffect", &(n,)).await?,
                None => {}
            }
            let now = daemon_prop::<String>(c, "Effect").await.map_err(|e| e.to_string())?;
            let speed = daemon_prop::<String>(c, "EffectSpeed").await.unwrap_or_default();
            let hw = daemon_prop::<bool>(c, "EffectHardware").await.unwrap_or(false);
            let all = daemon_prop::<Vec<String>>(c, "Effects").await.unwrap_or_default();
            let mcu = if hw { tr!(" · animated by the MCU") } else { String::new() };
            match now.as_str() {
                "" => println!("{}", tr!("effect: none · speed {speed}", speed = speed)),
                n => println!("{}", tr!("effect: {effect} · speed {speed}{mcu}", effect = n, speed = speed, mcu = mcu)),
            }
            if args.is_empty() {
                println!("{}", tr!("available: {list}", list = all.join(", ")));
            }
            Ok(())
        }
        ("eq", []) => {
            let c = daemon.as_ref().ok_or_else(needs_daemon)?;
            let on = daemon_prop::<String>(c, "Effect").await.map_err(|e| e.to_string())? == daemon::effects::EQUALIZER;
            if on {
                call(c, "StopEffect", &()).await?;
                notify(&tr!("Equalizer off"));
            } else {
                call(c, "StartEffect", &(daemon::effects::EQUALIZER,)).await?;
                notify(&tr!("Equalizer on (run again to turn it off)"));
            }
            Ok(())
        }
        ("proton", []) | ("proton", [_]) if rest.first().is_none_or(|a| a == "list") => {
            proton_list();
            Ok(())
        }
        ("proton", [scan]) if scan == "scan" => proton_scan(None),
        ("proton", [scan, appid]) if scan == "scan" => proton_scan(Some(appid)),
        ("proton", [action, target]) if action == "install" || action == "remove" => proton_apply(action, target),
        ("games", [action, game @ ..]) if (action == "block" || action == "allow") && !game.is_empty() => {
            let c = daemon.as_ref().ok_or_else(needs_daemon)?;
            let game = game.join(" ");
            let allowed = action == "allow";
            c.call_method(Some(NAME), PATH, Some(NAME), "SetGameAllowed", &(game.as_str(), allowed)).await.map_err(daemon_error)?;
            if allowed {
                println!("{}", tr!("{game}: may paint the rings", game = game));
            } else {
                println!("{}", tr!("{game}: blocked", game = game));
            }
            Ok(())
        }
        ("games", args) if args.len() <= 1 => {
            let c = daemon.as_ref().ok_or_else(needs_daemon)?;
            match args.first().map(String::as_str) {
                Some("on") => call(c, "SetGamesEnabled", &(true,)).await?,
                Some("off") => call(c, "SetGamesEnabled", &(false,)).await?,
                Some(other) => return Err(tr!("Use on or off, not {value}", value = other)),
                None => {}
            }
            let on = daemon_prop::<bool>(c, "GamesEnabled").await.map_err(|e| e.to_string())?;
            if on {
                println!("{}", tr!("Games may control the lights"));
            } else {
                println!("{}", tr!("Games may NOT control the lights"));
            }
            if args.is_empty() {
                let games = daemon_prop::<Vec<(String, String, bool, bool)>>(c, "Games").await.unwrap_or_default();
                if games.is_empty() {
                    println!("{}", tr!("No game connected or blocked"));
                }
                for (game, bridge, connected, allowed) in games {
                    let state = match (connected, allowed) {
                        (true, true) => tr!("connected through {bridge}, painting", bridge = bridge),
                        (true, false) => tr!("connected through {bridge}, BLOCKED", bridge = bridge),
                        _ => tr!("blocked"),
                    };
                    println!("  {game}: {state}");
                }
            }
            Ok(())
        }
        ("profile", []) => {
            let c = daemon.as_ref().ok_or_else(needs_daemon)?;
            let (appid, name) = daemon_prop::<(String, String)>(c, "CurrentGame").await.map_err(|e| e.to_string())?;
            let profiles = daemon_prop::<Vec<(String, String, String)>>(c, "Profiles").await.unwrap_or_default();
            match appid.as_str() {
                "" => println!("{}", tr!("Running game: none")),
                a if profiles.iter().any(|(p, ..)| p == a) => println!("{}", tr!("Running game: {name} ({appid}) · with a profile", name = name, appid = a)),
                a => println!("{}", tr!("Running game: {name} ({appid}) · no profile", name = name, appid = a)),
            }
            if profiles.is_empty() {
                println!("{}", tr!("No profiles; e.g. `halo profile 427520 tide` (`halo proton list` shows the AppIDs)"));
            }
            for (appid, name, value) in profiles {
                println!("  {appid:>10}  {name:<30} {value}");
            }
            Ok(())
        }
        ("profile", [appid, value @ ..]) if !value.is_empty() => {
            let c = daemon.as_ref().ok_or_else(needs_daemon)?;
            let value = value.join(" ");
            set_config(c, &format!("profile_{appid}"), &value).await?;
            match value.as_str() {
                "remove" | "none" => println!("{}", tr!("{appid}: no profile", appid = appid)),
                v => println!("{}", tr!("{appid}: when it opens, {value}", appid = appid, value = v)),
            }
            Ok(())
        }
        ("config", []) => {
            let entries = match &daemon {
                Some(c) => {
                    let reply = c.call_method(Some(NAME), PATH, Some(NAME), "Config", &()).await.map_err(|e| e.to_string())?;
                    reply.body().deserialize::<Vec<(String, String)>>().map_err(|e| e.to_string())?
                }
                None => daemon::config::entries(&daemon::config::load()),
            };
            for (k, v) in entries {
                println!("{k:<20} {v}");
            }
            println!("{}", tr!("file: {path}", path = daemon::config::path().display()));
            Ok(())
        }
        ("config", [key, value]) => match &daemon {
            Some(c) => set_config(c, key, value).await.map(|_| println!("{key} = {value}")),
            None => {
                let mut cfg = daemon::config::load();
                daemon::config::set(&mut cfg, key, value)?;
                daemon::config::save(&cfg).map_err(|e| e.to_string())?;
                println!("{}", tr!("{key} = {value} (the daemon is not running; it applies at start)", key = key, value = value));
                Ok(())
            }
        },
        ("status", []) => {
            status(&daemon).await;
            Ok(())
        }
        ("-h" | "--help" | "help", _) => {
            println!("{}", usage());
            Ok(())
        }
        _ => Err(usage()),
    }
}

// ---- the daemon over D-Bus ----

/// The session connection if the daemon owns its name; otherwise `None` and we write directly.
async fn daemon_connection() -> Option<Connection> {
    let conn = Connection::session().await.ok()?;
    let dbus = zbus::fdo::DBusProxy::new(&conn).await.ok()?;
    let name = zbus::names::BusName::try_from(NAME).ok()?;
    dbus.name_has_owner(name).await.ok()?.then_some(conn)
}

async fn call<B>(conn: &Connection, method: &str, body: &B) -> Result<(), String>
where
    B: serde::Serialize + zbus::zvariant::DynamicType,
{
    conn.call_method(Some(NAME), PATH, Some(NAME), method, body).await.map(|_| ()).map_err(daemon_error)?;
    if let Ok(frame) = daemon_prop::<Vec<String>>(conn, "Frame").await {
        println!("daemon: {}", frame.join(" "));
    }
    Ok(())
}

async fn set_config(conn: &Connection, key: &str, value: &str) -> Result<(), String> {
    conn.call_method(Some(NAME), PATH, Some(NAME), "SetConfig", &(key, value)).await.map(|_| ()).map_err(daemon_error)
}

/// Daemon errors are already translated; drop the D-Bus error name in front.
fn daemon_error(e: zbus::Error) -> String {
    let text = e.to_string();
    match text.strip_prefix("org.freedesktop.DBus.Error.").and_then(|t| t.split_once(": ")) {
        Some((_, msg)) => msg.to_string(),
        None => format!("daemon: {text}"),
    }
}

async fn daemon_prop<T>(conn: &Connection, prop: &str) -> zbus::Result<T>
where
    T: serde::de::DeserializeOwned + zbus::zvariant::Type,
{
    zbus::Proxy::new(conn, NAME, PATH, NAME).await?.get_property(prop).await
}

fn notify(msg: &str) {
    let sent = Command::new("notify-send").args(["-a", "Halo", "-i", "audio-volume-high", msg]).status();
    if !sent.is_ok_and(|s| s.success()) {
        println!("{msg}");
    }
}

// ---- Proton ----

fn proton_list() {
    let prefixes = proton::find_prefixes();
    if prefixes.is_empty() {
        println!("{}", tr!("No Proton prefixes found (steamapps/compatdata); open a game once to create one"));
        return;
    }
    for p in prefixes {
        let st = match proton::status(&p.windows) {
            proton::Status::Installed => tr!("with Halo"),
            proton::Status::Missing => "-".into(),
            proton::Status::Foreign => tr!("Razer DLL (left alone)"),
        };
        println!("{:>10}  {:<26} {}", p.appid, st, p.name.as_deref().unwrap_or("?"));
    }
}

fn proton_scan(appid: Option<&str>) -> Result<(), String> {
    let games: Vec<_> = scan::installed_games().into_iter().filter(|g| appid.is_none_or(|a| g.appid == a)).collect();
    if games.is_empty() {
        return Err(match appid {
            Some(a) => tr!("{appid} is not installed", appid = a),
            None => tr!("No games installed"),
        });
    }
    for g in &games {
        let (hits, files) = scan::scan(g);
        let partial = if g.complete { String::new() } else { tr!(" (incomplete download)") };
        if files == 0 {
            println!("{}", tr!("{name} ({appid}){partial}: no .exe/.dll; looks like the native Linux build (for the Windows SDKs, force Proton in Steam and reinstall)", name = g.name, appid = g.appid, partial = partial));
            continue;
        }
        if hits.is_empty() {
            println!("{}", tr!("{name} ({appid}){partial}: nothing in {files} .exe/.dll", name = g.name, appid = g.appid, partial = partial, files = files));
            continue;
        }
        println!("{} ({}){partial}:", g.name, g.appid);
        for h in hits {
            let file = h.file.strip_prefix(&g.dir).unwrap_or(&h.file).display();
            let halo = if h.sdk.covered { tr!("covered by Halo") } else { tr!("NOT covered by Halo") };
            println!("  {:<24} {halo:<17} {file} ({})", h.sdk.name, h.pattern);
        }
    }
    println!("\n{}", tr!("\"nothing\" = no fingerprints found; an encrypted .exe (Denuvo, etc.) can hide them."));
    Ok(())
}

fn proton_apply(action: &str, target: &str) -> Result<(), String> {
    let all = proton::find_prefixes();
    let chosen: Vec<_> = all.iter().filter(|p| target == "--all" || p.appid == target).collect();
    if chosen.is_empty() {
        return Err(tr!("No Proton prefix for {appid} (halo proton list)", appid = target));
    }
    let dlls = proton::dll_dir();
    let mut failed = false;
    for p in chosen {
        let name = p.name.as_deref().unwrap_or(&p.appid);
        let r = if action == "install" {
            proton::install(&p.windows, &dlls).map(|_| tr!("installed"))
        } else {
            proton::remove(&p.windows).map(|n| tr!("{count} files removed", count = n))
        };
        match r {
            Ok(msg) => println!("{name}: {msg}"),
            Err(e) => {
                eprintln!("{name}: {e}");
                failed = true;
            }
        }
    }
    if failed { Err(tr!("There were errors")) } else { Ok(()) }
}

// ---- straight to the hardware ----

fn open(name: &str) -> Result<Box<dyn Backend>, String> {
    let r: io::Result<Box<dyn Backend>> = match name {
        "auto" => backend::open_auto(),
        "lamparray" => LampArray::open().map(|b| Box::new(b) as _),
        "vendor" => Vendor::open().map(|b| Box::new(b) as _),
        "dummy" => Ok(Box::new(Dummy { echo: true, ..Default::default() })),
        _ => return Err(tr!("Unknown backend: {name}", name = name)),
    };
    r.map_err(|e| e.to_string())
}

fn hex(frame: &halo_core::Frame) -> Vec<String> {
    frame.iter().map(Rgb::to_string).collect()
}

fn parse_frame(args: &[String]) -> Result<halo_core::Frame, String> {
    let colors: Vec<Rgb> = args
        .iter()
        .map(|a| {
            Rgb::parse(a).ok_or_else(|| {
                let names: Vec<&str> = color::NAMED.iter().map(|(n, _)| *n).collect();
                tr!("Invalid color: {value} (use RRGGBB or one of: {names})", value = a, names = names.join(", "))
            })
        })
        .collect::<Result<_, _>>()?;
    match colors[..] {
        [c] => Ok([c; ZONES]),
        _ if colors.len() == ZONES => Ok(colors.try_into().unwrap()),
        _ => Err(usage()),
    }
}

fn current() -> State {
    state::load().unwrap_or(State { frame: [Rgb::OFF; ZONES], brightness: state::DEFAULT_BRIGHTNESS })
}

/// Sends the frame (and the brightness, when given) and saves it.
fn set(backend_name: &str, frame: halo_core::Frame, brightness: Option<u8>) -> Result<(), String> {
    let mut b = open(backend_name)?;
    let s = State { frame, brightness: brightness.unwrap_or_else(|| current().brightness) };
    send(b.as_mut(), &s)?;
    if backend_name != "dummy" {
        state::save(&s).map_err(|e| tr!("Could not save the state: {error}", error = e))?;
    }
    Ok(())
}

fn send(b: &mut dyn Backend, s: &State) -> Result<(), String> {
    // Brightness goes through the vendor channel; without the udev rule there is no access, and
    // that is not fatal.
    if let Err(e) = b.set_brightness(s.brightness) {
        eprintln!("halo: {}", tr!("warning: could not set the hardware brightness ({error})", error = e));
    }
    b.apply(&s.frame).map_err(|e| format!("{}: {e}", b.name()))?;
    println!("{}", tr!("{backend}: {frame} · brightness {brightness}", backend = b.name(), frame = hex(&s.frame).join(" "), brightness = s.brightness));
    Ok(())
}

fn restore(backend_name: &str) -> Result<(), String> {
    let Some(s) = state::load() else {
        println!("{}", tr!("No saved color; nothing to restore"));
        return Ok(());
    };
    send(open(backend_name)?.as_mut(), &s)
}

async fn pick(daemon: &Option<Connection>) -> Result<Rgb, String> {
    let base = match daemon {
        Some(c) => daemon_prop::<Vec<String>>(c, "Base").await.ok().and_then(|v| v.first().and_then(|s| Rgb::parse(s))),
        None => None,
    };
    let default = format!("#{}", base.unwrap_or(current().frame[0]));
    let out = Command::new("kdialog")
        .args(["--title", &tr!("Ring color"), "--getcolor", "--default", &default])
        .output()
        .map_err(|e| tr!("Could not open kdialog: {error}", error = e))?;
    if !out.status.success() {
        return Err(tr!("Cancelled"));
    }
    let s = String::from_utf8_lossy(&out.stdout);
    Rgb::parse(s.trim()).ok_or_else(|| tr!("kdialog returned {value}", value = s.trim()))
}

async fn status(daemon: &Option<Connection>) {
    match daemon {
        Some(c) => {
            let frame = daemon_prop::<Vec<String>>(c, "Frame").await.map(|f| f.join(" ")).unwrap_or_else(|e| e.to_string());
            let backend = daemon_prop::<String>(c, "Backend").await.unwrap_or_default();
            let brightness = daemon_prop::<u8>(c, "Brightness").await.map(|b| b.to_string()).unwrap_or_default();
            let layers = daemon_prop::<Vec<String>>(c, "Layers").await.unwrap_or_default();
            let effect = daemon_prop::<String>(c, "Effect").await.unwrap_or_default();
            let speed = daemon_prop::<String>(c, "EffectSpeed").await.unwrap_or_default();
            let games = daemon_prop::<bool>(c, "GamesEnabled").await.unwrap_or(true);
            let hw = daemon_prop::<bool>(c, "EffectHardware").await.unwrap_or(false);
            println!("{}", tr!("daemon: running · backend {backend} · {frame} · brightness {brightness}", backend = backend, frame = frame, brightness = brightness));
            if !effect.is_empty() {
                let mcu = if hw { tr!(" · animated by the MCU") } else { String::new() };
                println!("  {}", tr!("effect: {effect} ({speed}){mcu}", effect = effect, speed = speed, mcu = mcu));
            }
            if !games {
                println!("  {}", tr!("games may NOT control the lights (halo games on)"));
            }
            if !layers.is_empty() {
                println!("  {}", tr!("layers: {list}", list = layers.join(", ")));
            }
            if let Ok((appid, name)) = daemon_prop::<(String, String)>(c, "CurrentGame").await
                && !appid.is_empty()
            {
                println!("  {}", tr!("running game: {name} ({appid})", name = name, appid = appid));
            }
        }
        None => println!("{}", tr!("daemon: not running (commands go straight to the hardware)")),
    }
    for (label, iface) in [("LampArray (interface 00)", Interface::LampArray), ("vendor    (interface 02)", Interface::Vendor)] {
        match discover::find(iface) {
            Some(p) => {
                let ok = fs::OpenOptions::new().read(true).write(true).open(&p).is_ok();
                let access = if ok { tr!("accessible") } else { tr!("NO access") };
                println!("{label}: {} ({access})", p.display());
            }
            None => println!("{label}: {}", tr!("not found")),
        }
    }
    if let Ok(l) = LampArray::open() {
        match l.attributes() {
            Ok(a) => println!("  {}", tr!("lamps: {count}, kind {kind}, minimum update {us} µs", count = a.lamp_count, kind = a.kind, us = a.min_update_us)),
            Err(e) => println!("  {}", tr!("could not read the attributes: {error}", error = e)),
        }
    }
    match state::load() {
        Some(s) => print!("{}", tr!("saved: {state}", state = state::format(&s))),
        None => println!("{}", tr!("saved: nothing")),
    }
    println!("{}", tr!("state file: {path}", path = state::path().display()));
    let leds = Path::new("/sys/class/leds/ally:rgb:joystick_rings");
    let read = |f: &str| fs::read_to_string(leds.join(f)).map(|s| s.trim().to_string()).unwrap_or_else(|_| "?".into());
    println!("{}", tr!("driver sysfs: brightness={brightness} multi_intensity={intensity} (blue only; Halo does not use it)", brightness = read("brightness"), intensity = read("multi_intensity")));
    let ip = if process_running("inputplumber") { tr!("running") } else { tr!("not running") };
    println!("InputPlumber: {ip}");
}

fn process_running(comm: &str) -> bool {
    fs::read_dir("/proc").into_iter().flatten().flatten().any(|e| fs::read_to_string(e.path().join("comm")).is_ok_and(|c| c.trim() == comm))
}
