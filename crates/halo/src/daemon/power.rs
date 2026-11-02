//! Restore on wake-up: logind signals it with `PrepareForSleep(false)`.
//! On wake-up the MCU may return to autonomous mode and lose the hardware brightness, so everything is reopened.
//!
//! Also turn the rings off with the screen: the kernel reports in `/sys/class/drm/*-eDP-*/dpms`
//! whether the internal panel is on. The driver writes it, so it works the same under KWin and
//! gamescope. There is no event for it (neither uevent nor inotify), so it is read every [`SCREEN_EVERY`].

use super::device::Core;
use futures_util::StreamExt;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use zbus::Connection;

/// After wake-up: InputPlumber rewrites the LED within the first second; two attempts cover it.
const AFTER_RESUME: [Duration; 2] = [Duration::from_secs(2), Duration::from_secs(3)];

const SCREEN_EVERY: Duration = Duration::from_secs(1);

/// The `dpms` of the connected internal panel (eDP or LVDS).
fn panel_dpms() -> Option<std::path::PathBuf> {
    let mut found: Vec<_> = std::fs::read_dir("/sys/class/drm").ok()?.flatten().map(|e| e.path()).collect();
    found.sort();
    found.into_iter().find(|p| {
        let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
        (name.contains("-eDP-") || name.contains("-LVDS-"))
            && std::fs::read_to_string(p.join("status")).is_ok_and(|s| s.trim() == "connected")
    }).map(|p| p.join("dpms"))
}

pub fn screen_on(dpms: &str) -> bool {
    dpms.trim() != "Off"
}

pub async fn watch_screen(core: Arc<Mutex<Core>>) {
    let Some(dpms) = panel_dpms() else {
        eprintln!("halo: internal panel not found in /sys/class/drm; the rings will not turn off with the screen");
        return;
    };
    eprintln!("halo: following the screen at {}", dpms.display());
    let mut tick = tokio::time::interval(SCREEN_EVERY);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tick.tick().await;
        // If unreadable (disconnected?), treat it as on: better extra lights than a blackout.
        let on = std::fs::read_to_string(&dpms).map_or(true, |s| screen_on(&s));
        let mut core = core.lock().await;
        if core.comp.blank == !on {
            continue;
        }
        core.comp.blank = !on;
        eprintln!("halo: screen {}", if on { "on: rings back" } else { "off: rings off" });
        // On turning on, reopen: the MCU may have gone to autonomous mode in the meantime.
        let r = if on { core.reclaim() } else { core.push() };
        if let Err(e) = r {
            eprintln!("halo: {e}");
        }
    }
}

pub async fn watch_sleep(core: Arc<Mutex<Core>>) -> zbus::Result<()> {
    let system = Connection::system().await?;
    let login = zbus::Proxy::new(&system, "org.freedesktop.login1", "/org/freedesktop/login1", "org.freedesktop.login1.Manager").await?;
    let mut signals = login.receive_signal("PrepareForSleep").await?;
    eprintln!("halo: listening for logind sleep events");
    while let Some(msg) = signals.next().await {
        let going_to_sleep: bool = msg.body().deserialize()?;
        if going_to_sleep {
            continue;
        }
        for wait in AFTER_RESUME {
            tokio::time::sleep(wait).await;
            match core.lock().await.reclaim() {
                Ok(_) => eprintln!("halo: restored after wake-up"),
                Err(e) => eprintln!("halo: could not restore after wake-up: {e}"),
            }
        }
    }
    Ok(())
}
