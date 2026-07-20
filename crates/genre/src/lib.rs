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
    /// BPM implied by the median inter-kick interval; 0 when unknown.
    pub kick_ibi_bpm: f32,
    /// 0..1: how periodic the kick intervals are.
    pub kick_regularity: f32,
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

/// Weighted mean of 0..1 evidence terms.
fn evidence(terms: &[(f32, f32)]) -> f32 {
    let wsum: f32 = terms.iter().map(|(w, _)| w).sum();
    if wsum <= 0.0 {
        return 0.0;
    }
    terms.iter().map(|(w, v)| w * v.clamp(0.0, 1.0)).sum::<f32>() / wsum
}

impl RuleBasedClassifier {
    /// Octave-corrected tempo plus a 0..1 four-on-the-floor score.
    ///
    /// The autocorrelation tempo detector can lock onto half the true
    /// tempo; a regular kick grid running at ~2x the detected BPM is the
    /// telltale, so we trust the kicks and double the tempo. This is what
    /// used to dump 160-200 BPM tracks into the hip-hop range.
    fn rhythm_context(f: &Features) -> (f32, f32) {
        let mut bpm = f.bpm;
        if bpm > 0.0 && f.kick_ibi_bpm > 0.0 && f.kick_regularity > 0.5 {
            let ratio = f.kick_ibi_bpm / bpm;
            if (1.8..=2.2).contains(&ratio) {
                bpm *= 2.0;
            }
        }
        let four_floor = if bpm > 0.0 && f.kick_ibi_bpm > 0.0 {
            f.kick_regularity * trapezoid(f.kick_ibi_bpm / bpm, 0.82, 0.92, 1.08, 1.22)
        } else {
            0.0
        };
        (bpm, four_floor)
    }

    fn instant_scores(f: &Features) -> Vec<(Genre, f32)> {
        let (bpm, four_floor) = Self::rhythm_context(f);
        let [low, low_mid, high_mid, high] = f.band_energy;
        let bright = f.centroid;
        let mut out = Vec::new();

        if f.intensity < 0.08 || f.bpm_confidence < 0.05 {
            out.push((Genre::Ambient, 0.6));
            return out;
        }

        let breakbeat = 1.0 - four_floor;
        // Real-world full-mix centroids sit around 2-5 kHz, so "dark" and
        // "bright" are calibrated against that, not against pure tones.
        let dark = trapezoid(bright, 0.0, 0.0, 2800.0, 5500.0);
        let bright_hi = trapezoid(bright, 1800.0, 3200.0, 9000.0, 14000.0);

        // Every genre uses the same shape — tempo gate x weighted evidence
        // mean — so no genre wins just by having fewer conditions.
        let mut push = |g: Genre, tempo: f32, ev: f32| {
            out.push((g, tempo * (0.25 + 0.75 * ev)));
        };

        push(
            Genre::DeepHouse,
            trapezoid(bpm, 112.0, 117.0, 124.0, 127.0),
            evidence(&[
                (1.0, low),
                (1.0, four_floor),
                (1.0, dark),
                (0.5, trapezoid(f.hat_density, 0.0, 0.5, 4.0, 6.0)),
            ]),
        );
        push(
            Genre::House,
            trapezoid(bpm, 118.0, 122.0, 130.0, 134.0),
            evidence(&[
                (1.2, four_floor),
                (0.8, high_mid),
                (0.6, trapezoid(f.hat_density, 0.5, 1.5, 8.0, 10.0)),
            ]),
        );
        push(
            Genre::Techno,
            trapezoid(bpm, 124.0, 128.0, 142.0, 148.0),
            evidence(&[
                (1.2, four_floor),
                (1.0, low),
                (0.8, dark),
                (0.5, trapezoid(f.kick_density, 1.4, 1.9, 3.2, 3.8)),
            ]),
        );
        push(
            Genre::Trance,
            trapezoid(bpm, 130.0, 134.0, 143.0, 149.0),
            evidence(&[
                (1.0, four_floor),
                (1.0, bright_hi),
                (0.8, high_mid),
            ]),
        );
        push(
            Genre::KawaiiFutureBass,
            trapezoid(bpm, 128.0, 138.0, 160.0, 172.0),
            evidence(&[
                (1.2, trapezoid(bright, 2500.0, 4500.0, 12000.0, 16000.0)),
                (1.0, high),
                (0.5, 1.0 - 0.5 * four_floor),
            ]),
        );
        // Dubstep: half-time feel — heavy lows but sparse, non-grid kicks.
        push(
            Genre::Dubstep,
            trapezoid(bpm, 132.0, 136.0, 146.0, 152.0),
            evidence(&[
                (1.0, breakbeat),
                (1.0, low),
                (0.8, trapezoid(f.kick_density, 0.0, 0.1, 1.5, 2.2)),
            ]),
        );
        // DnB rides a breakbeat; hardcore at the same tempo is 4-on-floor.
        push(
            Genre::DrumAndBass,
            trapezoid(bpm, 160.0, 168.0, 180.0, 192.0),
            evidence(&[
                (1.2, breakbeat),
                (1.0, low),
                (0.7, trapezoid(f.hat_density, 1.0, 2.5, 10.0, 12.0)),
            ]),
        );
        push(
            Genre::Hardcore,
            trapezoid(bpm, 148.0, 158.0, 196.0, 215.0),
            evidence(&[
                (1.2, four_floor),
                (1.0, trapezoid(f.kick_density, 2.0, 2.6, 10.0, 12.0)),
                (0.6, low_mid),
                (0.4, f.intensity),
            ]),
        );
        // Hip hop is the syncopated-kick genre; a four-on-the-floor grid
        // in this tempo range means we mis-detected half tempo instead.
        push(
            Genre::HipHop,
            trapezoid(bpm, 76.0, 84.0, 100.0, 106.0),
            evidence(&[
                (1.2, breakbeat),
                (1.0, low),
                (0.8, dark),
                (0.8, trapezoid(f.kick_density, 0.4, 0.8, 2.2, 3.0)),
            ]),
        );
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
            kick_ibi_bpm: 122.0,
            kick_regularity: 0.85,
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
            kick_ibi_bpm: 180.0,
            kick_regularity: 0.9,
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
            kick_ibi_bpm: 150.0,
            kick_regularity: 0.5,
            centroid: 6000.0,
            intensity: 0.7,
        }
    }

    fn hip_hop() -> Features {
        Features {
            bpm: 92.0,
            bpm_confidence: 0.5,
            band_energy: [0.85, 0.5, 0.35, 0.3],
            kick_density: 1.2,
            hat_density: 3.0,
            // Syncopated kicks: irregular, not locked to the beat grid.
            kick_ibi_bpm: 130.0,
            kick_regularity: 0.3,
            centroid: 2000.0,
            intensity: 0.6,
        }
    }

    /// 174 BPM four-on-floor track whose tempo detector locked onto 87.
    fn half_tempo_hardcore() -> Features {
        Features {
            bpm: 87.0,
            bpm_confidence: 0.6,
            band_energy: [0.9, 0.7, 0.5, 0.4],
            kick_density: 2.9,
            hat_density: 3.0,
            kick_ibi_bpm: 174.0,
            kick_regularity: 0.85,
            centroid: 2500.0,
            intensity: 0.9,
        }
    }

    fn house_128() -> Features {
        Features {
            bpm: 128.0,
            bpm_confidence: 0.7,
            band_energy: [0.8, 0.5, 0.6, 0.4],
            kick_density: 2.1,
            hat_density: 4.0,
            kick_ibi_bpm: 128.0,
            kick_regularity: 0.9,
            centroid: 3200.0,
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

        let mut c = RuleBasedClassifier::default();
        assert_eq!(feed(&mut c, &hip_hop(), 20), Some(Genre::HipHop));
    }

    #[test]
    fn four_on_floor_never_reads_as_hip_hop() {
        // A 128 BPM 4-on-floor club track must not land on hip hop.
        let mut c = RuleBasedClassifier::default();
        let got = feed(&mut c, &house_128(), 20);
        assert!(got.is_some(), "should classify something");
        assert_ne!(c.current(), Genre::HipHop, "got {:?}", c.current());
    }

    #[test]
    fn half_tempo_detection_is_octave_corrected() {
        // Tempo detector says 87 but the kick grid runs at 174: the
        // classifier must correct the octave instead of calling it hip hop.
        let mut c = RuleBasedClassifier::default();
        assert_eq!(feed(&mut c, &half_tempo_hardcore(), 20), Some(Genre::Hardcore));
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
