import QtQuick
import QtQuick.Layouts
import QtQuick.Controls as QQC2
import org.kde.kirigami as Kirigami
import org.kde.kirigamiaddons.formcard as FormCard
import org.halo.gui

Kirigami.ScrollablePage {
    title: Halo.tr("Settings")

    readonly property var config: {
        try { return JSON.parse(Halo.configJson); } catch (e) { return {}; }
    }
    function yes(v) { return v === "yes"; }

    ColumnLayout {
        spacing: Kirigami.Units.largeSpacing

        FormCard.FormHeader { title: Halo.tr("Battery") }
        FormCard.FormCard {
            FormCard.FormSwitchDelegate {
                text: Halo.tr("Turn the rings off with the screen")
                description: Halo.tr("When the screen turns off, so do the rings; they come back when it turns on.")
                checked: yes(config.screen_off)
                onToggled: Halo.setConfig("screen_off", checked ? "yes" : "no")
            }
            FormCard.FormDelegateSeparator {}
            FormCard.FormSwitchDelegate {
                text: Halo.tr("Hardware effects")
                description: Halo.tr("Breathing, cycle and rainbow are animated by the rings' MCU without using the CPU.")
                checked: yes(config.hardware_effects)
                onToggled: Halo.setConfig("hardware_effects", checked ? "yes" : "no")
            }
        }

        FormCard.FormHeader { title: Halo.tr("Sun effect") }
        FormCard.FormCard {
            FormCard.FormTextFieldDelegate {
                id: lat
                label: Halo.tr("Latitude")
                placeholderText: "auto"
                text: (config.latitude || "auto") === "auto" ? "" : config.latitude
                onEditingFinished: Halo.setConfig("latitude", text.trim() === "" ? "auto" : text.trim())
            }
            FormCard.FormDelegateSeparator {}
            FormCard.FormTextFieldDelegate {
                id: lon
                label: Halo.tr("Longitude")
                placeholderText: "auto"
                text: (config.longitude || "auto") === "auto" ? "" : config.longitude
                onEditingFinished: Halo.setConfig("longitude", text.trim() === "" ? "auto" : text.trim())
            }
            FormCard.FormTextDelegate {
                text: Halo.tr("In degrees; north and east positive. Empty = approximated from the time zone.")
            }
        }

        FormCard.FormHeader { title: Halo.tr("Status") }
        FormCard.FormCard {
            FormCard.FormTextDelegate {
                text: Halo.tr("Daemon")
                description: Halo.connected ? (Halo.tr("running · backend") + " " + Halo.backend) : Halo.tr("not connected")
            }
            FormCard.FormDelegateSeparator {}
            FormCard.FormTextDelegate {
                text: Halo.tr("File")
                description: "~/.config/halo/halo.conf"
            }
        }
    }
}
