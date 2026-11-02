//! 4-band equalizer: whatever plays on the default output, one band per zone.

use halo_core::{Frame, Rgb, ZONES};
use rustfft::num_complex::Complex;
use rustfft::{Fft, FftPlanner};
use std::f32::consts::PI;
use std::sync::Arc;

pub const RATE: u32 = 16_000;
pub const FPS: u32 = 30;
const BANDS_HZ: [(f32, f32); ZONES] = [(40.0, 160.0), (160.0, 600.0), (600.0, 2500.0), (2500.0, 7500.0)];
/// Color of each band at full level: bass red, low-mids orange, mids green, highs sky blue.
const COLORS: [[f32; 3]; ZONES] = [[255.0, 0.0, 40.0], [255.0, 120.0, 0.0], [0.0, 255.0, 60.0], [0.0, 140.0, 255.0]];
/// Smoothing: rises fast, falls slowly.
const ATTACK: f32 = 0.6;
const DECAY: f32 = 0.15;
/// How fast the peak is forgotten (automatic gain).
const GAIN_DECAY: f32 = 0.995;
/// Below this the zone goes off (silence).
const FLOOR: f32 = 0.04;

pub struct Equalizer {
    fft: Arc<dyn Fft<f32>>,
    window: Vec<f32>,
    /// Bin range `[start, end)` of each band.
    bands: [(usize, usize); ZONES],
    level: [f32; ZONES],
    peak: [f32; ZONES],
    buf: Vec<Complex<f32>>,
}

impl Equalizer {
    pub fn new(block: usize) -> Self {
        let fft = FftPlanner::new().plan_fft_forward(block);
        // numpy.hanning
        let window = (0..block).map(|n| 0.5 - 0.5 * (2.0 * PI * n as f32 / (block - 1) as f32).cos()).collect();
        let bin_hz = RATE as f32 / block as f32;
        let bins = block / 2 + 1;
        let bands = BANDS_HZ.map(|(lo, hi)| {
            let start = (lo / bin_hz).ceil() as usize;
            let end = ((hi / bin_hz).ceil() as usize).min(bins);
            (start, end.max(start + 1))
        });
        Self { fft, window, bands, level: [0.0; ZONES], peak: [1e-3; ZONES], buf: vec![Complex::default(); block] }
    }

    pub fn block(&self) -> usize {
        self.window.len()
    }

    /// Mean energy of each band (unnormalized) from a block of samples in [-1, 1].
    pub fn energies(&mut self, samples: &[f32]) -> [f32; ZONES] {
        for ((b, s), w) in self.buf.iter_mut().zip(samples).zip(&self.window) {
            *b = Complex::new(s * w, 0.0);
        }
        self.fft.process(&mut self.buf);
        self.bands.map(|(start, end)| self.buf[start..end].iter().map(|c| c.norm()).sum::<f32>() / (end - start) as f32)
    }

    /// A block of samples in [-1, 1] → the frame to show.
    pub fn process(&mut self, samples: &[f32]) -> Frame {
        let energies = self.energies(samples);
        let mut frame = [Rgb::OFF; ZONES];
        for z in 0..ZONES {
            let energy = energies[z];
            // With a floor: in a long silence the peak would reach 0 and 0/0 would leave the level at NaN.
            self.peak[z] = energy.max(self.peak[z] * GAIN_DECAY).max(super::pulse::MIN_PEAK);
            let target = (energy / self.peak[z]).clamp(0.0, 1.0).powf(1.5);
            let coef = if target > self.level[z] { ATTACK } else { DECAY };
            self.level[z] += (target - self.level[z]) * coef;
            let shown = if self.level[z] < FLOOR { 0.0 } else { self.level[z] };
            let [r, g, b] = COLORS[z].map(|c| (c * shown) as u8);
            frame[z] = Rgb::new(r, g, b);
        }
        frame
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(hz: f32, n: usize) -> Vec<f32> {
        (0..n).map(|i| 0.5 * (2.0 * PI * hz * i as f32 / RATE as f32).sin()).collect()
    }

    #[test]
    fn silence_is_dark() {
        let mut eq = Equalizer::new((RATE / FPS) as usize);
        let block = vec![0.0; eq.block()];
        for _ in 0..10 {
            assert_eq!(eq.process(&block), [Rgb::OFF; ZONES]);
        }
    }

    #[test]
    fn long_silence_does_not_poison_the_level() {
        let mut eq = Equalizer::new((RATE / FPS) as usize);
        let quiet = vec![0.0; eq.block()];
        for _ in 0..FPS * 60 * 15 {
            eq.process(&quiet);
        }
        let tone = sine(80.0, eq.block());
        let frame = (0..5).map(|_| eq.process(&tone)).last().unwrap();
        assert!(frame[0].r > 200, "{:?}", frame[0]);
    }

    #[test]
    fn bass_lights_zone_one_red() {
        let mut eq = Equalizer::new((RATE / FPS) as usize);
        let tone = sine(80.0, eq.block());
        let mut frame = [Rgb::OFF; ZONES];
        for _ in 0..10 {
            frame = eq.process(&tone);
        }
        assert!(frame[0].r > 200, "bass: {:?}", frame[0]);
        // The other bands do not reach their own peak with a pure tone that far away.
        assert!(frame[3].b < frame[0].r);
    }
}
