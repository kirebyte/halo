import QtQuick
import QtQuick.Layouts
import QtQuick.Controls as QQC2
import QtQuick.Dialogs
import org.kde.kirigami as Kirigami
import org.kde.kirigamiaddons.formcard as FormCard
import org.halo.gui

Kirigami.ScrollablePage {
    id: page
    title: Halo.tr("Lights")

    // Which zone the color dialog is for (-1 = all four, -2 = one of the effect's colors).
    property int pickingZone: -1
    // Which effect color is being edited (index in effectColors, or effectColors.length to add).
    property int pickingEffectColor: 0

    readonly property var config: {
        try { return JSON.parse(Halo.configJson); } catch (e) { return {}; }
    }
    // The running effect's custom colors, as ["rrggbb", ...].
    readonly property var effectColors: {
        const v = config["colors_" + Halo.effect];
        return v ? v.split(" ") : [];
    }
    readonly property bool effectReversed: config["reverse_" + Halo.effect] === "yes"
    // How many colors each effect takes (missing = none).
    readonly property var colorSlots: ({ breathing: 2, strobe: 1, cycle: 8, rainbow: 8, comet: 1, tide: 2, starry: 4, star: 2, pulse: 8 })
    readonly property int maxColors: colorSlots[Halo.effect] || 0
    readonly property bool hasDirection: ["rainbow", "cycle", "comet", "tide"].indexOf(Halo.effect) >= 0

    // The last non-zero brightness, to turn the rings back on with the same one.
    property int lastBrightness: 2
    onConfigChanged: if (Halo.brightness > 0) lastBrightness = Halo.brightness

    function saveEffectColors(list) {
        Halo.setConfig("colors_" + Halo.effect, list.length ? list.join(" ") : "auto");
    }

    ColorDialog {
        id: colorDialog
        title: page.pickingZone < 0 ? Halo.tr("Ring color") : Halo.tr("Zone color")
        onAccepted: {
            const hex = selectedColor.toString().substring(1); // "#rrggbb" → "rrggbb"
            if (page.pickingZone === -2) {
                let list = page.effectColors.slice();
                if (page.pickingEffectColor < list.length) list[page.pickingEffectColor] = hex; else list.push(hex);
                page.saveEffectColors(list);
            } else if (page.pickingZone < 0) {
                Halo.setZones([hex]);
            } else {
                let zones = [];
                for (let i = 0; i < 4; i++)
                    zones.push(i === page.pickingZone ? hex : Halo.base[i]);
                Halo.setZones(zones);
            }
        }
    }

    ColumnLayout {
        spacing: Kirigami.Units.largeSpacing

        Rings {
            Layout.alignment: Qt.AlignHCenter
            Layout.topMargin: Kirigami.Units.largeSpacing
            colors: Halo.frame.length === 4 ? Halo.frame : ["000000", "000000", "000000", "000000"]
            onZoneTapped: zone => {
                page.pickingZone = zone;
                colorDialog.selectedColor = "#" + Halo.base[zone];
                colorDialog.open();
            }
        }

        QQC2.Label {
            Layout.alignment: Qt.AlignHCenter
            text: Halo.effect.length > 0
                ? (Halo.tr("Effect") + " " + Halo.tr("effect." + Halo.effect) + " (" + Halo.tr("speed." + Halo.speed) + ")" + (Halo.hardware ? " · " + Halo.tr("animated by the MCU, it does not move here") : ""))
                : Halo.tr("Tap a half to change its color")
            opacity: 0.7
        }

        FormCard.FormHeader { title: Halo.tr("Base color") }
        FormCard.FormCard {
            // Armoury Crate's "Dark": hardware brightness 0 turns the rings off whatever paints them.
            FormCard.FormSwitchDelegate {
                text: Halo.tr("Rings off")
                description: checked ? Halo.tr("Nothing turns them on (not even games) until you switch this off") : Halo.tr("Turns the rings off without losing the color or the effect")
                checked: Halo.brightness === 0
                onToggled: Halo.applyBrightness(checked ? 0 : page.lastBrightness)
            }
            FormCard.FormDelegateSeparator {}
            FormCard.FormButtonDelegate {
                text: Halo.tr("One color for all four")
                icon.name: "color-picker"
                onClicked: {
                    page.pickingZone = -1;
                    colorDialog.selectedColor = "#" + (Halo.base.length ? Halo.base[0] : "000000");
                    colorDialog.open();
                }
            }
            FormCard.FormDelegateSeparator {}
            FormCard.AbstractFormDelegate {
                background: null
                contentItem: RowLayout {
                    QQC2.Label { text: Halo.tr("Hardware brightness") }
                    QQC2.Slider {
                        id: brightness
                        Layout.fillWidth: true
                        from: 0; to: 3; stepSize: 1; snapMode: QQC2.Slider.SnapAlways
                        value: Halo.brightness
                        onMoved: Halo.applyBrightness(Math.round(value))
                    }
                    QQC2.Label { text: Math.round(brightness.value) === 0 ? Halo.tr("off") : Math.round(brightness.value) }
                }
            }
        }

        FormCard.FormHeader { title: Halo.tr("Effect") }
        FormCard.FormCard {
            FormCard.FormComboBoxDelegate {
                id: effectBox
                text: Halo.tr("Effect")
                readonly property var ids: ["none"].concat(Halo.effects)
                model: ids.map(e => e === "none" ? Halo.tr("none") : Halo.tr("effect." + e))
                currentIndex: Math.max(0, ids.indexOf(Halo.effect.length ? Halo.effect : "none"))
                onActivated: index => {
                    if (index === 0) Halo.stopEffect(); else Halo.startEffect(ids[index]);
                }
            }
            FormCard.FormDelegateSeparator {}
            FormCard.AbstractFormDelegate {
                background: null
                contentItem: RowLayout {
                    QQC2.Label { text: Halo.tr("Speed") }
                    Item { Layout.fillWidth: true }
                    Repeater {
                        model: ["slow", "medium", "fast"]
                        QQC2.Button {
                            required property string modelData
                            text: Halo.tr("speed." + modelData)
                            checkable: true
                            checked: Halo.speed === modelData
                            onClicked: Halo.applySpeed(modelData)
                        }
                    }
                }
            }
        }

        FormCard.FormHeader {
            title: Halo.tr("Effect colors and direction")
            visible: Halo.effect.length > 0 && (page.maxColors > 0 || page.hasDirection)
        }
        FormCard.FormCard {
            visible: Halo.effect.length > 0 && (page.maxColors > 0 || page.hasDirection)
            FormCard.AbstractFormDelegate {
                visible: page.maxColors > 0
                background: null
                contentItem: ColumnLayout {
                    QQC2.Label {
                        text: page.effectColors.length ? Halo.tr("Custom colors (tap one to change it)") : Halo.tr("Default colors; add one to change them")
                        opacity: 0.7
                    }
                    Flow {
                        Layout.fillWidth: true
                        spacing: Kirigami.Units.smallSpacing
                        Repeater {
                            model: page.effectColors
                            QQC2.AbstractButton {
                                required property string modelData
                                required property int index
                                width: Kirigami.Units.gridUnit * 2.5
                                height: width
                                background: Rectangle {
                                    radius: height / 2
                                    color: "#" + parent.modelData
                                    border.color: Kirigami.Theme.textColor
                                    border.width: 1
                                }
                                onClicked: {
                                    page.pickingZone = -2;
                                    page.pickingEffectColor = index;
                                    colorDialog.selectedColor = "#" + modelData;
                                    colorDialog.open();
                                }
                            }
                        }
                        QQC2.Button {
                            visible: page.effectColors.length < page.maxColors
                            icon.name: "list-add"
                            text: Halo.tr("Add")
                            onClicked: {
                                page.pickingZone = -2;
                                page.pickingEffectColor = page.effectColors.length;
                                colorDialog.selectedColor = "#ffffff";
                                colorDialog.open();
                            }
                        }
                        QQC2.Button {
                            visible: page.effectColors.length > 0
                            icon.name: "edit-undo"
                            text: Halo.tr("Remove the last")
                            onClicked: page.saveEffectColors(page.effectColors.slice(0, -1))
                        }
                        QQC2.Button {
                            visible: page.effectColors.length > 0
                            icon.name: "edit-clear"
                            text: Halo.tr("Defaults")
                            onClicked: page.saveEffectColors([])
                        }
                    }
                }
            }
            FormCard.FormDelegateSeparator { visible: page.maxColors > 0 && page.hasDirection }
            FormCard.FormSwitchDelegate {
                visible: page.hasDirection
                text: Halo.tr("Reverse")
                description: Halo.tr("Runs through the zones the other way round")
                checked: page.effectReversed
                onToggled: Halo.setConfig("reverse_" + Halo.effect, checked ? "yes" : "no")
            }
        }

        FormCard.FormHeader {
            title: Halo.tr("Layers")
            visible: Halo.layers.length > 0
        }
        FormCard.FormCard {
            visible: Halo.layers.length > 0
            Repeater {
                model: Halo.layers
                FormCard.FormTextDelegate {
                    required property string modelData
                    text: modelData
                }
            }
        }
    }
}
