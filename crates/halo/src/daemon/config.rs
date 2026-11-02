//! Daemon preferences in `~/.config/halo/halo.conf` (`key = value`, one per line).

use super::effects::anim::{Kind, Options, Speed};
use halo_core::tr;
use halo_core::{Frame, Rgb, ZONES};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::{env, fmt, fs, io};

#[derive(Clone, Debug, PartialEq)]
pub struct Config {
    /// "Games control the lights".
    pub games: bool,
    /// The light effect started at boot. Audio effects are not saved: they depend on the music.
    pub effect: Option<Kind>,
    pub speed: Speed,
    /// For the `sun` effect; without them the time zone gives an approximation.
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
    /// Turn the rings off together with the screen.
    pub screen_off: bool,
    /// Custom colors and direction, by effect name (`colors_X`, `reverse_X`).
    pub effects: BTreeMap<String, EffectConfig>,
    /// Let the MCU animate breathing, cycle and rainbow (0 CPU). `no` forces software.
    pub hardware_effects: bool,
    /// Games that may not paint (`block = NAME`, one line per game), as written.
    pub blocked: BTreeSet<String>,
    /// What to show when each Steam game opens, by AppID (`profile_APPID`).
    pub profiles: BTreeMap<String, Profile>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct EffectConfig {
    pub colors: Vec<Rgb>,
    pub reverse: bool,
}

/// A game's profile: one color (or one per zone, or off) or an effect. While the game runs it
/// covers the usual effect; the bridges (Chroma, GameSense) paint on top.
#[derive(Clone, Debug, PartialEq)]
pub enum Profile {
    Colors(Frame),
    Effect(String),
}

impl Profile {
    /// `off`, an effect, one color or one per zone.
    pub fn parse(value: &str) -> Option<Self> {
        let value = value.trim();
        if value == "off" {
            return Some(Self::Colors([Rgb::OFF; ZONES]));
        }
        if let Some(k) = Kind::parse(value) {
            return Some(Self::Effect(k.name().into()));
        }
        if super::effects::is_audio(value) {
            return Some(Self::Effect(value.into()));
        }
        let colors: Vec<Rgb> = value.split_whitespace().map(Rgb::parse).collect::<Option<_>>()?;
        match colors[..] {
            [c] => Some(Self::Colors([c; ZONES])),
            _ if colors.len() == ZONES => Some(Self::Colors(colors.try_into().ok()?)),
            _ => None,
        }
    }
}

impl fmt::Display for Profile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Effect(name) => f.write_str(name),
            Self::Colors(frame) if *frame == [Rgb::OFF; ZONES] => f.write_str("off"),
            Self::Colors(frame) if frame.iter().all(|c| *c == frame[0]) => write!(f, "{}", frame[0]),
            Self::Colors(frame) => f.write_str(&frame.iter().map(Rgb::to_string).collect::<Vec<_>>().join(" ")),
        }
    }
}

impl Config {
    pub fn place(&self) -> (f64, f64) {
        let (lat, lon) = super::effects::sun_default_place();
        (self.latitude.unwrap_or(lat), self.longitude.unwrap_or(lon))
    }

    /// What an effect needs to paint.
    pub fn options(&self, effect: &str) -> Options {
        let e = self.effects.get(effect).cloned().unwrap_or_default();
        Options { colors: e.colors, reverse: e.reverse, place: self.place() }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self { games: true, effect: None, speed: Speed::default(), latitude: None, longitude: None, screen_off: true, effects: BTreeMap::new(), hardware_effects: true, blocked: BTreeSet::new(), profiles: BTreeMap::new() }
    }
}

pub fn path() -> PathBuf {
    env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env::var_os("HOME").unwrap_or_default()).join(".config"))
        .join("halo/halo.conf")
}

fn yes(value: &str) -> bool {
    matches!(value, "yes" | "true" | "1" | "on")
}

fn no(value: &str) -> bool {
    matches!(value, "no" | "false" | "0" | "off")
}

fn yes_no(value: bool) -> &'static str {
    if value { "yes" } else { "no" }
}

pub fn parse(text: &str) -> Config {
    let mut c = Config::default();
    for line in text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')) {
        let Some((key, value)) = line.split_once('=') else { continue };
        let value = value.trim();
        match key.trim() {
            "games" => c.games = yes(value),
            "screen_off" => c.screen_off = yes(value),
            "hardware_effects" => c.hardware_effects = yes(value),
            "block" if !value.is_empty() => {
                c.blocked.insert(value.to_string());
            }
            "effect" => c.effect = Kind::parse(value),
            "speed" => c.speed = Speed::parse(value).unwrap_or_default(),
            "latitude" => c.latitude = value.parse().ok(),
            "longitude" => c.longitude = value.parse().ok(),
            k => {
                if let Some(name) = k.strip_prefix("colors_") {
                    c.effects.entry(name.into()).or_default().colors = value.split_whitespace().filter_map(Rgb::parse).collect();
                } else if let Some(name) = k.strip_prefix("reverse_") {
                    c.effects.entry(name.into()).or_default().reverse = yes(value);
                } else if let Some(appid) = k.strip_prefix("profile_")
                    && is_appid(appid)
                    && let Some(p) = Profile::parse(value)
                {
                    c.profiles.insert(appid.into(), p);
                }
            }
        }
    }
    c
}

pub fn format(c: &Config) -> String {
    let mut out = format!(
        "# Halo daemon preferences\ngames = {}\neffect = {}\nspeed = {}\nscreen_off = {}\nhardware_effects = {}\n",
        yes_no(c.games),
        c.effect.map_or("none", Kind::name),
        c.speed.name(),
        yes_no(c.screen_off),
        yes_no(c.hardware_effects),
    );
    out += "# For the sun effect, in degrees (north and east positive).\n";
    for (key, v) in [("latitude", c.latitude), ("longitude", c.longitude)] {
        match v {
            Some(v) => out += &format!("{key} = {v}\n"),
            None => out += &format!("# {key} = \n"),
        }
    }
    if !c.blocked.is_empty() {
        out += "# Games that may not paint the rings (halo games block|allow).\n";
    }
    for g in &c.blocked {
        out += &format!("block = {g}\n");
    }
    if !c.profiles.is_empty() {
        out += "# What to show when each Steam game opens, by AppID (halo profile).\n";
    }
    for (appid, p) in &c.profiles {
        out += &format!("profile_{appid} = {p}\n");
    }
    let own: Vec<_> = c.effects.iter().filter(|(_, e)| !e.colors.is_empty() || e.reverse).collect();
    if !own.is_empty() {
        out += "# Custom colors and direction per effect (halo effect colors|direction).\n";
    }
    for (name, e) in own {
        if !e.colors.is_empty() {
            out += &format!("colors_{name} = {}\n", e.colors.iter().map(Rgb::to_string).collect::<Vec<_>>().join(" "));
        }
        if e.reverse {
            out += &format!("reverse_{name} = yes\n");
        }
    }
    out
}

/// What `halo config` shows and changes. `games`, `effect` and `speed` have their own commands,
/// and each effect's colors and direction go as `colors_X` and `reverse_X`.
pub const KEYS: &[&str] = &["screen_off", "hardware_effects", "latitude", "longitude", "colors_EFFECT", "reverse_EFFECT", "profile_APPID"];

/// A Steam AppID: digits only.
fn is_appid(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}

pub fn entries(c: &Config) -> Vec<(String, String)> {
    let deg = |v: Option<f64>| v.map_or("auto".to_string(), |v| v.to_string());
    let mut out = vec![
        ("games".into(), yes_no(c.games).into()),
        ("effect".into(), c.effect.map_or("none", Kind::name).into()),
        ("speed".into(), c.speed.name().into()),
        ("screen_off".into(), yes_no(c.screen_off).into()),
        ("hardware_effects".into(), yes_no(c.hardware_effects).into()),
        ("blocked".into(), if c.blocked.is_empty() { "none".into() } else { c.blocked.iter().cloned().collect::<Vec<_>>().join(", ") }),
        ("latitude".into(), deg(c.latitude)),
        ("longitude".into(), deg(c.longitude)),
    ];
    for (name, e) in &c.effects {
        if !e.colors.is_empty() {
            out.push((format!("colors_{name}"), e.colors.iter().map(Rgb::to_string).collect::<Vec<_>>().join(" ")));
        }
        if e.reverse {
            out.push((format!("reverse_{name}"), "yes".into()));
        }
    }
    for (appid, p) in &c.profiles {
        out.push((format!("profile_{appid}"), p.to_string()));
    }
    out
}

/// An effect name as it goes in the configuration.
fn effect_key(name: &str) -> Result<String, String> {
    let plain = Kind::parse(name).map(|k| k.name().to_string()).or_else(|| super::effects::is_audio(name).then(|| name.to_string()));
    plain.ok_or_else(|| tr!("There is no effect {name}; available: {list}", name = name, list = super::effects::available().join(", ")))
}

/// Changes one of [`KEYS`]. The error is ready to show.
pub fn set(c: &mut Config, key: &str, value: &str) -> Result<(), String> {
    let value = value.trim();
    let degrees = |max: f64| -> Result<Option<f64>, String> {
        if matches!(value, "auto" | "none" | "") {
            return Ok(None);
        }
        let v: f64 = value.parse().map_err(|_| tr!("{key} takes degrees (e.g. 20.97) or auto, not {value}", key = key, value = value))?;
        if !v.is_finite() || v.abs() > max {
            return Err(tr!("{key} goes from -{max} to {max}", key = key, max = max));
        }
        Ok(Some(v))
    };
    let switch = |value: &str| -> Result<bool, String> {
        if yes(value) {
            Ok(true)
        } else if no(value) {
            Ok(false)
        } else {
            Err(tr!("{key} takes yes or no, not {value}", key = key, value = value))
        }
    };
    match key {
        "screen_off" => c.screen_off = switch(value)?,
        "hardware_effects" => c.hardware_effects = switch(value)?,
        "latitude" => c.latitude = degrees(90.0)?,
        "longitude" => c.longitude = degrees(180.0)?,
        k if k.starts_with("colors_") => {
            let name = effect_key(&k["colors_".len()..])?;
            if !matches!(Kind::parse(&name), Some(Kind::Breathing | Kind::Strobe | Kind::Cycle | Kind::Rainbow | Kind::Comet | Kind::Tide | Kind::Starry | Kind::Star)) && name != super::effects::PULSE {
                return Err(tr!("{name} has no custom colors", name = name));
            }
            let colors = if matches!(value, "auto" | "") {
                vec![]
            } else {
                value.split_whitespace().map(|v| Rgb::parse(v).ok_or_else(|| tr!("Invalid color: {value}", value = v))).collect::<Result<_, _>>()?
            };
            c.effects.entry(name).or_default().colors = colors;
        }
        k if k.starts_with("reverse_") => {
            let name = effect_key(&k["reverse_".len()..])?;
            if !matches!(Kind::parse(&name), Some(Kind::Cycle | Kind::Rainbow | Kind::Comet | Kind::Tide)) {
                return Err(tr!("{name} has no direction to reverse", name = name));
            }
            c.effects.entry(name).or_default().reverse = match value {
                "reverse" => true,
                "normal" => false,
                v => switch(v)?,
            };
        }
        k if k.starts_with("profile_") => {
            let appid = &k["profile_".len()..];
            if !is_appid(appid) {
                return Err(tr!("profile_APPID takes a Steam AppID (digits), not {value}; `halo proton list` shows them", value = appid));
            }
            if matches!(value, "remove" | "none" | "") {
                c.profiles.remove(appid);
            } else {
                let p = Profile::parse(value).ok_or_else(|| {
                    tr!("A profile is one color, one per zone (4), an effect or off; not {value}. Effects: {list}", value = value, list = super::effects::available().join(", "))
                })?;
                c.profiles.insert(appid.into(), p);
            }
        }
        "games" => return Err(tr!("Use `halo games on|off` for that")),
        "effect" | "speed" => return Err(tr!("Use `halo effect [EFFECT] [SPEED]` for that")),
        _ => return Err(tr!("Unknown setting {key}; these can be changed: {list}", key = key, list = KEYS.join(", "))),
    }
    Ok(())
}

pub fn load() -> Config {
    fs::read_to_string(path()).map(|t| parse(&t)).unwrap_or_default()
}

pub fn save(c: &Config) -> io::Result<()> {
    let p = path();
    fs::create_dir_all(p.parent().expect("the path has a parent"))?;
    fs::write(p, format(c))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        assert_eq!(parse(""), Config::default());
        let off = Config { games: false, ..Default::default() };
        assert_eq!(parse(&format(&off)), off);
        let fx = Config { effect: Some(Kind::Comet), speed: Speed::Fast, ..Default::default() };
        assert_eq!(parse(&format(&fx)), fx);
        assert_eq!(parse("effect = Rainbow\nspeed = FAST").effect, Some(Kind::Rainbow));
        let here = Config { latitude: Some(20.97), longitude: Some(-89.62), ..Default::default() };
        assert_eq!(parse(&format(&here)), here);
        assert_eq!(here.place(), (20.97, -89.62));
        assert!(parse("# x\n games = yes \n").games);
        let lit = Config { screen_off: false, ..Default::default() };
        assert_eq!(parse(&format(&lit)), lit);
        let soft = Config { hardware_effects: false, ..Default::default() };
        assert_eq!(parse(&format(&soft)), soft);
        let blocked = Config { blocked: ["Factorio".to_string(), "Some Game".to_string()].into(), ..Default::default() };
        assert_eq!(parse(&format(&blocked)), blocked);
        assert!(parse("").hardware_effects);
        assert!(parse("").screen_off);
    }

    #[test]
    fn set_validates() {
        let mut c = Config::default();
        set(&mut c, "latitude", "20.97").unwrap();
        set(&mut c, "longitude", "-89.62").unwrap();
        assert_eq!((c.latitude, c.longitude), (Some(20.97), Some(-89.62)));
        assert!(set(&mut c, "latitude", "91").is_err());
        assert!(set(&mut c, "longitude", "NaN").is_err());
        assert!(set(&mut c, "latitude", "north").is_err());
        assert_eq!(c.latitude, Some(20.97), "an error changes nothing");
        set(&mut c, "latitude", "auto").unwrap();
        assert_eq!(c.latitude, None);
        set(&mut c, "screen_off", "no").unwrap();
        assert!(!c.screen_off);
        assert!(set(&mut c, "screen_off", "maybe").is_err());
        assert!(set(&mut c, "games", "no").is_err());
        assert!(set(&mut c, "color", "red").is_err());
        // Colors and direction per effect, round-tripped through the file.
        set(&mut c, "colors_tide", "red 00ff00").unwrap();
        set(&mut c, "reverse_comet", "yes").unwrap();
        assert_eq!(c.options("tide").colors, vec![Rgb::new(255, 0, 0), Rgb::new(0, 255, 0)]);
        assert!(c.options("comet").reverse && !c.options("tide").reverse);
        assert_eq!(parse(&format(&c)), c);
        set(&mut c, "reverse_comet", "normal").unwrap();
        assert!(!c.options("comet").reverse);
        set(&mut c, "reverse_comet", "reverse").unwrap();
        assert!(c.options("comet").reverse);
        assert!(set(&mut c, "colors_clock", "red").is_err(), "the clock has no custom colors");
        assert!(set(&mut c, "reverse_breathing", "yes").is_err());
        assert!(set(&mut c, "colors_tide", "red loud-purple").is_err());
        assert!(set(&mut c, "colors_nothing", "red").is_err());
        set(&mut c, "colors_tide", "auto").unwrap();
        assert!(c.options("tide").colors.is_empty());
    }

    #[test]
    fn profiles() {
        let r = Rgb::new(255, 0, 0);
        let g = Rgb::new(0, 255, 0);
        assert_eq!(Profile::parse("red"), Some(Profile::Colors([r; ZONES])));
        assert_eq!(Profile::parse("red green red green"), Some(Profile::Colors([r, g, r, g])));
        assert_eq!(Profile::parse("off"), Some(Profile::Colors([Rgb::OFF; ZONES])));
        assert_eq!(Profile::parse("Rainbow"), Some(Profile::Effect("rainbow".into())));
        assert_eq!(Profile::parse("pulse"), Some(Profile::Effect("pulse".into())));
        assert_eq!(Profile::parse("red green"), None);
        assert_eq!(Profile::parse("nothing"), None);
        for v in ["red", "red green red green", "off", "rainbow"] {
            let p = Profile::parse(v).unwrap();
            assert_eq!(Profile::parse(&p.to_string()), Some(p), "round trip of {v}");
        }
        assert_eq!(Profile::Colors([r; ZONES]).to_string(), "ff0000");

        let mut c = Config::default();
        set(&mut c, "profile_427520", "tide").unwrap();
        set(&mut c, "profile_12", "red").unwrap();
        assert_eq!(c.profiles["427520"], Profile::Effect("tide".into()));
        assert_eq!(parse(&format(&c)), c);
        assert!(set(&mut c, "profile_factorio", "red").is_err());
        assert!(set(&mut c, "profile_", "red").is_err());
        assert!(set(&mut c, "profile_12", "red green").is_err());
        assert_eq!(c.profiles.len(), 2, "an error changes nothing");
        set(&mut c, "profile_12", "remove").unwrap();
        assert!(!c.profiles.contains_key("12"));
        assert!(entries(&c).contains(&("profile_427520".to_string(), "tide".to_string())));
        assert!(parse("profile_x = red\nprofile_5 = red green").profiles.is_empty(), "invalid lines are ignored when reading");
    }
}
