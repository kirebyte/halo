import QtQuick
import QtQuick.Controls as QQC2
import QtQuick.Shapes
import org.kde.kirigami as Kirigami

// The two rings, with their four halves in the colors the daemon reports.
// Zones: 0 left bottom, 1 left top, 2 right top, 3 right bottom.
// Each half is a button: tap it, or reach it with Tab and press Space (a gamepad's A).
Item {
    id: rings
    // "rrggbb" per zone.
    property var colors: ["000000", "000000", "000000", "000000"]
    // Emitted with the zone that was tapped.
    signal zoneTapped(int zone)

    implicitWidth: Kirigami.Units.gridUnit * 18
    implicitHeight: Kirigami.Units.gridUnit * 8

    readonly property real ringSize: Math.min(height, width / 2.4)
    readonly property real thickness: ringSize * 0.22

    Row {
        anchors.centerIn: parent
        spacing: rings.ringSize * 0.4

        Repeater {
            model: 2 // 0 = left, 1 = right
            Item {
                id: ring
                required property int modelData
                width: rings.ringSize
                height: rings.ringSize

                // Top and bottom. Top zone: 1 (left) or 2 (right); bottom: 0 (left) or 3 (right).
                Repeater {
                    model: 2
                    Item {
                        id: half
                        required property int modelData
                        readonly property bool isTop: modelData === 0
                        readonly property int zone: ring.modelData === 0 ? (isTop ? 1 : 0) : (isTop ? 2 : 3)
                        readonly property color zoneColor: "#" + rings.colors[zone]
                        anchors.fill: parent

                        Shape {
                            anchors.fill: parent
                            antialiasing: true
                            ShapePath {
                                strokeWidth: rings.thickness
                                strokeColor: half.zoneColor
                                fillColor: "transparent"
                                capStyle: ShapePath.FlatCap
                                PathAngleArc {
                                    centerX: ring.width / 2
                                    centerY: ring.height / 2
                                    radiusX: ring.width / 2 - rings.thickness / 2
                                    radiusY: radiusX
                                    startAngle: half.isTop ? 180 : 0
                                    sweepAngle: 180
                                }
                            }
                            // A faint outline so the half is visible even when off; the focus
                            // highlight when the half is reached with the keyboard or a gamepad.
                            ShapePath {
                                strokeWidth: button.visualFocus ? 3 : 1
                                strokeColor: button.visualFocus ? Kirigami.Theme.highlightColor : Kirigami.Theme.disabledTextColor
                                fillColor: "transparent"
                                PathAngleArc {
                                    centerX: ring.width / 2
                                    centerY: ring.height / 2
                                    radiusX: ring.width / 2
                                    radiusY: radiusX
                                    startAngle: half.isTop ? 180 : 0
                                    sweepAngle: 180
                                }
                            }
                        }

                        // The half's own strip, so the two halves do not overlap and both get
                        // their taps (one area per ring deciding by y was the previous way).
                        QQC2.AbstractButton {
                            id: button
                            x: 0
                            y: half.isTop ? 0 : half.height / 2
                            width: half.width
                            height: half.height / 2
                            focusPolicy: Qt.StrongFocus
                            Accessible.name: "zone " + half.zone
                            onClicked: rings.zoneTapped(half.zone)
                        }
                    }
                }
            }
        }
    }
}
