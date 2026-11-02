//! Armoury Crate-style light effects as pure functions of time: `render` says which frame is due
//! at second `t` and `next` when it changes. The loop (in `mod.rs`) only wakes up for that:
//! ~25 fps for the smooth ones and only on changes for the strobe.
//!
//! Colors come from the base color: breathing and strobe use each zone's, and the comet the first
//! zone's. Cycle and rainbow walk the full color wheel.

use halo_core::backend::{HwMode, HwProgram, HwSpeed, HwStep};
use halo_core::{Frame, Rgb, ZONES};

/// The configurable part of an effect (`halo effect colors|direction`), plus the place for `sun`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Options {
    /// Custom colors; empty = the defaults. What each one means depends on the effect.
    pub colors: Vec<Rgb>,
    /// Run backwards: rainbow, cycle, comet and tide.
    pub reverse: bool,
    /// (latitude, longitude), only for `sun`.
    pub place: (f64, f64),
}
use std::f64::consts::TAU;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Breathing,
    Strobe,
    Cycle,
    Rainbow,
    Comet,
    Tide,
    Starry,
    /// A single star at a time, with a stronger flash.
    Star,
    /// The time (24 h); `clock.rs` paints it with the local time, not with `t`.
    Clock,
    /// The time in 12 h.
    Clock12,
    /// The color follows the sun's elevation (`sun.rs`).
    Sun,
    /// Temperature and CPU usage (`smart.rs`). Keeps state between reads, so it does not go through
    /// [`render`]: its own loop paints it.
    Sensors,
    /// The battery charge (`battery.rs`).
    Battery,
}

pub const KINDS: &[(&str, Kind)] = &[
    ("breathing", Kind::Breathing),
    ("strobe", Kind::Strobe),
    ("cycle", Kind::Cycle),
    ("rainbow", Kind::Rainbow),
    ("comet", Kind::Comet),
    ("tide", Kind::Tide),
    ("starry", Kind::Starry),
    ("star", Kind::Star),
    ("clock", Kind::Clock),
    ("clock12", Kind::Clock12),
    ("sun", Kind::Sun),
    ("sensors", Kind::Sensors),
    ("battery", Kind::Battery),
];

impl Kind {
    pub fn parse(name: &str) -> Option<Self> {
        let plain = name.to_lowercase();
        KINDS.iter().find(|(n, _)| *n == plain).map(|&(_, k)| k)
    }

    pub fn name(self) -> &'static str {
        KINDS.iter().find(|&&(_, k)| k == self).map(|&(n, _)| n).expect("every Kind is listed in KINDS")
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Speed {
    Slow,
    #[default]
    Medium,
    Fast,
}

impl Speed {
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "slow" => Some(Self::Slow),
            "medium" => Some(Self::Medium),
            "fast" => Some(Self::Fast),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Slow => "slow",
            Self::Medium => "medium",
            Self::Fast => "fast",
        }
    }
}

const FPS: f64 = 25.0;

/// Seconds one cycle of the effect lasts.
fn period(kind: Kind, speed: Speed) -> f64 {
    let [slow, medium, fast] = match kind {
        Kind::Breathing => [5.0, 3.5, 2.0],
        // Never more than 3 flashes per second (WCAG 2.3.1, photosensitive epilepsy): fast = 2.5.
        Kind::Strobe => [1.0, 0.5, 0.4],
        Kind::Cycle => [12.0, 8.0, 4.0],
        Kind::Rainbow => [8.0, 5.0, 3.0],
        Kind::Comet => [2.5, 1.6, 1.0],
        Kind::Tide => [7.0, 4.5, 2.5],
        // How long each star lasts.
        Kind::Starry => [2.2, 1.6, 1.1],
        // How often the star appears (≤ 1.1 flashes/s, well below the WCAG limit).
        Kind::Star => [2.0, 1.4, 0.9],
        Kind::Clock | Kind::Clock12 | Kind::Sun | Kind::Sensors | Kind::Battery => [60.0; 3],
    };
    match speed {
        Speed::Slow => slow,
        Speed::Medium => medium,
        Speed::Fast => fast,
    }
}

// Each LED lights half a ring: lowering its brightness does not look "dim", it looks as if that
// half went off. That is why tide and stars shift the hue and never drop the brightness much.

/// Tide: the wave goes from deep blue to aquamarine, with brightness between 75 % and 100 %.
const TIDE_DEEP: Rgb = Rgb::new(0, 50, 255);
const TIDE_CREST: Rgb = Rgb::new(0, 255, 190);
const TIDE_MIN: f64 = 0.75;

/// Stars: a lit night-blue background; the stars go to one of these colors and back.
const STARRY_SKY: Rgb = Rgb::new(20, 30, 200);
const STARS: [Rgb; 3] = [Rgb::new(255, 255, 255), Rgb::new(170, 210, 255), Rgb::new(255, 225, 150)];

/// Probability that a star is born in each zone per interval (slow / medium / fast).
fn star_chance(speed: Speed) -> f64 {
    match speed {
        Speed::Slow => 0.25,
        Speed::Medium => 0.4,
        Speed::Fast => 0.6,
    }
}

/// splitmix64: reproducible randomness from the interval and the zone.
fn hash(a: u64, b: u64) -> u64 {
    let mut z = a.wrapping_mul(0x9e37_79b9_7f4a_7c15) ^ b.wrapping_add(0xd1b5_4a32_d192_ed03);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

fn unit(h: u64) -> f64 {
    (h >> 11) as f64 / (1u64 << 53) as f64
}

pub fn mix(a: Rgb, b: Rgb, f: f64) -> Rgb {
    let f = f.clamp(0.0, 1.0);
    let m = |p: u8, q: u8| (p as f64 + (q as f64 - p as f64) * f).round() as u8;
    Rgb::new(m(a.r, b.r), m(a.g, b.g), m(a.b, b.b))
}

/// The brightest star in `zone` at second `t`: (intensity 0..1, color from `stars`). Each interval
/// of `life` seconds may give birth to one at a random moment within it, and it lasts `life`, so at
/// a given instant both the current interval's and the previous one's may count.
/// Star: rises during the first `STAR_ATTACK` of the interval and fades over what follows until
/// `STAR_END`; the rest is just sky. The other LED of the same ring lightens by `STAR_HALO`.
const STAR_ATTACK: f64 = 0.12;
const STAR_END: f64 = 0.75;
const STAR_HALO: f64 = 0.3;

/// (zone, intensity 0..1) of the star of the interval `t` falls in. It never repeats the previous
/// interval's zone: zone = (interval + r) mod 4 with r random in {0, 2}, so two in a row differ by
/// 1 ± 2, which is never 0 mod 4. Stateless, like the rest.
fn lone_star(t: f64, every: f64) -> (usize, f64) {
    let slot = (t / every).floor() as u64;
    let zone = ((slot + 2 * (hash(slot, 7) & 1)) % ZONES as u64) as usize;
    let u = (t / every).fract();
    let level = if u < STAR_ATTACK {
        u / STAR_ATTACK
    } else if u < STAR_END {
        (1.0 - (u - STAR_ATTACK) / (STAR_END - STAR_ATTACK)).powi(2)
    } else {
        0.0
    };
    (zone, level)
}

fn star(zone: usize, t: f64, life: f64, chance: f64, stars: &[Rgb]) -> (f64, Rgb) {
    let slot = (t / life).floor() as i64;
    let mut best = (0.0, stars[0]);
    for k in [slot - 1, slot] {
        let h = hash(k as u64, zone as u64);
        if unit(h) >= chance {
            continue;
        }
        let born = (k as f64 + unit(hash(h, 1))) * life;
        let u = (t - born) / life;
        if (0.0..1.0).contains(&u) {
            let level = (std::f64::consts::PI * u).sin().powi(2);
            if level > best.0 {
                best = (level, stars[(hash(h, 2) % stars.len() as u64) as usize]);
            }
        }
    }
    best
}

/// Order in which the comet walks the zones. On the left ring zone B goes before A so both rings
/// spin the same way (verified on hardware).
const COMET_ORDER: [usize; ZONES] = [1, 0, 2, 3];
/// Tail length, in zones.
const COMET_TAIL: f64 = 2.0;

fn scale(c: Rgb, level: f64) -> Rgb {
    let f = |v: u8| (v as f64 * level.clamp(0.0, 1.0)).round() as u8;
    Rgb::new(f(c.r), f(c.g), f(c.b))
}

/// Hue in degrees → color with full saturation and value.
pub fn hue(h: f64) -> Rgb {
    let h = h.rem_euclid(360.0) / 60.0;
    let x = 1.0 - (h % 2.0 - 1.0).abs();
    let (r, g, b) = match h as u8 {
        0 => (1.0, x, 0.0),
        1 => (x, 1.0, 0.0),
        2 => (0.0, 1.0, x),
        3 => (0.0, x, 1.0),
        4 => (x, 0.0, 1.0),
        _ => (1.0, 0.0, x),
    };
    let f = |v: f64| (v * 255.0).round() as u8;
    Rgb::new(f(r), f(g), f(b))
}

/// The program for the MCU to animate this effect on its own, if it can: breathing (base color per
/// zone, or 1-2 custom colors), cycle and rainbow without a palette. Everything else is software.
pub fn hardware(kind: Kind, speed: Speed, base: &Frame, o: &Options) -> Option<HwProgram> {
    let speed = match speed {
        Speed::Slow => HwSpeed::Slow,
        Speed::Medium => HwSpeed::Medium,
        Speed::Fast => HwSpeed::Fast,
    };
    let step = |zone: u8, mode: HwMode, color: Rgb, color2: Rgb| HwStep { zone, mode, color, color2, speed, reverse: o.reverse };
    let steps = match (kind, o.colors.as_slice()) {
        // MCU zones: 1-4 in the same order as ours (0-3).
        (Kind::Breathing, []) => (0..ZONES).map(|z| step(z as u8 + 1, HwMode::Breathe, base[z], Rgb::OFF)).collect(),
        (Kind::Breathing, [c]) => vec![step(0, HwMode::Breathe, *c, Rgb::OFF)],
        (Kind::Breathing, [a, b]) => vec![step(0, HwMode::Breathe, *a, *b)],
        (Kind::Cycle, [] | [_]) => vec![step(0, HwMode::Cycle, Rgb::OFF, Rgb::OFF)],
        (Kind::Rainbow, [] | [_]) => vec![step(0, HwMode::Wave, Rgb::OFF, Rgb::OFF)],
        _ => return None,
    };
    Some(HwProgram { steps })
}

/// A point of a closed palette: `x` in turns (0..1 walks all the colors and returns to the first).
fn palette_at(colors: &[Rgb], x: f64) -> Rgb {
    let n = colors.len();
    let pos = x.rem_euclid(1.0) * n as f64;
    let i = (pos as usize).min(n - 1);
    mix(colors[i], colors[(i + 1) % n], pos - i as f64)
}

pub fn render(kind: Kind, speed: Speed, base: &Frame, t: f64, o: &Options) -> Frame {
    let p = period(kind, speed);
    let phase = (t / p).fract();
    // Reversed: time runs backwards.
    let phase = if o.reverse { 1.0 - phase } else { phase };
    let colors = o.colors.as_slice();
    let place = o.place;
    match kind {
        Kind::Breathing => {
            // Squared raised cosine: the eye notices changes more in the dark.
            let level = (1.0 - (TAU * phase).cos()) / 2.0;
            match colors {
                // With two colors it breathes between them and never goes off.
                [a, b, ..] => [mix(*a, *b, level * level); ZONES],
                [c] => [scale(*c, level * level); ZONES],
                [] => base.map(|c| scale(c, level * level)),
            }
        }
        Kind::Strobe => {
            if phase < 0.5 {
                colors.first().map_or(*base, |c| [*c; ZONES])
            } else {
                [Rgb::OFF; ZONES]
            }
        }
        Kind::Cycle => match colors {
            [] | [_] => [hue(360.0 * phase); ZONES],
            _ => [palette_at(colors, phase); ZONES],
        },
        Kind::Rainbow => std::array::from_fn(|i| {
            let x = phase + i as f64 / ZONES as f64;
            match colors {
                [] | [_] => hue(360.0 * x),
                _ => palette_at(colors, x),
            }
        }),
        Kind::Clock | Kind::Clock12 => {
            let now = super::clock::now();
            let hour = if kind == Kind::Clock12 { super::clock::twelve(now.hour) } else { now.hour };
            let level = super::clock::breath(now.second, now.frac);
            super::clock::frame(hour, now.minute).map(|c| scale(c, level))
        }
        Kind::Sun => {
            let (elev, min, max) = super::sun::elevation(super::sun::unix_now(), place.0, place.1);
            [super::sun::color(super::sun::position(elev, min, max)); ZONES]
        }
        // `run_smart` paints it; here only the temperature, without the usage.
        Kind::Sensors => super::smart::frame(super::smart::find_sensor().as_deref().and_then(super::smart::read_temp), None),
        Kind::Battery => super::battery::frame(super::battery::read(), t, COMET_ORDER),
        Kind::Tide => {
            let deep = colors.first().copied().unwrap_or(TIDE_DEEP);
            let crest = colors.get(1).copied().unwrap_or(TIDE_CREST);
            let mut out = [Rgb::OFF; ZONES];
            for (pos, &zone) in COMET_ORDER.iter().enumerate() {
                let w = (1.0 + (TAU * (phase - pos as f64 / ZONES as f64)).sin()) / 2.0;
                out[zone] = scale(mix(deep, crest, w), TIDE_MIN + (1.0 - TIDE_MIN) * w);
            }
            out
        }
        Kind::Starry => {
            // The first is the sky and the rest are the stars.
            let sky = colors.first().copied().unwrap_or(STARRY_SKY);
            let stars = if colors.len() > 1 { &colors[1..] } else { &STARS[..] };
            std::array::from_fn(|zone| {
                let (level, color) = star(zone, t, p, star_chance(speed), stars);
                mix(sky, color, level)
            })
        }
        Kind::Star => {
            let sky = colors.first().copied().unwrap_or(STARRY_SKY);
            let star = colors.get(1).copied().unwrap_or(STARS[0]);
            let (zone, level) = lone_star(t, p);
            let mut out = [sky; ZONES];
            out[zone] = mix(sky, star, level);
            // The partner on the same ring: 0↔1 (left) and 2↔3 (right).
            out[zone ^ 1] = mix(sky, star, level * STAR_HALO);
            out
        }
        Kind::Comet => {
            let head_color = colors.first().copied().unwrap_or(base[0]);
            let head = phase * ZONES as f64;
            let mut out = [Rgb::OFF; ZONES];
            for (pos, &zone) in COMET_ORDER.iter().enumerate() {
                // How far behind the head this zone is (0 = the head).
                let behind = (head - pos as f64).rem_euclid(ZONES as f64);
                let level = (1.0 - behind / COMET_TAIL).max(0.0);
                out[zone] = scale(head_color, level * level);
            }
            out
        }
    }
}

/// When to paint again, in seconds since the start.
pub fn next(kind: Kind, speed: Speed, t: f64) -> f64 {
    match kind {
        Kind::Strobe => {
            let half = period(kind, speed) / 2.0;
            ((t / half).floor() + 1.0) * half
        }
        Kind::Sun => t + super::sun::EVERY,
        Kind::Sensors => t + super::smart::EVERY,
        // While charging, at the next flash step; otherwise, at the next read.
        Kind::Battery => match super::battery::read() {
            Some(s) if s.charging => ((t / super::battery::STEP).floor() + 1.0) * super::battery::STEP,
            _ => t + super::battery::EVERY,
        },
        _ => ((t * FPS).floor() + 1.0) / FPS,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const R: Rgb = Rgb::new(255, 0, 0);
    const B: Rgb = Rgb::new(0, 0, 255);

    #[test]
    fn names() {
        assert_eq!(Kind::parse("Rainbow"), Some(Kind::Rainbow));
        assert_eq!(Kind::parse("STROBE"), Some(Kind::Strobe));
        assert_eq!(Kind::parse("equalizer"), None);
        for &(n, k) in KINDS {
            assert_eq!(k.name(), n);
        }
        assert_eq!(Speed::parse("Fast"), Some(Speed::Fast));
        assert_eq!(Speed::parse("x"), None);
    }

    #[test]
    fn hues() {
        assert_eq!(hue(0.0), R);
        assert_eq!(hue(120.0), Rgb::new(0, 255, 0));
        assert_eq!(hue(240.0), B);
        assert_eq!(hue(360.0 + 60.0), Rgb::new(255, 255, 0));
        assert_eq!(hue(-120.0), B);
    }

    #[test]
    fn breathing_goes_dark_and_back() {
        let base = [R, B, R, B];
        let p = period(Kind::Breathing, Speed::Medium);
        assert_eq!(render(Kind::Breathing, Speed::Medium, &base, 0.0, &Options::default()), [Rgb::OFF; ZONES]);
        assert_eq!(render(Kind::Breathing, Speed::Medium, &base, p / 2.0, &Options::default()), base);
        let quarter = render(Kind::Breathing, Speed::Medium, &base, p / 4.0, &Options::default());
        assert_eq!(quarter[0], Rgb::new(64, 0, 0)); // (1/2)² of 255
    }

    #[test]
    fn strobe_only_wakes_on_changes() {
        let base = [R; ZONES];
        assert_eq!(render(Kind::Strobe, Speed::Medium, &base, 0.1, &Options::default()), base);
        assert_eq!(render(Kind::Strobe, Speed::Medium, &base, 0.3, &Options::default()), [Rgb::OFF; ZONES]);
        assert_eq!(next(Kind::Strobe, Speed::Medium, 0.1), 0.25);
        assert_eq!(next(Kind::Strobe, Speed::Medium, 0.25), 0.5);
        assert!((next(Kind::Rainbow, Speed::Medium, 0.0) - 0.04).abs() < 1e-9);
    }

    #[test]
    fn rainbow_spreads_the_wheel() {
        let f = render(Kind::Rainbow, Speed::Medium, &[R; ZONES], 0.0, &Options::default());
        assert_eq!(f, [hue(0.0), hue(90.0), hue(180.0), hue(270.0)]);
        let c = render(Kind::Cycle, Speed::Medium, &[R; ZONES], 0.0, &Options::default());
        assert_eq!(c, [R; ZONES]);
    }

    #[test]
    fn tide_moves_the_hue_not_the_light() {
        let p = period(Kind::Tide, Speed::Medium);
        for i in 0..40 {
            let f = render(Kind::Tide, Speed::Medium, &[R; ZONES], p * i as f64 / 40.0, &Options::default());
            for c in f {
                // Never off: the strongest channel always stays above 75 %.
                assert!(c.r.max(c.g).max(c.b) >= 190, "{c:?}");
            }
        }
        // The zones are out of phase: not all have the same color at once.
        let f = render(Kind::Tide, Speed::Medium, &[R; ZONES], 0.0, &Options::default());
        assert_ne!(f[0], f[2]);
    }

    #[test]
    fn stars_twinkle_over_a_lit_sky() {
        let (p, chance) = (period(Kind::Starry, Speed::Medium), star_chance(Speed::Medium));
        let mut lit = 0;
        for i in 0..2000 {
            let t = i as f64 * 0.05;
            let f = render(Kind::Starry, Speed::Medium, &[R; ZONES], t, &Options::default());
            for (zone, c) in f.into_iter().enumerate() {
                assert!(c.b >= 150, "the sky never goes off: {c:?}");
                if star(zone, t, p, chance, &STARS).0 > 0.5 {
                    lit += 1;
                }
            }
        }
        // In 100 s and 4 zones there must be stars, but not all the time.
        assert!(lit > 200 && lit < 6000, "{lit}");
        // Reproducible: the same instant gives the same result.
        assert_eq!(star(2, 12.34, p, chance, &STARS), star(2, 12.34, p, chance, &STARS));
    }

    #[test]
    fn one_star_at_a_time_and_it_jumps() {
        let p = period(Kind::Star, Speed::Medium);
        let mut last = None;
        for slot in 0..200u32 {
            let t = slot as f64 * p;
            let peak = render(Kind::Star, Speed::Medium, &[R; ZONES], t + STAR_ATTACK * p, &Options::default());
            let (zone, level) = lone_star(t + STAR_ATTACK * p, p);
            assert!(level > 0.99);
            assert_eq!(peak[zone], STARS[0], "peak in white");
            assert!(peak[zone ^ 1].r > STARRY_SKY.r && peak[zone ^ 1] != STARS[0], "halo on the other LED of the ring");
            assert_eq!(peak.iter().filter(|&&c| c == STARRY_SKY).count(), 2, "the other ring, sky only");
            assert_ne!(Some(zone), last, "never repeats a zone in a row");
            last = Some(zone);
            // At the end of the interval, sky only.
            assert_eq!(render(Kind::Star, Speed::Medium, &[R; ZONES], t + 0.9 * p, &Options::default()), [STARRY_SKY; ZONES]);
        }
    }

    #[test]
    fn hardware_program() {
        let base = [R, Rgb::new(0, 255, 0), Rgb::new(0, 0, 255), Rgb::new(255, 255, 0)];
        let p = hardware(Kind::Breathing, Speed::Slow, &base, &Options::default()).unwrap();
        assert_eq!(p.steps.len(), 4);
        assert_eq!((p.steps[2].zone, p.steps[2].color, p.steps[2].speed), (3, base[2], HwSpeed::Slow));
        let two = Options { colors: vec![R, Rgb::new(0, 0, 255)], ..Default::default() };
        let p = hardware(Kind::Breathing, Speed::Fast, &base, &two).unwrap();
        assert_eq!(p.steps, vec![HwStep { zone: 0, mode: HwMode::Breathe, color: R, color2: Rgb::new(0, 0, 255), speed: HwSpeed::Fast, reverse: false }]);
        let back = Options { reverse: true, ..Default::default() };
        assert_eq!(hardware(Kind::Rainbow, Speed::Medium, &base, &back).unwrap().steps[0].mode, HwMode::Wave);
        assert!(hardware(Kind::Rainbow, Speed::Medium, &base, &back).unwrap().steps[0].reverse);
        // What the MCU cannot do, in software.
        assert!(hardware(Kind::Rainbow, Speed::Medium, &base, &two).is_none());
        assert!(hardware(Kind::Comet, Speed::Medium, &base, &Options::default()).is_none());
        assert!(hardware(Kind::Breathing, Speed::Medium, &base, &Options { colors: vec![R, R, R], ..Default::default() }).is_none());
    }

    #[test]
    fn own_colors_and_reverse() {
        let (g, b) = (Rgb::new(0, 255, 0), Rgb::new(0, 0, 255));
        let two = Options { colors: vec![g, b], ..Default::default() };
        let p = period(Kind::Breathing, Speed::Medium);
        // With two colors it breathes between them: never off.
        assert_eq!(render(Kind::Breathing, Speed::Medium, &[R; ZONES], 0.0, &two), [g; ZONES]);
        assert_eq!(render(Kind::Breathing, Speed::Medium, &[R; ZONES], p / 2.0, &two), [b; ZONES]);
        // Closed palette: half a turn through [green, blue] is the blend, and a full turn returns.
        assert_eq!(palette_at(&[g, b], 0.25), mix(g, b, 0.5));
        assert_eq!(palette_at(&[g, b], 1.0), g);
        let f = render(Kind::Cycle, Speed::Medium, &[R; ZONES], 0.0, &two);
        assert_eq!(f, [g; ZONES]);
        // The comet with a custom color ignores the base color.
        let one = Options { colors: vec![g], ..Default::default() };
        let f = render(Kind::Comet, Speed::Medium, &[R; ZONES], 0.0, &one);
        assert_eq!(f[COMET_ORDER[0]], g);
        // Reversed, at t = p/4 the head is at the last zone of the order, not the second.
        let back = Options { reverse: true, ..Default::default() };
        let p = period(Kind::Comet, Speed::Medium);
        let f = render(Kind::Comet, Speed::Medium, &[R; ZONES], p / 4.0, &back);
        assert_eq!(f[COMET_ORDER[3]], R);
        assert_eq!(f[COMET_ORDER[1]], Rgb::OFF);
        // Stars: custom sky.
        let sky = Options { colors: vec![Rgb::new(10, 0, 30), R], ..Default::default() };
        let f = render(Kind::Star, Speed::Medium, &[R; ZONES], 0.9 * period(Kind::Star, Speed::Medium), &sky);
        assert_eq!(f, [Rgb::new(10, 0, 30); ZONES]);
    }

    #[test]
    fn comet_head_and_tail() {
        let base = [B, R, R, R];
        let p = period(Kind::Comet, Speed::Medium);
        // Head at the second zone of the walk (left A): left B is one behind (level ½, squared ¼)
        // and the rest off.
        let f = render(Kind::Comet, Speed::Medium, &base, p / 4.0, &Options::default());
        assert_eq!(f[0], B);
        assert_eq!(f[1], Rgb::new(0, 0, 64));
        assert_eq!(f[2], Rgb::OFF);
        assert_eq!(f[3], Rgb::OFF);
    }
}
