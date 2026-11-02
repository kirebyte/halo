//! `halo daemon`: sole owner of the rings. Without animations it only waits for events and reads
//! the screen state once per second (`power::watch_screen`, disabled with `apagar_con_pantalla = no`).

pub(crate) mod bridges;
mod compositor;
pub(crate) mod config;
mod device;
pub(crate) mod effects;
pub mod dbus;
mod power;
pub(crate) mod steam;

use halo_core::state::{self, State};
use halo_core::{Rgb, ZONES};
use std::sync::Arc;
use std::time::Duration;
use tokio::signal::unix::{SignalKind, signal};
use tokio::sync::Mutex;

pub async fn run(backend_name: &str) -> Result<(), String> {
    let initial = state::load().unwrap_or(State { frame: [Rgb::OFF; ZONES], brightness: state::DEFAULT_BRIGHTNESS });
    let core = Arc::new(Mutex::new(device::Core::new(backend_name, initial)));

    // Startup: the MCU may still be waking up, so it is retried once later.
    if let Err(e) = core.lock().await.reclaim() {
        eprintln!("halo: no device yet ({e}); retrying on the next change");
    }
    let late = core.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(5)).await;
        let _ = late.lock().await.reclaim();
    });

    let _conn = zbus::connection::Builder::session()
        .name(dbus::NAME)
        .serve_at(dbus::PATH, dbus::Halo::new(core.clone()).await)
        .build()
        .await
        .map_err(|e| format!("could not register {} on the session bus: {e}", dbus::NAME))?;
    eprintln!("halo: daemon ready at {} ({})", dbus::NAME, core.lock().await.backend());

    // Per-game profiles: the Steam watcher reports the current game and the D-Bus object applies it.
    let iface = _conn.object_server().interface::<_, dbus::Halo>(dbus::PATH).await.map_err(|e| e.to_string())?;
    let (games_tx, mut games_rx) = tokio::sync::mpsc::channel(8);
    tokio::spawn(steam::watch(games_tx));
    tokio::spawn(async move {
        while let Some(game) = games_rx.recv().await {
            iface.get().await.set_current_game(game).await;
        }
    });

    let sleeper = core.clone();
    tokio::spawn(async move {
        if let Err(e) = power::watch_sleep(sleeper).await {
            eprintln!("halo: no sleep notifications ({e}); the color will not be restored on wake-up");
        }
    });

    let chroma = core.clone();
    tokio::spawn(async move {
        if let Err(e) = bridges::chroma::serve(chroma, bridges::chroma::PORT).await {
            eprintln!("halo: no Chroma bridge ({e})");
        }
    });

    let chroma_rest = core.clone();
    tokio::spawn(async move {
        if let Err(e) = bridges::chroma_rest::serve(chroma_rest, bridges::chroma_rest::PORT).await {
            eprintln!("halo: no Chroma REST bridge ({e})");
        }
    });

    let gamesense = core.clone();
    let persist = backend_name != "dummy";
    tokio::spawn(async move {
        if let Err(e) = bridges::gamesense::serve(gamesense, bridges::gamesense::PORT, persist).await {
            eprintln!("halo: no GameSense bridge ({e})");
        }
    });

    let logitech = core.clone();
    tokio::spawn(async move {
        if let Err(e) = bridges::logitech::serve(logitech, bridges::logitech::PORT).await {
            eprintln!("halo: no Logitech bridge ({e})");
        }
    });

    let openrgb = core.clone();
    tokio::spawn(async move {
        if let Err(e) = bridges::openrgb::serve(openrgb, bridges::openrgb::PORT).await {
            eprintln!("halo: no OpenRGB server ({e}; is OpenRGB itself running?)");
        }
    });

    let mut term = signal(SignalKind::terminate()).map_err(|e| e.to_string())?;
    tokio::select! {
        _ = term.recv() => {}
        _ = tokio::signal::ctrl_c() => {}
    }
    // The rings stay as they are: the color lives in the MCU.
    eprintln!("halo: exiting");
    Ok(())
}
