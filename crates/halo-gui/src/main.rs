//! `halo-gui`: Halo's window (Qt 6 + Kirigami, through cxx-qt). Everything it does goes through
//! the daemon (`org.halo.Halo1` on the session bus); if the daemon is not there, it says so and waits.
//!
//! Two shapes: the desktop window, and the **handheld** one (`--handheld`, or on its own under
//! gamescope, that is Steam's game mode): fullscreen, touch-sized controls, a bottom bar for the
//! pages and a color picker that works with a gamepad. The gamepad itself is read in both
//! (`gamepad.rs`), unless `--no-gamepad`.

pub mod halo;
mod daemon;
mod gamepad;

use cxx_qt_lib::{QGuiApplication, QQmlApplicationEngine, QString, QUrl};
use std::sync::atomic::Ordering;

/// Handheld unless told otherwise: under gamescope (Steam's game mode) the window has no desktop.
fn handheld(args: &[String]) -> bool {
    if args.iter().any(|a| a == "--handheld") {
        return true;
    }
    if args.iter().any(|a| a == "--desktop") {
        return false;
    }
    let desktop = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default().to_ascii_lowercase();
    desktop.contains("gamescope") || std::env::var_os("GAMESCOPE_WAYLAND_DISPLAY").is_some()
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!("halo-gui [--handheld | --desktop] [--no-gamepad | --gamepads]");
        return;
    }
    if args.iter().any(|a| a == "--gamepads") {
        // Diagnostic: which pads the reader would use, without opening a window.
        for (name, path) in gamepad::list() {
            println!("{name} ({path})");
        }
        return;
    }
    let handheld = handheld(&args);
    halo::HANDHELD.store(handheld, Ordering::Relaxed);
    if handheld && std::env::var_os("QT_QUICK_CONTROLS_MOBILE").is_none() {
        // Kirigami's mobile mode: bigger targets, full-width cards, dialogs instead of popups.
        // SAFETY: before the application exists and with no other threads.
        unsafe { std::env::set_var("QT_QUICK_CONTROLS_MOBILE", "1") };
    }
    // Kirigami needs a QtQuick Controls style; Plasma's for QGuiApplication is Breeze.
    if std::env::var_os("QT_QUICK_CONTROLS_STYLE").is_none() {
        // SAFETY: before the application exists and with no other threads.
        unsafe { std::env::set_var("QT_QUICK_CONTROLS_STYLE", "org.kde.breeze") };
    }
    let mut app = QGuiApplication::new();
    if let Some(mut app) = app.as_mut() {
        app.as_mut().set_application_name(&QString::from("halo"));
        app.as_mut().set_organization_domain(&QString::from("halo.org"));
        // The window icon comes from the .desktop file (on Wayland, through the desktop name).
        QGuiApplication::set_desktop_file_name(&QString::from("org.halo.gui"));
    }
    let mut engine = QQmlApplicationEngine::new();
    if let Some(engine) = engine.as_mut() {
        engine.load(&QUrl::from("qrc:/qt/qml/org/halo/gui/qml/Main.qml"));
    }
    if !args.iter().any(|a| a == "--no-gamepad") {
        gamepad::start();
    }
    if let Ok(script) = std::env::var("HALO_GUI_SCRIPT") {
        run_script(script);
    }
    if let Some(app) = app.as_mut() {
        app.exec();
    }
}

/// `HALO_GUI_SCRIPT`: a comma-separated list of steps played 1.5 s after start, 400 ms apart, for
/// testing the gamepad navigation without a pad (works with `QT_QPA_PLATFORM=offscreen`). A step
/// is a key (Up, Down, Left, Right, Tab, Backtab, Space, Escape, Return, Back, Forward), `focus` (prints
/// the focused item), `shot:<png>` (saves the window) or `quit`.
fn run_script(script: String) {
    let _ = std::thread::Builder::new().name("script".into()).spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(1500));
        for step in script.split(',').map(str::trim).filter(|s| !s.is_empty()) {
            let key = match step.to_ascii_lowercase().as_str() {
                "up" => 0x0100_0013,
                "down" => 0x0100_0015,
                "left" => 0x0100_0012,
                "right" => 0x0100_0014,
                "tab" => 0x0100_0001,
                "backtab" => 0x0100_0002,
                "space" => 0x20,
                "escape" => 0x0100_0000,
                "return" => 0x0100_0004,
                "back" => 0x0100_0061,
                "forward" => 0x0100_0062,
                _ => 0,
            };
            if key != 0 {
                halo::qobject::send_key(key, 0);
            } else {
                halo::qobject::debug_step(step);
            }
            std::thread::sleep(std::time::Duration::from_millis(400));
        }
    });
}
