//! The `Halo` object QML sees (singleton of the `org.halo.gui` module): the daemon's state as
//! properties and the actions as invokables. `refresh` reads everything again; each action calls
//! the daemon and then refreshes. `tr` gives QML the translated text for a key.

#[cxx_qt::bridge]
pub mod qobject {
    unsafe extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
        include!("cxx-qt-lib/qstringlist.h");
        type QStringList = cxx_qt_lib::QStringList;
    }

    extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[qml_singleton]
        #[qproperty(bool, connected)]
        #[qproperty(QString, error)]
        #[qproperty(QString, backend)]
        #[qproperty(QStringList, frame)]
        #[qproperty(QStringList, base)]
        #[qproperty(i32, brightness)]
        #[qproperty(QString, effect)]
        #[qproperty(QString, speed)]
        #[qproperty(bool, hardware)]
        #[qproperty(QStringList, effects)]
        #[qproperty(bool, games_enabled, cxx_name = "gamesEnabled")]
        #[qproperty(QString, games_json, cxx_name = "gamesJson")]
        #[qproperty(QString, config_json, cxx_name = "configJson")]
        #[qproperty(QStringList, layers)]
        #[qproperty(QString, current_game_json, cxx_name = "currentGameJson")]
        #[qproperty(QString, profiles_json, cxx_name = "profilesJson")]
        #[qproperty(QString, installed_json, cxx_name = "installedJson")]
        #[qproperty(QString, prefixes_json, cxx_name = "prefixesJson")]
        type Halo = super::HaloRust;

        #[qinvokable]
        fn tr(self: &Halo, key: &QString) -> QString;
        #[qinvokable]
        fn refresh(self: Pin<&mut Self>);
        #[qinvokable]
        #[cxx_name = "setZones"]
        fn set_zones(self: Pin<&mut Self>, colors: &QStringList);
        #[qinvokable]
        #[cxx_name = "applyBrightness"]
        fn apply_brightness(self: Pin<&mut Self>, level: i32);
        #[qinvokable]
        #[cxx_name = "startEffect"]
        fn start_effect(self: Pin<&mut Self>, name: &QString);
        #[qinvokable]
        #[cxx_name = "stopEffect"]
        fn stop_effect(self: Pin<&mut Self>);
        #[qinvokable]
        #[cxx_name = "applySpeed"]
        fn apply_speed(self: Pin<&mut Self>, speed: &QString);
        #[qinvokable]
        #[cxx_name = "applyGamesEnabled"]
        fn apply_games_enabled(self: Pin<&mut Self>, on: bool);
        #[qinvokable]
        #[cxx_name = "setGameAllowed"]
        fn set_game_allowed(self: Pin<&mut Self>, game: &QString, allowed: bool);
        #[qinvokable]
        #[cxx_name = "setConfig"]
        fn set_config(self: Pin<&mut Self>, key: &QString, value: &QString);
        #[qinvokable]
        #[cxx_name = "setProfile"]
        fn set_profile(self: Pin<&mut Self>, appid: &QString, value: &QString);
        #[qinvokable]
        #[cxx_name = "setBridgeInstalled"]
        fn set_bridge_installed(self: Pin<&mut Self>, appid: &QString, installed: bool);
    }
}

use crate::daemon::Daemon;
use core::pin::Pin;
use cxx_qt::CxxQtType;
use cxx_qt_lib::{QList, QString, QStringList};
use halo_core::tr;

#[derive(Default)]
pub struct HaloRust {
    connected: bool,
    error: QString,
    backend: QString,
    frame: QStringList,
    base: QStringList,
    brightness: i32,
    effect: QString,
    speed: QString,
    hardware: bool,
    effects: QStringList,
    games_enabled: bool,
    games_json: QString,
    config_json: QString,
    layers: QStringList,
    current_game_json: QString,
    profiles_json: QString,
    installed_json: QString,
    prefixes_json: QString,
    daemon: Option<Daemon>,
}

fn list(items: &[String]) -> QStringList {
    let mut l = QList::<QString>::default();
    for s in items {
        l.append(QString::from(s));
    }
    QStringList::from(&l)
}

fn strings(l: &QStringList) -> Vec<String> {
    QList::<QString>::from(l).iter().map(|s| s.to_string()).collect()
}

impl qobject::Halo {
    /// The translated text for `key` (see `halo_core::i18n`).
    pub fn tr(&self, key: &QString) -> QString {
        QString::from(&tr!(&key.to_string()))
    }

    /// Connects if needed and reads the daemon's state again.
    pub fn refresh(mut self: Pin<&mut Self>) {
        if self.daemon.is_none() {
            match Daemon::connect() {
                Ok(d) => self.as_mut().rust_mut().daemon = Some(d),
                Err(_) => {
                    self.as_mut().set_connected(false);
                    self.set_error(QString::from(&tr!("The Halo daemon is not running: systemctl --user start halo")));
                    return;
                }
            }
        }
        let snap = self.daemon.as_ref().expect("just connected").snapshot();
        let snap = match snap {
            Ok(s) => s,
            Err(e) => {
                self.as_mut().rust_mut().daemon = None;
                self.as_mut().set_connected(false);
                self.set_error(QString::from(&tr!("Lost the daemon: {error}", error = e)));
                return;
            }
        };
        let games: Vec<serde_json::Value> = snap
            .games
            .iter()
            .map(|(g, b, c, a)| serde_json::json!({ "name": g, "bridge": b, "connected": c, "allowed": a }))
            .collect();
        let config: serde_json::Map<String, serde_json::Value> =
            snap.config.iter().map(|(k, v)| (k.clone(), serde_json::Value::String(v.clone()))).collect();
        self.as_mut().set_connected(true);
        self.as_mut().set_error(QString::default());
        self.as_mut().set_backend(QString::from(&snap.backend));
        self.as_mut().set_frame(list(&snap.frame));
        self.as_mut().set_base(list(&snap.base));
        self.as_mut().set_brightness(snap.brightness as i32);
        self.as_mut().set_effect(QString::from(&snap.effect));
        self.as_mut().set_speed(QString::from(&snap.speed));
        self.as_mut().set_hardware(snap.hardware);
        self.as_mut().set_effects(list(&snap.effects));
        self.as_mut().set_games_enabled(snap.games_enabled);
        self.as_mut().set_games_json(QString::from(&serde_json::Value::Array(games).to_string()));
        self.as_mut().set_config_json(QString::from(&serde_json::Value::Object(config).to_string()));
        self.as_mut().set_layers(list(&snap.layers));
        let (appid, name) = &snap.current_game;
        self.as_mut().set_current_game_json(QString::from(&serde_json::json!({ "appid": appid, "name": name }).to_string()));
        let profiles: Vec<_> = snap.profiles.iter().map(|(a, n, v)| serde_json::json!({ "appid": a, "name": n, "value": v })).collect();
        self.as_mut().set_profiles_json(QString::from(&serde_json::Value::Array(profiles).to_string()));
        let installed: Vec<_> = snap.installed.iter().map(|(a, n)| serde_json::json!({ "appid": a, "name": n })).collect();
        self.as_mut().set_installed_json(QString::from(&serde_json::Value::Array(installed).to_string()));
        let prefixes: Vec<_> = snap.prefixes.iter().map(|(a, n, s)| serde_json::json!({ "appid": a, "name": n, "status": s })).collect();
        self.set_prefixes_json(QString::from(&serde_json::Value::Array(prefixes).to_string()));
    }

    fn act<B>(mut self: Pin<&mut Self>, method: &str, body: &B)
    where
        B: serde::ser::Serialize + zbus::zvariant::DynamicType,
    {
        let result = match self.daemon.as_ref() {
            Some(d) => d.call(method, body),
            None => Err(tr!("No daemon")),
        };
        if let Err(e) = result {
            self.as_mut().set_error(QString::from(&e));
        }
        self.refresh();
    }

    pub fn set_zones(self: Pin<&mut Self>, colors: &QStringList) {
        self.act("SetZones", &(strings(colors),));
    }

    pub fn apply_brightness(self: Pin<&mut Self>, level: i32) {
        self.act("SetBrightness", &(level.clamp(0, 3) as u8,));
    }

    pub fn start_effect(self: Pin<&mut Self>, name: &QString) {
        self.act("StartEffect", &(name.to_string(),));
    }

    pub fn stop_effect(self: Pin<&mut Self>) {
        self.act("StopEffect", &());
    }

    pub fn apply_speed(self: Pin<&mut Self>, speed: &QString) {
        self.act("SetEffectSpeed", &(speed.to_string(),));
    }

    pub fn apply_games_enabled(self: Pin<&mut Self>, on: bool) {
        self.act("SetGamesEnabled", &(on,));
    }

    pub fn set_game_allowed(self: Pin<&mut Self>, game: &QString, allowed: bool) {
        self.act("SetGameAllowed", &(game.to_string(), allowed));
    }

    pub fn set_config(self: Pin<&mut Self>, key: &QString, value: &QString) {
        self.act("SetConfig", &(key.to_string(), value.to_string()));
    }

    /// A Steam game's profile: one color, one per zone, an effect, "off" or "remove".
    pub fn set_profile(self: Pin<&mut Self>, appid: &QString, value: &QString) {
        self.act("SetConfig", &(format!("profile_{appid}"), value.to_string()));
    }

    /// Puts or removes Halo's bridge in the game's Proton prefix.
    pub fn set_bridge_installed(self: Pin<&mut Self>, appid: &QString, installed: bool) {
        self.act("SetBridgeInstalled", &(appid.to_string(), installed));
    }
}
