//! `pulse` (Armoury Crate's "Music"): the 4 LEDs beat with the music. Always on in a color of the
//! wheel; the level lightens them toward white and every beat jumps the hue. It never dims: each
//! LED lights half a ring.
//!
//! It listens to the bass (kick and bass guitar). If [`BORING`] goes by without a bass beat, it
//! follows the whole spectrum (whichever band stands out at the moment), and the first bass beat
//! brings it back. Both detectors always run, so switching between them does not invent beats.

use super::anim::{hue, mix};
use super::equalizer::{Equalizer, FPS};
use halo_core::{Frame, Rgb, ZONES};

/// The closest it gets to white at full level.
const WHITE_MAX: f64 = 0.7;
const WHITE: Rgb = Rgb::new(255, 255, 255);
/// Level smoothing: rises fast, falls slowly.
const ATTACK: f32 = 0.7;
const DECAY: f32 = 0.2;
const GAIN_DECAY: f32 = 0.995;
/// Floor for the automatic-gain peak. Without it, in a long silence the peak reaches 0 and 0/0
/// leaves the level at NaN forever; and if it is too low, the window leakage between bands (a
/// 1 kHz tone leaves ~0.0007 in the bass) normalizes to 1 and looks like a kick. A bass at -40 dB
/// gives ~0.65.
pub const MIN_PEAK: f32 = 0.05;
/// Beat: the level jumps (at least `BEAT_RISE` over the previous frame) and exceeds this
/// multiple of its recent average. A sustained sound does not count.
const BEAT_RATIO: f32 = 1.5;
const BEAT_RISE: f32 = 0.25;
/// Recent average: exponential moving average of ~1 s.
const AVERAGE: f32 = 1.0 / FPS as f32;
/// Minimum between beats, in frames: 0.35 s → no more than 2.9 beats/s (WCAG asks for ≤ 3 flashes/s).
const BEAT_GAP: u32 = (0.35 * FPS as f64) as u32 + 1;
/// How much the hue jumps on each beat, in degrees.
const BEAT_HUE: f64 = 45.0;
/// Below this is silence: no level and no beats.
const FLOOR: f32 = 0.04;
/// Frames without a bass beat before switching to the whole spectrum: 5 s.
const BORING: u32 = 5 * FPS;

/// Automatic gain and beat detection for one signal.
#[derive(Default)]
struct Detector {
    peak: f32,
    previous: f32,
    average: f32,
}

impl Detector {
    /// The energy normalized against its own peak (0..1).
    fn normalize(peak: &mut f32, energy: f32) -> f32 {
        *peak = energy.max(*peak * GAIN_DECAY).max(MIN_PEAK);
        let t = (energy / *peak).clamp(0.0, 1.0);
        if t < FLOOR { 0.0 } else { t }
    }

    /// (level 0..1, did it jump?) from the already-normalized level.
    fn feed(&mut self, target: f32) -> (f32, bool) {
        let onset = target > FLOOR && target - self.previous >= BEAT_RISE && target > self.average * BEAT_RATIO;
        self.average += (target - self.average) * AVERAGE;
        self.previous = target;
        (target, onset)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Bass,
    Spectrum,
}

pub struct Pulse {
    eq: Equalizer,
    level: f32,
    bass: Detector,
    spectrum: Detector,
    /// Peak of each band for spectrum mode, each with its own gain.
    band_peaks: [f32; ZONES],
    since_beat: u32,
    since_bass: u32,
    hue: f64,
    /// Custom colors: every beat moves to the next one. Empty = the color wheel.
    colors: Vec<Rgb>,
    index: usize,
}

impl Pulse {
    pub fn new(eq: Equalizer, colors: Vec<Rgb>) -> Self {
        Self {
            eq,
            level: 0.0,
            bass: Detector::default(),
            spectrum: Detector::default(),
            band_peaks: [MIN_PEAK; ZONES],
            since_beat: BEAT_GAP,
            since_bass: 0,
            hue: 0.0,
            colors,
            index: 0,
        }
    }

    /// What it is listening to right now.
    pub fn source(&self) -> Source {
        if self.since_bass < BORING { Source::Bass } else { Source::Spectrum }
    }

    pub fn process(&mut self, samples: &[f32]) -> Frame {
        let energies = self.eq.energies(samples);
        let bass = Detector::normalize(&mut self.bass.peak, energies[0]);
        let mut spectrum: f32 = 0.0;
        for (peak, e) in self.band_peaks.iter_mut().zip(energies) {
            spectrum = spectrum.max(Detector::normalize(peak, e));
        }
        let (bass, bass_onset) = self.bass.feed(bass);
        let (spectrum, spectrum_onset) = self.spectrum.feed(spectrum);

        self.since_bass = if bass_onset { 0 } else { self.since_bass.saturating_add(1) };
        let (target, onset) = match self.source() {
            Source::Bass => (bass, bass_onset),
            Source::Spectrum => (spectrum, spectrum_onset),
        };

        let coef = if target > self.level { ATTACK } else { DECAY };
        self.level += (target - self.level) * coef;
        self.since_beat = self.since_beat.saturating_add(1);
        if onset && self.since_beat >= BEAT_GAP {
            self.hue = (self.hue + BEAT_HUE) % 360.0;
            self.index = (self.index + 1) % self.colors.len().max(1);
            self.since_beat = 0;
        }

        let level = self.level.powf(1.5) as f64;
        let color = self.colors.get(self.index).copied().unwrap_or_else(|| hue(self.hue));
        [mix(color, WHITE, WHITE_MAX * level); ZONES]
    }
}

#[cfg(test)]
mod tests {
    use super::super::equalizer::RATE;
    use super::*;
    use std::f32::consts::PI;

    fn pulse() -> Pulse {
        Pulse::new(Equalizer::new((RATE / FPS) as usize), vec![])
    }

    fn tone(hz: f32, a: f32) -> Vec<f32> {
        (0..(RATE / FPS) as usize).map(|i| a * (2.0 * PI * hz * i as f32 / RATE as f32).sin()).collect()
    }

    fn block(loud: bool) -> Vec<f32> {
        tone(80.0, if loud { 0.6 } else { 0.0 })
    }

    #[test]
    fn silence_keeps_the_color_on() {
        let mut p = pulse();
        let quiet = block(false);
        for _ in 0..20 {
            assert_eq!(p.process(&quiet), [Rgb::new(255, 0, 0); ZONES]);
        }
    }

    #[test]
    fn beats_brighten_and_shift_the_hue_but_not_too_often() {
        let mut p = pulse();
        let (loud, quiet) = (block(true), block(false));
        // One beat: lightens toward white and jumps the hue.
        let f = p.process(&loud);
        assert!(f[0].g > 0 && f[0].b > 0, "lightens: {:?}", f[0]);
        assert_eq!(p.hue, BEAT_HUE);
        // Sustained bass: after the first beat it is no longer a "beat".
        for _ in 0..60 {
            p.process(&loud);
        }
        assert_eq!(p.hue, BEAT_HUE);
        // Beats every 4 frames (7.5/s): only one per BEAT_GAP counts.
        let mut q = pulse();
        let mut beats = 0;
        for i in 0..FPS * 4 {
            let before = q.hue;
            q.process(if i % 4 == 0 { &loud } else { &quiet });
            beats += (q.hue != before) as u32;
        }
        assert!(beats <= 4 * 3, "{beats} beats in 4 s");
        assert!(beats >= 4, "{beats} beats in 4 s");
    }

    #[test]
    fn boring_bass_switches_to_the_spectrum_and_back() {
        let mut p = pulse();
        // A piano with no bass: 1 kHz notes every 0.5 s, no low end.
        let (note, quiet) = (tone(1000.0, 0.5), block(false));
        let mut beats = 0;
        for i in 0..FPS * 10 {
            let before = p.hue;
            p.process(if i % 15 == 0 { &note } else { &quiet });
            if i < BORING - 1 {
                assert_eq!(p.source(), Source::Bass, "frame {i}");
                assert_eq!(p.hue, before, "on bass, the piano does not move the hue");
            } else if i >= BORING {
                assert_eq!(p.source(), Source::Spectrum);
                beats += (p.hue != before) as u32;
            }
        }
        assert!(beats >= 8, "{beats} beats in 5 s of piano");
        // The kick comes in: that same frame it goes back to the bass.
        p.process(&block(true));
        assert_eq!(p.source(), Source::Bass);
    }

    #[test]
    fn long_silence_does_not_poison_the_level() {
        let mut p = pulse();
        let quiet = block(false);
        for _ in 0..FPS * 60 * 15 {
            p.process(&quiet);
        }
        let f = p.process(&block(true));
        assert!(f[0].g > 0, "still reacts after 15 min of silence: {:?}", f[0]);
    }
}
