import QtQuick
import QtQuick.Layouts
import QtQuick.Controls as QQC2
import org.kde.kirigami as Kirigami
import org.halo.gui

// A color picker for fingers and gamepads: hue, saturation and brightness sliders (Left/Right
// move them in steps) and a row of swatches. Used in handheld mode instead of the platform's
// color dialog, which only a mouse can drive.
QQC2.Dialog {
    id: dialog
    property color selectedColor: "#ffffff"
    // The chosen color as "rrggbb".
    readonly property string hex: selectedColor.toString().substring(1)

    title: Halo.tr("Pick a color")
    modal: true
    standardButtons: QQC2.Dialog.Ok | QQC2.Dialog.Cancel
    parent: QQC2.Overlay.overlay
    anchors.centerIn: parent
    width: Math.min(parent.width - Kirigami.Units.gridUnit * 2, Kirigami.Units.gridUnit * 30)

    readonly property var swatches: ["ff0000", "ff8000", "ffff00", "00ff00", "00ffff", "0000ff", "8000ff", "ff00ff", "ff80c0", "ffffff", "000000"]

    function fromSliders() {
        selectedColor = Qt.hsva(hue.value / 360, sat.value / 100, val.value / 100, 1);
    }
    function toSliders() {
        hue.value = Math.max(0, selectedColor.hsvHue) * 360;
        sat.value = selectedColor.hsvSaturation * 100;
        val.value = selectedColor.hsvValue * 100;
    }
    onAboutToShow: {
        toSliders();
        hue.forceActiveFocus(Qt.TabFocusReason);
    }

    contentItem: ColumnLayout {
        spacing: Kirigami.Units.largeSpacing
        // A popup is outside the page, so the keys nobody takes are forwarded from here.
        Keys.onPressed: event => applicationWindow().handleKey(event)

        Rectangle {
            Layout.fillWidth: true
            Layout.preferredHeight: Kirigami.Units.gridUnit * 3
            radius: Kirigami.Units.smallSpacing
            color: dialog.selectedColor
            border.color: Kirigami.Theme.textColor
            border.width: 1
        }

        GridLayout {
            Layout.fillWidth: true
            columns: 2
            columnSpacing: Kirigami.Units.largeSpacing

            QQC2.Label { text: Halo.tr("Hue") }
            QQC2.Slider {
                id: hue
                Layout.fillWidth: true
                from: 0; to: 359; stepSize: 15
                onMoved: dialog.fromSliders()
            }
            QQC2.Label { text: Halo.tr("Saturation") }
            QQC2.Slider {
                id: sat
                Layout.fillWidth: true
                from: 0; to: 100; stepSize: 10
                onMoved: dialog.fromSliders()
            }
            QQC2.Label { text: Halo.tr("Brightness") }
            QQC2.Slider {
                id: val
                Layout.fillWidth: true
                from: 0; to: 100; stepSize: 10
                onMoved: dialog.fromSliders()
            }
        }

        Flow {
            Layout.fillWidth: true
            spacing: Kirigami.Units.smallSpacing
            Repeater {
                model: dialog.swatches
                QQC2.AbstractButton {
                    id: swatch
                    required property string modelData
                    width: Kirigami.Units.gridUnit * 2.5
                    height: width
                    focusPolicy: Qt.StrongFocus
                    Accessible.name: modelData
                    background: Rectangle {
                        radius: height / 2
                        color: "#" + swatch.modelData
                        border.color: swatch.visualFocus ? Kirigami.Theme.highlightColor : Kirigami.Theme.textColor
                        border.width: swatch.visualFocus ? 3 : 1
                    }
                    onClicked: {
                        dialog.selectedColor = "#" + modelData;
                        dialog.toSliders();
                    }
                }
            }
        }
    }
}
