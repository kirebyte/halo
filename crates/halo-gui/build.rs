use cxx_qt_build::{CxxQtBuilder, QmlModule};

fn main() {
    CxxQtBuilder::new_qml_module(
        QmlModule::new("org.halo.gui")
            .qml_file("qml/Main.qml")
            .qml_file("qml/Rings.qml")
            .qml_file("qml/LightsPage.qml")
            .qml_file("qml/GamesPage.qml")
            .qml_file("qml/SettingsPage.qml"),
    )
    .files(["src/halo.rs"])
    .build();
}
