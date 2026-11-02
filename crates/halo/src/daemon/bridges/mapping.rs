//! How what games send (meant for keyboards and other peripherals) is reduced to the 4 zones.
//!
//! Zones follow the position of the Ally's lamps (`docs/lamparray.md`):
//! 0 left bottom, 1 left top, 2 right top, 3 right bottom.

use halo_core::{Frame, Rgb};

/// Average of the non-black colors (games usually light a few keys over black).
pub fn average_lit(colors: impl IntoIterator<Item = Rgb>) -> Rgb {
    let (mut r, mut g, mut b, mut n) = (0u32, 0u32, 0u32, 0u32);
    for c in colors.into_iter().filter(|c| *c != Rgb::OFF) {
        r += c.r as u32;
        g += c.g as u32;
        b += c.b as u32;
        n += 1;
    }
    match (r.checked_div(n), g.checked_div(n), b.checked_div(n)) {
        (Some(r), Some(g), Some(b)) => Rgb::new(r as u8, g as u8, b as u8),
        _ => Rgb::OFF,
    }
}

/// A grid (row by row, from the top left) to 4 zones: each quadrant goes to its zone.
pub fn grid(rows: usize, cols: usize, cells: &[Rgb]) -> Frame {
    let region = |right: bool, bottom: bool| {
        average_lit(
            (0..rows)
                .flat_map(|r| (0..cols).map(move |c| (r, c)))
                .filter(|&(r, c)| (c >= cols / 2) == right && (r >= rows / 2) == bottom)
                .map(|(r, c)| cells[r * cols + c]),
        )
    };
    [region(false, true), region(false, false), region(true, false), region(true, true)]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn average_ignores_black() {
        let r = Rgb::new(200, 0, 0);
        assert_eq!(average_lit([Rgb::OFF, r, Rgb::OFF]), r);
        assert_eq!(average_lit([Rgb::OFF]), Rgb::OFF);
        assert_eq!(average_lit([Rgb::new(100, 0, 0), Rgb::new(0, 0, 200)]), Rgb::new(50, 0, 100));
    }

    #[test]
    fn quadrants() {
        let (r, g, b, y) = (Rgb::new(255, 0, 0), Rgb::new(0, 255, 0), Rgb::new(0, 0, 255), Rgb::new(255, 255, 0));
        // 2x2: top left r, top right g, bottom left b, bottom right y
        assert_eq!(grid(2, 2, &[r, g, b, y]), [b, r, g, y]);
    }
}
