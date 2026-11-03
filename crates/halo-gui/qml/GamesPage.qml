import QtQuick
import QtQuick.Layouts
import QtQuick.Controls as QQC2
import org.kde.kirigami as Kirigami
import org.kde.kirigamiaddons.formcard as FormCard
import org.halo.gui

Kirigami.ScrollablePage {
    Keys.onPressed: event => applicationWindow().handleKey(event)
    title: Halo.tr("Games")

    readonly property var games: {
        try { return JSON.parse(Halo.gamesJson); } catch (e) { return []; }
    }
    readonly property var currentGame: {
        try { return JSON.parse(Halo.currentGameJson); } catch (e) { return { appid: "", name: "" }; }
    }
    readonly property var profiles: {
        try { return JSON.parse(Halo.profilesJson); } catch (e) { return []; }
    }
    readonly property var installed: {
        try { return JSON.parse(Halo.installedJson); } catch (e) { return []; }
    }
    readonly property var prefixes: {
        try { return JSON.parse(Halo.prefixesJson); } catch (e) { return []; }
    }
    readonly property var currentProfile: profiles.find(p => p.appid === currentGame.appid)

    // What a game can get when it opens: the current color, off, or an effect.
    readonly property var choices: [{ label: Halo.tr("The current color"), value: Halo.base.join(" ") }, { label: Halo.tr("Off"), value: "off" }]
        .concat(Halo.effects.map(e => ({ label: Halo.tr("Effect") + " " + Halo.tr("effect." + e), value: e })))

    // A profile value as shown to the user: effect names translated, colors as they are.
    function describe(value) {
        if (value === "off") return Halo.tr("Off");
        return Halo.effects.indexOf(value) >= 0 ? Halo.tr("effect." + value) : value;
    }

    ColumnLayout {
        spacing: Kirigami.Units.largeSpacing

        FormCard.FormHeader { title: Halo.tr("General permission") }
        FormCard.FormCard {
            FormCard.FormSwitchDelegate {
                text: Halo.tr("Games control the lights")
                description: Halo.tr("Chroma, GameSense and Logitech paint the rings while you play. When you quit, your color or effect comes back.")
                checked: Halo.gamesEnabled
                onToggled: Halo.applyGamesEnabled(checked)
            }
        }

        FormCard.FormHeader { title: Halo.tr("Per game") }
        FormCard.FormCard {
            FormCard.FormTextDelegate {
                visible: games.length === 0
                text: Halo.tr("No game connected or blocked")
                description: Halo.tr("Games show up here when they send lights, with the name they give themselves.")
            }
            Repeater {
                model: games
                FormCard.FormSwitchDelegate {
                    required property var modelData
                    text: modelData.name
                    description: modelData.connected
                        ? (Halo.tr("connected through {bridge}").replace("{bridge}", modelData.bridge) + ", " + (modelData.allowed ? Halo.tr("painting") : Halo.tr("blocked")))
                        : Halo.tr("blocked (not connected)")
                    checked: modelData.allowed
                    onToggled: Halo.setGameAllowed(modelData.name, checked)
                }
            }
        }

        FormCard.FormHeader { title: Halo.tr("Running game") }
        FormCard.FormCard {
            FormCard.FormTextDelegate {
                text: currentGame.appid === "" ? Halo.tr("No Steam game open") : currentGame.name + " (" + currentGame.appid + ")"
                description: currentGame.appid === "" ? Halo.tr("When one opens, you can give it its color or effect here.")
                    : (currentProfile ? Halo.tr("When it opens:") + " " + describe(currentProfile.value) : Halo.tr("No profile: the usual lights stay"))
            }
            FormCard.FormComboBoxDelegate {
                displayMode: FormCard.FormComboBoxDelegate.ComboBox // popup list: a gamepad can drive it
                Keys.onLeftPressed: decrementCurrentIndex()
                Keys.onRightPressed: incrementCurrentIndex()
                id: currentChoice
                visible: currentGame.appid !== ""
                text: Halo.tr("When it opens")
                model: choices.map(c => c.label)
                currentIndex: 0
            }
            FormCard.FormButtonDelegate {
                visible: currentGame.appid !== ""
                text: Halo.tr("Save for {game}").replace("{game}", currentGame.name)
                icon.name: "document-save"
                onClicked: Halo.setProfile(currentGame.appid, choices[currentChoice.currentIndex].value)
            }
            FormCard.FormButtonDelegate {
                visible: currentProfile !== undefined
                text: Halo.tr("Remove the profile")
                icon.name: "edit-delete"
                onClicked: Halo.setProfile(currentGame.appid, "remove")
            }
        }

        FormCard.FormHeader { title: Halo.tr("Per-game profiles") }
        FormCard.FormCard {
            FormCard.FormTextDelegate {
                visible: profiles.length === 0
                text: Halo.tr("No profiles")
                description: Halo.tr("A profile sets a color or an effect when the game opens and brings the usual lights back when it closes. Games that send lights paint on top.")
            }
            Repeater {
                model: profiles
                FormCard.FormTextDelegate {
                    required property var modelData
                    text: modelData.name
                    description: Halo.tr("When it opens:") + " " + describe(modelData.value)
                    trailing: QQC2.ToolButton {
                        icon.name: "edit-delete"
                        text: Halo.tr("Remove")
                        display: QQC2.AbstractButton.IconOnly
                        onClicked: Halo.setProfile(modelData.appid, "remove")
                    }
                }
            }
            FormCard.FormComboBoxDelegate {
                displayMode: FormCard.FormComboBoxDelegate.ComboBox // popup list: a gamepad can drive it
                Keys.onLeftPressed: decrementCurrentIndex()
                Keys.onRightPressed: incrementCurrentIndex()
                id: newGame
                text: Halo.tr("Game")
                model: installed.map(g => g.name)
                currentIndex: 0
            }
            FormCard.FormComboBoxDelegate {
                displayMode: FormCard.FormComboBoxDelegate.ComboBox // popup list: a gamepad can drive it
                Keys.onLeftPressed: decrementCurrentIndex()
                Keys.onRightPressed: incrementCurrentIndex()
                id: newChoice
                text: Halo.tr("When it opens")
                model: choices.map(c => c.label)
                currentIndex: 0
            }
            FormCard.FormButtonDelegate {
                enabled: installed.length > 0
                text: Halo.tr("Add the profile")
                icon.name: "list-add"
                onClicked: Halo.setProfile(installed[newGame.currentIndex].appid, choices[newChoice.currentIndex].value)
            }
        }

        FormCard.FormHeader { title: Halo.tr("Bridge in Proton games") }
        FormCard.FormCard {
            FormCard.FormTextDelegate {
                visible: prefixes.length === 0
                text: Halo.tr("No Proton prefixes")
                description: Halo.tr("Open a Windows game once so Steam creates its prefix.")
            }
            FormCard.FormTextDelegate {
                visible: prefixes.length > 0
                text: Halo.tr("With the bridge, the game finds Chroma, GameSense and Logitech and paints the rings")
                description: Halo.tr("Copies the DLLs and coreProps into the prefix (the same as halo proton install). halo proton scan tells which SDK each game uses.")
            }
            Repeater {
                model: prefixes
                FormCard.FormSwitchDelegate {
                    required property var modelData
                    text: modelData.name
                    description: modelData.status === "foreign" ? Halo.tr("has a Chroma DLL that is not Halo's: left alone")
                        : (modelData.status === "halo" ? Halo.tr("with Halo's bridge") : Halo.tr("no bridge"))
                    enabled: modelData.status !== "foreign"
                    checked: modelData.status === "halo"
                    onToggled: Halo.setBridgeInstalled(modelData.appid, checked)
                }
            }
        }
    }
}
