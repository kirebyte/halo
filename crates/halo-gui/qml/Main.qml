import QtQuick
import QtQuick.Controls as QQC2
import org.kde.kirigami as Kirigami
import org.halo.gui

Kirigami.ApplicationWindow {
    id: root
    title: "Halo"
    minimumWidth: Kirigami.Units.gridUnit * 24
    minimumHeight: Kirigami.Units.gridUnit * 20
    width: Kirigami.Units.gridUnit * 36
    height: Kirigami.Units.gridUnit * 28

    // The state is read again every second; with a software effect, five times per second so
    // the rings in the window move smoothly (the MCU animates without telling: nothing to read).
    Timer {
        interval: Halo.connected && Halo.effect.length > 0 && !Halo.hardware ? 200 : 1000
        running: true
        repeat: true
        triggeredOnStart: true
        onTriggered: Halo.refresh()
    }

    globalDrawer: Kirigami.GlobalDrawer {
        title: "Halo"
        titleIcon: "org.halo.gui"
        isMenu: !root.wideScreen
        actions: [
            Kirigami.Action {
                text: Halo.tr("Lights")
                icon.name: "color-management"
                onTriggered: root.pageStack.replace(lightsPage)
            },
            Kirigami.Action {
                text: Halo.tr("Games")
                icon.name: "input-gamepad"
                onTriggered: root.pageStack.replace(gamesPage)
            },
            Kirigami.Action {
                text: Halo.tr("Settings")
                icon.name: "configure"
                onTriggered: root.pageStack.replace(settingsPage)
            }
        ]
    }

    pageStack.initialPage: lightsPage

    Component { id: lightsPage; LightsPage {} }
    Component { id: gamesPage; GamesPage {} }
    Component { id: settingsPage; SettingsPage {} }

    footer: Kirigami.InlineMessage {
        visible: Halo.error.length > 0
        type: Halo.connected ? Kirigami.MessageType.Warning : Kirigami.MessageType.Error
        text: Halo.error
    }
}
