//! `org.halo.Halo1` on the session bus. Used by the CLI, the GUI, the Decky plugin and the bridges.

use super::compositor::{Layer, PRIORITY_PROFILE, Zones};
use super::config::Profile;
use super::device::{Core, hex};
use super::effects::{self, anim};
use super::steam;
use halo_core::{Frame, Rgb, ZONES, tr};
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use zbus::fdo;
use zbus::interface;
use zbus::object_server::SignalEmitter;

pub const NAME: &str = "org.halo.Halo1";
pub const PATH: &str = "/org/halo/Halo1";

pub struct Halo {
    core: Arc<Mutex<Core>>,
    effect: Mutex<Option<effects::Running>>,
    /// The screen watcher (`screen_off`); `None` if removed.
    screen: Mutex<Option<JoinHandle<()>>>,
    game: Mutex<GameState>,
}

/// The layer of the current game's profile.
const PROFILE_LAYER: &str = "profile";

#[derive(Default)]
struct GameState {
    /// The current Steam game (reported by `steam::watch`).
    current: Option<steam::Game>,
    /// The profile applied right now, if the current game has one.
    applied: Option<(String, Profile)>,
}

impl Halo {
    /// Starts the effect saved in the configuration, if any.
    pub async fn new(core: Arc<Mutex<Core>>) -> Self {
        let saved = {
            let c = core.lock().await;
            c.config.effect.map(|k| (k, c.config.speed))
        };
        let mut effect = None;
        if let Some((k, speed)) = saved {
            effect = effects::start(k.name(), speed, core.clone()).await;
        }
        let watch = core.lock().await.config.screen_off;
        let screen = watch.then(|| tokio::spawn(super::power::watch_screen(core.clone())));
        Self { core, effect: Mutex::new(effect), screen: Mutex::new(screen), game: Mutex::new(GameState::default()) }
    }

    /// The current game changed (or closed): its profile is applied or removed.
    pub(crate) async fn set_current_game(&self, game: Option<steam::Game>) {
        let mut st = self.game.lock().await;
        st.current = game;
        self.apply_profile(&mut st).await;
    }

    /// Leaves the right profile in place: the current game's if it has one, otherwise the usual.
    async fn apply_profile(&self, st: &mut GameState) {
        let wanted = match &st.current {
            Some(g) => self.core.lock().await.config.profiles.get(&g.appid).cloned().map(|p| (g.appid.clone(), p)),
            None => None,
        };
        if st.applied == wanted {
            return;
        }
        let had = st.applied.take();
        if had.is_some() {
            self.core.lock().await.comp.remove_layer(PROFILE_LAYER);
        }
        let speed = self.core.lock().await.config.speed;
        let mut effect = self.effect.lock().await;
        if let Some(old) = effect.take() {
            effects::stop(old, &self.core).await;
        }
        match &wanted {
            Some((appid, Profile::Colors(frame))) => {
                eprintln!("halo: profile of {appid}: {}", Profile::Colors(*frame));
                let mut core = self.core.lock().await;
                core.comp.set_layer(PROFILE_LAYER, Layer { priority: PRIORITY_PROFILE, zones: frame.map(Some) });
                if let Err(e) = core.push() {
                    eprintln!("halo: profile: {e}");
                }
            }
            Some((appid, Profile::Effect(name))) => {
                eprintln!("halo: profile of {appid}: effect {name}");
                *effect = effects::start(name, speed, self.core.clone()).await;
            }
            None => {
                // Back to the usual: the saved effect, or the base color.
                let saved = self.core.lock().await.config.effect;
                match saved {
                    Some(k) => *effect = effects::start(k.name(), speed, self.core.clone()).await,
                    None => {
                        if let Err(e) = self.core.lock().await.push() {
                            eprintln!("halo: removing the profile: {e}");
                        }
                    }
                }
            }
        }
        st.applied = wanted;
    }

    /// Restarts the running effect (e.g. something the MCU has programmed changed).
    async fn restart_effect(&self, only_if: impl Fn(&effects::Running) -> bool) {
        let mut effect = self.effect.lock().await;
        let Some(name) = effect.as_ref().filter(|r| only_if(r)).map(|r| r.name.clone()) else { return };
        let speed = self.core.lock().await.config.speed;
        effects::stop(effect.take().expect("checked above"), &self.core).await;
        *effect = effects::start(&name, speed, self.core.clone()).await;
    }

    /// Starts or removes the screen watcher according to the configuration.
    async fn apply_screen_watch(&self) {
        let want = self.core.lock().await.config.screen_off;
        let mut screen = self.screen.lock().await;
        match (want, screen.is_some()) {
            (true, false) => *screen = Some(tokio::spawn(super::power::watch_screen(self.core.clone()))),
            (false, true) => {
                screen.take().expect("checked above").abort();
                // If it was removed with the screen off, do not leave the rings black.
                let mut core = self.core.lock().await;
                if core.comp.blank {
                    core.comp.blank = false;
                    let _ = core.push();
                }
            }
            _ => {}
        }
    }

    /// Saves which light effect stays on (the equalizer is not saved).
    async fn remember(&self, effect: Option<anim::Kind>) -> fdo::Result<()> {
        let mut core = self.core.lock().await;
        core.config.effect = effect;
        core.save_config().map_err(io)
    }
}

fn bad(msg: String) -> fdo::Error {
    fdo::Error::InvalidArgs(msg)
}

fn io(e: std::io::Error) -> fdo::Error {
    fdo::Error::Failed(e.to_string())
}

/// 1 color (all zones) or one per zone. With `transparent`, "" leaves the zone uncovered.
fn parse_zones(colors: &[String], transparent: bool) -> fdo::Result<Zones> {
    let parsed: Vec<Option<Rgb>> = colors
        .iter()
        .map(|c| match c.as_str() {
            "" if transparent => Ok(None),
            _ => Rgb::parse(c).map(Some).ok_or_else(|| bad(tr!("Invalid color: {value}", value = c))),
        })
        .collect::<fdo::Result<_>>()?;
    match parsed[..] {
        [c] => Ok([c; ZONES]),
        _ if parsed.len() == ZONES => Ok(parsed.try_into().expect("length checked")),
        _ => Err(bad(tr!("Expected 1 or {zones} colors, got {count}", zones = ZONES, count = parsed.len()))),
    }
}

fn solid(z: Zones) -> Frame {
    z.map(|c| c.expect("no transparency"))
}

#[interface(name = "org.halo.Halo1")]
impl Halo {
    /// Changes the base color (the saved one): 1 color or one per zone, in hex or by name.
    async fn set_zones(&self, colors: Vec<String>, #[zbus(signal_emitter)] em: SignalEmitter<'_>) -> fdo::Result<()> {
        let frame = solid(parse_zones(&colors, false)?);
        self.core.lock().await.set_base(frame).map_err(io)?;
        // The MCU has the base color programmed (breathing): it has to be reprogrammed.
        self.restart_effect(|r| r.is_hardware()).await;
        self.base_changed(&em).await?;
        self.frame_changed(&em).await?;
        Ok(())
    }

    /// Hardware brightness 0-3.
    async fn set_brightness(&self, level: u8, #[zbus(signal_emitter)] em: SignalEmitter<'_>) -> fdo::Result<()> {
        if level > 3 {
            return Err(bad(tr!("Brightness goes from 0 to 3")));
        }
        self.core.lock().await.set_brightness(level).map_err(io)?;
        self.brightness_changed(&em).await?;
        Ok(())
    }

    /// Sets or replaces a layer on top of the base color ("" = transparent in that zone).
    async fn set_layer(
        &self,
        name: String,
        priority: i32,
        colors: Vec<String>,
        #[zbus(signal_emitter)] em: SignalEmitter<'_>,
    ) -> fdo::Result<()> {
        let zones = parse_zones(&colors, true)?;
        {
            let mut core = self.core.lock().await;
            core.comp.set_layer(&name, Layer { priority, zones });
            core.push().map_err(io)?;
        }
        self.layers_changed(&em).await?;
        self.frame_changed(&em).await?;
        Ok(())
    }

    async fn clear_layer(&self, name: String, #[zbus(signal_emitter)] em: SignalEmitter<'_>) -> fdo::Result<bool> {
        let existed = {
            let mut core = self.core.lock().await;
            let existed = core.comp.remove_layer(&name);
            core.push().map_err(io)?;
            existed
        };
        if existed {
            self.layers_changed(&em).await?;
            self.frame_changed(&em).await?;
        }
        Ok(existed)
    }

    /// Reopens the device and resends everything (brightness and frame).
    async fn restore(&self, #[zbus(signal_emitter)] em: SignalEmitter<'_>) -> fdo::Result<()> {
        self.core.lock().await.reclaim().map_err(io)?;
        self.backend_changed(&em).await?;
        Ok(())
    }

    /// Hands control back to the firmware until the next change.
    async fn release(&self) -> fdo::Result<()> {
        self.core.lock().await.release().map_err(io)
    }

    /// "Games control the lights" (Chroma, GameSense). When off, their layers are not painted.
    async fn set_games_enabled(&self, on: bool, #[zbus(signal_emitter)] em: SignalEmitter<'_>) -> fdo::Result<()> {
        self.core.lock().await.set_games(on).map_err(io)?;
        self.games_enabled_changed(&em).await?;
        self.frame_changed(&em).await?;
        Ok(())
    }

    #[zbus(property)]
    async fn games_enabled(&self) -> bool {
        self.core.lock().await.comp.games_enabled
    }

    /// Connected games and blocked ones: (game, bridge, connected, may paint).
    #[zbus(property)]
    async fn games(&self) -> Vec<(String, String, bool, bool)> {
        let core = self.core.lock().await;
        let mut out: Vec<_> = core.comp.games().into_iter().map(|(g, b, v)| (g, b, true, v)).collect();
        for g in &core.config.blocked {
            if !out.iter().any(|(n, ..)| n.eq_ignore_ascii_case(g)) {
                out.push((g.clone(), String::new(), false, false));
            }
        }
        out
    }

    /// Allows or blocks a game by its name (the one `Games` shows), case-insensitive.
    async fn set_game_allowed(&self, game: String, allowed: bool, #[zbus(signal_emitter)] em: SignalEmitter<'_>) -> fdo::Result<()> {
        let game = game.trim();
        if game.is_empty() {
            return Err(bad(tr!("Missing the game name")));
        }
        self.core.lock().await.set_game_allowed(game, allowed).map_err(io)?;
        self.games_changed(&em).await?;
        self.frame_changed(&em).await?;
        Ok(())
    }

    /// The configuration as (key, value) pairs for display.
    async fn config(&self) -> Vec<(String, String)> {
        super::config::entries(&self.core.lock().await.config)
    }

    /// Changes a halo.conf option (see `halo config`) and applies it immediately.
    async fn set_config(&self, key: String, value: String) -> fdo::Result<()> {
        {
            let mut core = self.core.lock().await;
            let mut next = core.config.clone();
            super::config::set(&mut next, &key, &value).map_err(bad)?;
            core.config = next;
            core.save_config().map_err(io)?;
        }
        self.apply_screen_watch().await;
        // Software light effects read their colors on every frame; music and MCU effects take them at
        // start, and `hardware_effects` decides who animates.
        let changed = key.strip_prefix("colors_").or_else(|| key.strip_prefix("reverse_"));
        self.restart_effect(|r| {
            key == "hardware_effects" || (changed == Some(r.name.as_str()) && (effects::is_audio(&r.name) || r.is_hardware()))
        })
        .await;
        if key.starts_with("profile_") {
            let mut st = self.game.lock().await;
            self.apply_profile(&mut st).await;
        }
        Ok(())
    }

    /// The current Steam game: (AppID, name), or ("", "") if none.
    #[zbus(property)]
    async fn current_game(&self) -> (String, String) {
        self.game.lock().await.current.as_ref().map_or_else(Default::default, |g| (g.appid.clone(), g.name.clone()))
    }

    /// Per-game profiles: (AppID, name, value), with the value as it goes in `halo.conf`.
    #[zbus(property)]
    async fn profiles(&self) -> Vec<(String, String, String)> {
        let libs = steam::libraries();
        let core = self.core.lock().await;
        core.config.profiles.iter().map(|(a, p)| (a.clone(), steam::game_name(&libs, a), p.to_string())).collect()
    }

    /// Installed Steam games: (AppID, name).
    #[zbus(property)]
    async fn installed_games(&self) -> Vec<(String, String)> {
        steam::installed(&steam::libraries())
    }

    /// Proton prefixes and whether they have the bridge: (AppID, name, status), with status
    /// `halo` (installed), `none` (missing) or `foreign` (a Chroma DLL that is not ours).
    #[zbus(property)]
    async fn prefixes(&self) -> Vec<(String, String, String)> {
        use crate::proton::{self, Status};
        proton::find_prefixes()
            .into_iter()
            .map(|p| {
                let st = match proton::status(&p.windows) {
                    Status::Installed => "halo",
                    Status::Missing => "none",
                    Status::Foreign => "foreign",
                };
                (p.appid.clone(), p.name.unwrap_or_else(|| p.appid.clone()), st.into())
            })
            .collect()
    }

    /// Installs or removes the bridge (Chroma and Logitech DLLs, GameSense coreProps) in a game's
    /// Proton prefix; same as `halo proton install|remove APPID`.
    async fn set_bridge_installed(&self, appid: String, installed: bool) -> fdo::Result<()> {
        use crate::proton;
        let prefix = proton::find_prefixes().into_iter().find(|p| p.appid == appid).ok_or_else(|| bad(tr!("No Proton prefix for {appid} (halo proton list)", appid = appid)))?;
        let r = if installed { proton::install(&prefix.windows, &proton::dll_dir()) } else { proton::remove(&prefix.windows).map(|_| ()) };
        r.map_err(|e| fdo::Error::Failed(format!("{}: {e}", prefix.name.unwrap_or(appid))))
    }

    /// Starts an effect (see the `Effects` property); replaces whichever was running. Light effects
    /// are saved and come back on startup.
    async fn start_effect(&self, name: String, #[zbus(signal_emitter)] em: SignalEmitter<'_>) -> fdo::Result<()> {
        let kind = anim::Kind::parse(&name);
        if kind.is_none() && !effects::is_audio(&name) {
            return Err(bad(tr!("Unknown effect {name}; available: {list}", name = name, list = effects::available().join(", "))));
        }
        let speed = self.core.lock().await.config.speed;
        let mut effect = self.effect.lock().await;
        if let Some(old) = effect.take() {
            effects::stop(old, &self.core).await;
        }
        *effect = effects::start(&name, speed, self.core.clone()).await;
        drop(effect);
        self.remember(kind).await?;
        self.effect_changed(&em).await?;
        Ok(())
    }

    /// Light effect speed: slow, medium or fast. If one is on, it restarts with the new speed.
    async fn set_effect_speed(&self, speed: String, #[zbus(signal_emitter)] em: SignalEmitter<'_>) -> fdo::Result<()> {
        let speed = anim::Speed::parse(&speed).ok_or_else(|| bad(tr!("Unknown speed {value} (slow, medium, fast)", value = speed)))?;
        {
            let mut core = self.core.lock().await;
            core.config.speed = speed;
            core.save_config().map_err(io)?;
        }
        let mut effect = self.effect.lock().await;
        if let Some(name) = effect.as_ref().filter(|r| anim::Kind::parse(&r.name).is_some()).map(|r| r.name.clone()) {
            effects::stop(effect.take().expect("checked above"), &self.core).await;
            *effect = effects::start(&name, speed, self.core.clone()).await;
        }
        drop(effect);
        self.effect_speed_changed(&em).await?;
        Ok(())
    }

    /// `true` if the running effect is animated by the MCU (the daemon spends no CPU).
    #[zbus(property)]
    async fn effect_hardware(&self) -> bool {
        self.effect.lock().await.as_ref().is_some_and(|r| r.is_hardware())
    }

    #[zbus(property)]
    async fn effect_speed(&self) -> String {
        self.core.lock().await.config.speed.name().into()
    }

    /// Stops the effect; `false` if there was none.
    async fn stop_effect(&self, #[zbus(signal_emitter)] em: SignalEmitter<'_>) -> fdo::Result<bool> {
        let Some(old) = self.effect.lock().await.take() else {
            return Ok(false);
        };
        effects::stop(old, &self.core).await;
        self.remember(None).await?;
        self.effect_changed(&em).await?;
        self.frame_changed(&em).await?;
        Ok(true)
    }

    /// The running effect, or "" if none.
    #[zbus(property)]
    async fn effect(&self) -> String {
        match self.effect.lock().await.as_ref() {
            Some(r) if r.is_alive() => r.name.clone(),
            _ => String::new(),
        }
    }

    #[zbus(property)]
    async fn effects(&self) -> Vec<String> {
        effects::available().into_iter().map(String::from).collect()
    }

    /// What the rings show right now.
    #[zbus(property)]
    async fn frame(&self) -> Vec<String> {
        hex(&self.core.lock().await.comp.output())
    }

    /// The saved base color.
    #[zbus(property)]
    async fn base(&self) -> Vec<String> {
        hex(&self.core.lock().await.comp.base)
    }

    #[zbus(property)]
    async fn brightness(&self) -> u8 {
        self.core.lock().await.brightness
    }

    #[zbus(property)]
    async fn backend(&self) -> String {
        self.core.lock().await.backend().to_string()
    }

    #[zbus(property)]
    async fn layers(&self) -> Vec<String> {
        self.core.lock().await.comp.layer_names()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &[&str]) -> Vec<String> {
        s.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn zones_parsing() {
        let r = Rgb::new(255, 0, 0);
        assert_eq!(parse_zones(&v(&["red"]), false).unwrap(), [Some(r); ZONES]);
        assert_eq!(parse_zones(&v(&["red", "", "", "ff0000"]), true).unwrap(), [Some(r), None, None, Some(r)]);
        assert!(parse_zones(&v(&["red", ""]), true).is_err());
        assert!(parse_zones(&v(&["", "", "", ""]), false).is_err());
    }
}
