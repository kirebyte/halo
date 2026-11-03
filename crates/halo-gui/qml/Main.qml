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
    // Handheld: the whole screen (there is no desktop under gamescope).
    visibility: Halo.handheld ? Window.FullScreen : Window.AutomaticVisibility

    // The state is read again every second; with a software effect, five times per second so
    // the rings in the window move smoothly (the MCU animates without telling: nothing to read).
    Timer {
        interval: Halo.connected && Halo.effect.length > 0 && !Halo.hardware ? 200 : 1000
        running: true
        repeat: true
        triggeredOnStart: true
        onTriggered: Halo.refresh()
    }

    readonly property var pages: [lightsPage, gamesPage, settingsPage]
    property int currentPage: 0

    function showPage(index) {
        const i = (index + pages.length) % pages.length;
        if (i === currentPage && pageStack.depth > 0) return;
        currentPage = i;
        pageStack.replace(pages[i]);
    }

    // Keys no control took, bubbled up from the pages (each page forwards them here).
    // With a gamepad, `gamepad.rs` already sends Tab/Backtab/Space/Escape/Back/Forward; the
    // arrows and Return come from Steam's desktop layout or a keyboard, and are turned into the
    // keys the controls understand.
    function handleKey(event) {
        switch (event.key) {
        case Qt.Key_Up: Halo.sendKey(Qt.Key_Backtab); break;
        case Qt.Key_Down: Halo.sendKey(Qt.Key_Tab); break;
        case Qt.Key_Return:
        case Qt.Key_Enter: Halo.sendKey(Qt.Key_Space); break;
        case Qt.Key_Back: showPage(currentPage - 1); break;
        case Qt.Key_Forward: showPage(currentPage + 1); break;
        case Qt.Key_Escape:
            if (currentPage !== 0) showPage(0); else return;
            break;
        default: return;
        }
        event.accepted = true;
    }

    // In handheld mode the bottom bar takes the drawer's place.
    globalDrawer: Halo.handheld ? null : drawer

    Kirigami.GlobalDrawer {
        id: drawer
        title: "Halo"
        titleIcon: "org.halo.gui"
        isMenu: !root.wideScreen
        actions: [
            Kirigami.Action {
                text: Halo.tr("Lights")
                icon.name: "color-management"
                onTriggered: root.showPage(0)
            },
            Kirigami.Action {
                text: Halo.tr("Games")
                icon.name: "input-gamepad"
                onTriggered: root.showPage(1)
            },
            Kirigami.Action {
                text: Halo.tr("Settings")
                icon.name: "configure"
                onTriggered: root.showPage(2)
            }
        ]
    }

    pageStack.initialPage: lightsPage

    Component { id: lightsPage; LightsPage {} }
    Component { id: gamesPage; GamesPage {} }
    Component { id: settingsPage; SettingsPage {} }

    footer: Column {
        Kirigami.InlineMessage {
            width: parent.width
            visible: Halo.error.length > 0
            type: Halo.connected ? Kirigami.MessageType.Warning : Kirigami.MessageType.Error
            text: Halo.error
        }
        QQC2.Label {
            width: parent.width
            visible: Halo.handheld
            horizontalAlignment: Text.AlignHCenter
            padding: Kirigami.Units.smallSpacing
            opacity: 0.6
            text: Halo.tr("A: select · B: back · LB/RB: change page")
        }
        Kirigami.NavigationTabBar {
            width: parent.width
            visible: Halo.handheld
            actions: [
                Kirigami.Action {
                    text: Halo.tr("Lights")
                    icon.name: "color-management"
                    checked: root.currentPage === 0
                    onTriggered: root.showPage(0)
                },
                Kirigami.Action {
                    text: Halo.tr("Games")
                    icon.name: "input-gamepad"
                    checked: root.currentPage === 1
                    onTriggered: root.showPage(1)
                },
                Kirigami.Action {
                    text: Halo.tr("Settings")
                    icon.name: "configure"
                    checked: root.currentPage === 2
                    onTriggered: root.showPage(2)
                }
            ]
        }
    }
}
