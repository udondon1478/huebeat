//! Genre estimation from tempo + spectral features.
//! v1 is a rule-based scorer with temporal smoothing and hysteresis;
//! an ONNX model can later implement the same `GenreClassifier` trait.

use std::collections::HashMap;

use core_types::Genre;

/// Aggregated features over the recent window (5-10 s), produced by the
/// engine from `AnalysisFrame`s and `BandBeatEvent`s.
#[derive(Debug, Clone, Default)]
pub struct Features {
    pub bpm: f32,
    /// 0..1 confidence of the tempo estimate.
    pub bpm_confidence: f32,
    /// Average per-band energy 0..1 (low, low-mid, high-mid, high).
    pub band_energy: [f32; 4],
    /// Low-band beats per second over the window (kick density).
    pub kick_density: f32,
    /// High-band beats per second (hat density).
    pub hat_density: f32,
    /// Average spectral centroid in Hz.
    pub centroid: f32,
    /// Average loudness envelope 0..1.
    pub intensity: f32,
}

pub trait GenreClassifier: Send {
    /// Called roughly once per second with fresh aggregate features.
    /// Returns a genre only when the classification *changes*.
    fn update(&mut self, features: &Features) -> Option<Genre>;
    fn current(&self) -> Genre;
}

/// Triangular membership: 1.0 inside [lo_full, hi_full], falling to 0 at
/// lo_zero / hi_zero.
fn trapezoid(x: f32, lo_zero: f32, lo_full: f32, hi_full: f32, hi_zero: f32) -> f32 {
    if x <= lo_zero || x >= hi_zero {
        0.0
    } else if x < lo_full {
        (x - lo_zero) / (lo_full - lo_zero)
    } else if x <= hi_full {
        1.0
    } else {
        (hi_zero - x) / (hi_zero - hi_full)
    }
}

pub struct RuleBasedClassifier {
    scores: HashMap<Genre, f32>,
    current: Genre,
    /// Consecutive updates the challenger has led by the margin.
    challenger: Option<(Genre, u32)>,
    /// Updates the challenger must lead before we switch (~seconds).
    switch_after: u32,
}

impl Default for RuleBasedClassifier {
    fn default() -> Self {
        Self {
            scores: HashMap::new(),
            current: Genre::Unknown,
            challenger: None,
            switch_after: 6,
        }
    }
}

impl RuleBasedClassifier {
    fn instant_scores(f: &Features) -> Vec<(Genre, f32)> {
        let bpm = f.bpm;
        let [low, low_mid, high_mid, high] = f.band_energy;
        let bright = f.centroid;
        let mut out = Vec::new();

        if f.intensity < 0.08 || f.bpm_confidence < 0.05 {
            out.push((Genre::Ambient, 0.6));
            return out;
        }

        // Each score: tempo membership x supporting spectral evidence.
        out.push((
            Genre::DeepHouse,
            trapezoid(bpm, 112.0, 118.0, 125.0, 128.0)
                * (0.5 + 0.5 * low)
                * trapezoid(bright, 0.0, 0.0, 1800.0, 3200.0),
        ));
        out.push((
            Genre::House,
            trapezoid(bpm, 118.0, 122.0, 130.0, 134.0) * (0.4 + 0.6 * high_mid),
        ));
        out.push((
            Genre::Techno,
            trapezoid(bpm, 124.0, 128.0, 140.0, 146.0)
                * (0.4 + 0.6 * low)
                * (0.5 + 0.5 * trapezoid(f.kick_density, 1.2, 1.8, 3.2, 4.0)),
        ));
        out.push((
            Genre::Trance,
            trapezoid(bpm, 130.0, 134.0, 142.0, 148.0)
                * (0.4 + 0.6 * high_mid)
                * trapezoid(bright, 1200.0, 2200.0, 6000.0, 9000.0),
        ));
        out.push((
            Genre::KawaiiFutureBass,
            trapezoid(bpm, 130.0, 140.0, 165.0, 175.0)
                * trapezoid(bright, 2500.0, 4000.0, 12000.0, 16000.0)
                * (0.3 + 0.7 * high)
                * (1.0 - 0.5 * trapezoid(f.kick_density, 2.4, 3.0, 10.0, 12.0)),
        ));
        out.push((
            Genre::Dubstep,
            trapezoid(bpm, 132.0, 138.0, 146.0, 152.0)
                * (0.3 + 0.7 * low)
                * trapezoid(f.kick_density, 0.0, 0.0, 1.4, 2.0),
        ));
        // DnB rides a breakbeat (sparse kicks) while hardcore at the same
        // tempo runs four-on-the-floor (kick_density ~ bpm/60).
        out.push((
            Genre::DrumAndBass,
            trapezoid(bpm, 160.0, 168.0, 180.0, 188.0)
                * (0.4 + 0.6 * low)
                * trapezoid(f.kick_density, 0.4, 0.9, 2.2, 2.8),
        ));
        out.push((
            Genre::Hardcore,
            trapezoid(bpm, 150.0, 160.0, 200.0, 220.0)
                * (0.3 + 0.7 * trapezoid(f.kick_density, 2.0, 2.6, 10.0, 12.0))
                * (0.5 + 0.5 * low_mid),
        ));
        out.push((
            Genre::HipHop,
            trapezoid(bpm, 72.0, 80.0, 100.0, 108.0) * (0.4 + 0.6 * low),
        ));
        out
    }
}

impl GenreClassifier for RuleBasedClassifier {
    fn update(&mut self, features: &Features) -> Option<Genre> {
        // Exponential decay so scores follow the music with ~5 s memory.
        for v in self.scores.values_mut() {
            *v *= 0.82;
        }
        for (g, s) in Self::instant_scores(features) {
            *self.scores.entry(g).or_insert(0.0) += s;
        }

        let (&best, &best_score) = self
            .scores
            .iter()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .unwrap_or((&Genre::Unknown, &0.0));

        if best_score < 0.5 {
            return None;
        }
        if best == self.current {
            self.challenger = None;
            return None;
        }
        let current_score = self.scores.get(&self.current).copied().unwrap_or(0.0);
        if best_score > current_score * 1.4 + 0.2 {
            let count = match self.challenger {
                Some((g, c)) if g == best => c + 1,
                _ => 1,
            };
            if count >= self.switch_after {
                self.current = best;
                self.challenger = None;
                return Some(best);
            }
            self.challenger = Some((best, count));
        } else {
            self.challenger = None;
        }
        None
    }

    fn current(&self) -> Genre {
        self.current
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed(c: &mut RuleBasedClassifier, f: &Features, n: usize) -> Option<Genre> {
        let mut last = None;
        for _ in 0..n {
            if let Some(g) = c.update(f) {
                last = Some(g);
            }
        }
        last
    }

    fn deep_house() -> Features {
        Features {
            bpm: 122.0,
            bpm_confidence: 0.6,
            band_energy: [0.8, 0.4, 0.3, 0.2],
            kick_density: 2.0,
            hat_density: 2.0,
            centroid: 900.0,
            intensity: 0.6,
        }
    }

    fn hardcore() -> Features {
        Features {
            bpm: 180.0,
            bpm_confidence: 0.7,
            band_energy: [0.9, 0.7, 0.5, 0.4],
            kick_density: 3.0,
            hat_density: 3.0,
            centroid: 2500.0,
            intensity: 0.9,
        }
    }

    fn kawaii() -> Features {
        Features {
            bpm: 150.0,
            bpm_confidence: 0.6,
            band_energy: [0.5, 0.4, 0.6, 0.8],
            kick_density: 1.5,
            hat_density: 4.0,
            centroid: 6000.0,
            intensity: 0.7,
        }
    }

    #[test]
    fn classifies_representative_genres() {
        let mut c = RuleBasedClassifier::default();
        assert_eq!(feed(&mut c, &deep_house(), 20), Some(Genre::DeepHouse));

        let mut c = RuleBasedClassifier::default();
        assert_eq!(feed(&mut c, &hardcore(), 20), Some(Genre::Hardcore));

        let mut c = RuleBasedClassifier::default();
        assert_eq!(feed(&mut c, &kawaii(), 20), Some(Genre::KawaiiFutureBass));
    }

    #[test]
    fn hysteresis_prevents_flapping() {
        let mut c = RuleBasedClassifier::default();
        feed(&mut c, &deep_house(), 20);
        assert_eq!(c.current(), Genre::DeepHouse);
        // A couple of odd readings must not flip the genre immediately.
        assert_eq!(feed(&mut c, &hardcore(), 3), None);
        assert_eq!(c.current(), Genre::DeepHouse);
        // Sustained change eventually switches.
        assert_eq!(feed(&mut c, &hardcore(), 30), Some(Genre::Hardcore));
    }
}
