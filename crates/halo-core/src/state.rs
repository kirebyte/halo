//! The last frame and brightness applied, restored on startup and on wake-up.
//!
//! Text format: `RRGGBB RRGGBB RRGGBB RRGGBB [brightness]`, with the hardware brightness (0-3)
//! optional.

use crate::{Frame, Rgb, ZONES};
use std::path::PathBuf;
use std::{env, fs, io};

pub const DEFAULT_BRIGHTNESS: u8 = 3;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct State {
    pub frame: Frame,
    pub brightness: u8,
}

pub fn path() -> PathBuf {
    let base = env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(".local/state"));
    base.join("halo/state")
}

fn home() -> PathBuf {
    env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| "/".into())
}

pub fn parse(text: &str) -> Option<State> {
    let words: Vec<&str> = text.split_whitespace().collect();
    if words.len() != ZONES && words.len() != ZONES + 1 {
        return None;
    }
    let mut frame = [Rgb::OFF; ZONES];
    for (z, w) in frame.iter_mut().zip(&words) {
        *z = Rgb::parse(w)?;
    }
    let brightness = match words.get(ZONES) {
        Some(b) => b.parse().ok().filter(|b| *b <= 3)?,
        None => DEFAULT_BRIGHTNESS,
    };
    Some(State { frame, brightness })
}

pub fn format(s: &State) -> String {
    let zones: Vec<String> = s.frame.iter().map(Rgb::to_string).collect();
    format!("{} {}\n", zones.join(" "), s.brightness)
}

pub fn load() -> Option<State> {
    fs::read_to_string(path()).ok().and_then(|t| parse(&t))
}

pub fn save(s: &State) -> io::Result<()> {
    let p = path();
    fs::create_dir_all(p.parent().expect("path has a parent"))?;
    fs::write(p, format(s))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let short = parse("5500ff 5500ff 5500ff 5500ff\n").unwrap();
        assert_eq!(short.brightness, DEFAULT_BRIGHTNESS);
        let s = State { frame: [Rgb::new(1, 2, 3); ZONES], brightness: 1 };
        assert_eq!(parse(&format(&s)), Some(s));
        assert_eq!(parse("ff0000 ff0000"), None);
        assert_eq!(parse("ff0000 ff0000 ff0000 ff0000 9"), None);
    }
}
