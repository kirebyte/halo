use std::fmt;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

/// Color names the CLI accepts.
pub const NAMED: &[(&str, Rgb)] = &[
    ("red", Rgb::new(0xff, 0x00, 0x00)),
    ("orange", Rgb::new(0xff, 0x60, 0x00)),
    ("yellow", Rgb::new(0xff, 0xc0, 0x00)),
    ("green", Rgb::new(0x00, 0xff, 0x00)),
    ("cyan", Rgb::new(0x00, 0xff, 0xff)),
    ("blue", Rgb::new(0x00, 0x00, 0xff)),
    ("purple", Rgb::new(0x80, 0x00, 0xff)),
    ("magenta", Rgb::new(0xff, 0x00, 0xff)),
    ("pink", Rgb::new(0xff, 0x40, 0x80)),
    ("white", Rgb::new(0xff, 0xff, 0xff)),
    ("off", Rgb::new(0x00, 0x00, 0x00)),
];

impl Rgb {
    pub const OFF: Rgb = Rgb::new(0, 0, 0);

    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    /// A name from [`NAMED`] or hex `RRGGBB` / `#RRGGBB`.
    pub fn parse(s: &str) -> Option<Self> {
        let lower = s.to_lowercase();
        if let Some((_, c)) = NAMED.iter().find(|(n, _)| *n == lower) {
            return Some(*c);
        }
        let hex = lower.strip_prefix('#').unwrap_or(&lower);
        if hex.len() != 6 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        let v = u32::from_str_radix(hex, 16).ok()?;
        Some(Self::new((v >> 16) as u8, (v >> 8) as u8, v as u8))
    }
}

impl fmt::Display for Rgb {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{:02x}{:02x}{:02x}", self.r, self.g, self.b)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_names_and_hex() {
        assert_eq!(Rgb::parse("Red"), Some(Rgb::new(255, 0, 0)));
        assert_eq!(Rgb::parse("#3a0057"), Some(Rgb::new(0x3a, 0, 0x57)));
        assert_eq!(Rgb::parse("FF8800"), Some(Rgb::new(0xff, 0x88, 0)));
        assert_eq!(Rgb::parse("fff"), None);
        assert_eq!(Rgb::parse("zzzzzz"), None);
        assert_eq!(Rgb::parse("+12345"), None);
    }

    #[test]
    fn display_roundtrip() {
        let c = Rgb::new(0x3a, 0x00, 0x57);
        assert_eq!(Rgb::parse(&c.to_string()), Some(c));
    }
}
