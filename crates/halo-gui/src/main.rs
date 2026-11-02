//! `halo-gui`: Halo's window (Qt 6 + Kirigami, through cxx-qt). Everything it does goes through
//! the daemon (`org.halo.Halo1` on the session bus); if the daemon is not there, it says so and waits.

pub mod halo;
mod daemon;

use cxx_qt_lib::{QGuiApplication, QQmlApplicationEngine, QString, QUrl};

fn main() {
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
    if let Some(app) = app.as_mut() {
        app.exec();
    }
}
