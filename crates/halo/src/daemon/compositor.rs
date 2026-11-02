//! Layer mixing: each zone takes the color of the highest-priority layer covering it.
//! The base profile sits under everything and is the only thing saved.

use halo_core::{Frame, Rgb, ZONES};
use std::collections::{BTreeMap, BTreeSet};

pub type Zones = [Option<Rgb>; ZONES];

/// Daemon effects (equalizer, battery).
pub const PRIORITY_AMBIENT: i32 = 100;
/// The current game's profile (`perfil_APPID`): covers the effect; games paint on top.
pub const PRIORITY_PROFILE: i32 = 120;
/// External tools (OpenRGB clients, `halo layer`).
pub const PRIORITY_EXTERNAL: i32 = 150;
/// Games (Chroma, GameSense). All switched off together with [`Compositor::games_enabled`].
pub const PRIORITY_GAME: i32 = 200;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Layer {
    pub priority: i32,
    /// `None` = transparent in that zone.
    pub zones: Zones,
}

#[derive(Debug)]
pub struct Compositor {
    pub base: Frame,
    /// If `false`, game layers exist but are not painted.
    pub games_enabled: bool,
    /// Games (see [`game_of`]) that may not paint, lowercased.
    pub blocked: BTreeSet<String>,
    /// The screen is off: everything comes out off, games included. Layers are kept and come
    /// back as they were when it turns on.
    pub blank: bool,
    layers: BTreeMap<String, Layer>,
}

impl Compositor {
    pub fn new(base: Frame) -> Self {
        Self { base, games_enabled: true, blocked: BTreeSet::new(), blank: false, layers: BTreeMap::new() }
    }

    pub fn set_layer(&mut self, name: &str, layer: Layer) {
        self.layers.insert(name.to_string(), layer);
    }

    /// `true` if the layer existed.
    pub fn remove_layer(&mut self, name: &str) -> bool {
        self.layers.remove(name).is_some()
    }

    pub fn layer_names(&self) -> Vec<String> {
        self.layers.keys().cloned().collect()
    }

    /// A game layer is painted if games are allowed and that game is not blocked.
    fn visible(&self, name: &str, layer: &Layer) -> bool {
        layer.priority != PRIORITY_GAME
            || (self.games_enabled && !game_of(name).is_some_and(|g| self.blocked.contains(&g.to_lowercase())))
    }

    /// Games with a layer right now: (game, bridge, painted).
    pub fn games(&self) -> Vec<(String, String, bool)> {
        let mut out: Vec<_> = self
            .layers
            .iter()
            .filter(|(_, l)| l.priority == PRIORITY_GAME)
            .filter_map(|(n, l)| Some((game_of(n)?.to_string(), n.split(':').next().unwrap_or("").to_string(), self.visible(n, l))))
            .collect();
        out.sort();
        out.dedup();
        out
    }

    /// Something must be painted by software above the ambient effect: screen off, games or
    /// external tools. Otherwise the MCU can animate on its own.
    pub fn needs_software(&self) -> bool {
        self.blank || self.layers.iter().any(|(n, l)| l.priority > PRIORITY_AMBIENT && self.visible(n, l))
    }

    pub fn output(&self) -> Frame {
        if self.blank {
            return [Rgb::OFF; ZONES];
        }
        let mut layers: Vec<&Layer> = self.layers.iter().filter(|(n, l)| self.visible(n, l)).map(|(_, l)| l).collect();
        // Stable: at equal priority the last one by name wins, always the same.
        layers.sort_by_key(|l| l.priority);
        let mut out = self.base;
        for layer in layers {
            for (o, z) in out.iter_mut().zip(layer.zones) {
                if let Some(c) = z {
                    *o = c;
                }
            }
        }
        out
    }
}

/// The game of a bridge layer: `chroma:Factorio#3` → `Factorio`, `gamesense:FACTORIO` → `FACTORIO`.
pub fn game_of(layer: &str) -> Option<&str> {
    let (_, rest) = layer.split_once(':')?;
    let game = rest.split('#').next().unwrap_or(rest);
    (!game.is_empty()).then_some(game)
}

#[cfg(test)]
mod tests {
    use super::*;

    const R: Rgb = Rgb::new(255, 0, 0);
    const G: Rgb = Rgb::new(0, 255, 0);
    const B: Rgb = Rgb::new(0, 0, 255);
    const W: Rgb = Rgb::new(255, 255, 255);

    #[test]
    fn base_alone() {
        assert_eq!(Compositor::new([R; ZONES]).output(), [R; ZONES]);
    }

    #[test]
    fn priority_and_transparency() {
        let mut c = Compositor::new([R; ZONES]);
        c.set_layer("game", Layer { priority: 200, zones: [Some(G), None, Some(G), None] });
        c.set_layer("eq", Layer { priority: 100, zones: [Some(B), Some(B), None, None] });
        assert_eq!(c.output(), [G, B, G, R]);
        c.set_layer("notice", Layer { priority: 300, zones: [None, None, None, Some(W)] });
        assert_eq!(c.output(), [G, B, G, W]);
        assert!(c.remove_layer("game"));
        assert!(!c.remove_layer("game"));
        assert_eq!(c.output(), [B, B, R, W]);
    }

    #[test]
    fn games_switch() {
        let mut c = Compositor::new([R; ZONES]);
        c.set_layer("chroma:game", Layer { priority: PRIORITY_GAME, zones: [Some(G); ZONES] });
        c.set_layer("eq", Layer { priority: PRIORITY_AMBIENT, zones: [Some(B), None, None, None] });
        assert_eq!(c.output(), [G; ZONES]);
        c.games_enabled = false;
        assert_eq!(c.output(), [B, R, R, R]);
    }

    #[test]
    fn blocked_games_do_not_paint() {
        let mut c = Compositor::new([R; ZONES]);
        c.set_layer("chroma:Factorio#3", Layer { priority: PRIORITY_GAME, zones: [Some(G); ZONES] });
        c.set_layer("gamesense:OTHER", Layer { priority: PRIORITY_GAME, zones: [None, None, None, Some(B)] });
        assert_eq!(c.output(), [G, G, G, B]);
        c.blocked.insert("factorio".into());
        assert_eq!(c.output(), [R, R, R, B]);
        assert!(c.needs_software(), "the other game still paints");
        assert_eq!(c.games(), vec![("Factorio".into(), "chroma".into(), false), ("OTHER".into(), "gamesense".into(), true)]);
        c.blocked.insert("other".into());
        assert_eq!(c.output(), [R; ZONES]);
        assert!(!c.needs_software());
        assert_eq!(game_of("logitech:A Game#7"), Some("A Game"));
        assert_eq!(game_of("eq"), None);
        assert_eq!(game_of("chroma:#rest1"), None);
    }

    #[test]
    fn software_needed_only_above_ambient() {
        let mut c = Compositor::new([R; ZONES]);
        assert!(!c.needs_software());
        c.set_layer("eq", Layer { priority: PRIORITY_AMBIENT, zones: [Some(B); ZONES] });
        assert!(!c.needs_software());
        c.set_layer("chroma:game", Layer { priority: PRIORITY_GAME, zones: [Some(G); ZONES] });
        assert!(c.needs_software());
        c.games_enabled = false;
        assert!(!c.needs_software());
        c.blank = true;
        assert!(c.needs_software());
    }

    #[test]
    fn blank_screen_turns_everything_off_and_back() {
        let mut c = Compositor::new([R; ZONES]);
        c.set_layer("chroma:game", Layer { priority: PRIORITY_GAME, zones: [Some(G), None, None, None] });
        c.blank = true;
        assert_eq!(c.output(), [Rgb::OFF; ZONES]);
        c.blank = false;
        assert_eq!(c.output(), [G, R, R, R]);
    }
}
