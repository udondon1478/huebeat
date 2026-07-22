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
    /// High-mid-band beats per second (snare / clap density).
    pub snare_density: f32,
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
        if bpm > 0.0 && f.kick_ibi_bpm > 0.0 {
            let ratio = f.kick_ibi_bpm / bpm;
            // A regular grid is the strongest telltale; a kick *rate*
            // near 2x the detected beat rate backs it up when the grid
            // reading is noisy (sloppy detection in a real room must not
            // leave club tracks stranded at half tempo).
            let rate_ratio = f.kick_density * 60.0 / bpm;
            if (1.8..=2.2).contains(&ratio)
                && (f.kick_regularity > 0.5 || rate_ratio > 1.6)
            {
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
        // Warm midrange mixes (disco, garage) sit between the two.
        let warm = trapezoid(bright, 1200.0, 2200.0, 4200.0, 6000.0);
        // Hat activity in three broad flavors.
        let hats_sparse = trapezoid(f.hat_density, 0.0, 0.0, 2.0, 3.5);
        let hats_groove = trapezoid(f.hat_density, 0.5, 1.5, 8.0, 10.0);
        let hats_busy = trapezoid(f.hat_density, 2.0, 4.0, 14.0, 18.0);
        let snares = trapezoid(f.snare_density, 0.3, 0.8, 6.0, 9.0);
        // How dominant the low band is relative to the rest of the mix.
        let total_energy = low + low_mid + high_mid + high;
        let bass_heavy = if total_energy > 1e-3 {
            trapezoid(low / total_energy, 0.2, 0.35, 1.0, 1.01)
        } else {
            0.0
        };
        let quiet = trapezoid(f.intensity, 0.0, 0.0, 0.45, 0.7);
        let loud = trapezoid(f.intensity, 0.35, 0.6, 1.0, 1.01);

        // Every genre uses the same shape — tempo gate x weighted evidence
        // mean — so no genre wins just by having fewer conditions.
        let mut push = |g: Genre, tempo: f32, ev: f32| {
            out.push((g, tempo * (0.25 + 0.75 * ev)));
        };

        // ---- Four-on-the-floor family --------------------------------
        push(
            Genre::DeepHouse,
            trapezoid(bpm, 108.0, 115.0, 124.0, 127.0),
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
                (0.6, hats_groove),
            ]),
        );
        // Tech house: tighter and darker than house, groovier than techno.
        push(
            Genre::TechHouse,
            trapezoid(bpm, 119.0, 123.0, 128.0, 131.0),
            evidence(&[
                (1.2, four_floor),
                (1.0, bass_heavy),
                (0.8, hats_groove),
                (0.5, dark),
                (0.4, snares),
            ]),
        );
        // Big-room EDM: bright, loud, anthemic leads in the high mids.
        push(
            Genre::ElectroHouse,
            trapezoid(bpm, 124.0, 126.0, 132.0, 136.0),
            evidence(&[
                (1.2, four_floor),
                (1.0, loud),
                (0.8, bright_hi),
                (0.8, low_mid),
            ]),
        );
        // Disco / nu-disco: warm midrange, open-hat groove, mid tempo.
        push(
            Genre::NuDisco,
            trapezoid(bpm, 100.0, 108.0, 120.0, 124.0),
            evidence(&[
                (1.0, four_floor),
                (1.0, warm),
                (0.8, hats_groove),
                (0.4, low_mid),
            ]),
        );
        // Netlabel J-electropop (tofubeats, Maltine-style): vocal-forward
        // bright pop on a mid-tempo four-on-floor with poppy drum fills.
        push(
            Genre::NetPop,
            trapezoid(bpm, 104.0, 110.0, 124.0, 128.0),
            evidence(&[
                (1.2, high_mid),
                (0.8, four_floor),
                (0.8, snares),
                (0.6, bright_hi),
                (0.4, hats_groove),
            ]),
        );
        // UKG / 2-step: shuffled kicks (not a clean grid) with busy bright
        // hats and prominent snares.
        push(
            Genre::UkGarage,
            trapezoid(bpm, 126.0, 129.0, 136.0, 140.0),
            evidence(&[
                (1.0, trapezoid(f.kick_regularity, 0.15, 0.3, 0.7, 0.85)),
                (1.0, hats_busy),
                (0.8, snares),
                (0.6, warm),
            ]),
        );
        // Jersey club: the signature bounce is a *burst* of kicks (the
        // 5-kick pattern) that is dense but not an even grid.
        push(
            Genre::JerseyClub,
            trapezoid(bpm, 130.0, 135.0, 146.0, 150.0),
            evidence(&[
                (1.2, trapezoid(f.kick_density, 2.4, 3.0, 6.0, 7.0)),
                (1.0, trapezoid(f.kick_regularity, 0.1, 0.25, 0.6, 0.75)),
                (0.6, high_mid),
                (0.6, snares),
                (0.5, low),
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
        // Trance is hypnotic and sustained: bright pads, few pop fills —
        // the low-snare term keeps vocal anison remixes from reading as
        // trance.
        push(
            Genre::Trance,
            trapezoid(bpm, 130.0, 134.0, 143.0, 149.0),
            evidence(&[
                (1.0, four_floor),
                (1.0, bright_hi),
                (0.8, high_mid),
                (0.5, 1.0 - snares),
            ]),
        );
        // Psytrance: relentless machine-regular kick with a rolling
        // bassline filling every gap — bass-heavy and hypnotically even.
        push(
            Genre::Psytrance,
            trapezoid(bpm, 134.0, 138.0, 148.0, 152.0),
            evidence(&[
                (1.2, four_floor),
                (1.0, trapezoid(f.kick_regularity, 0.6, 0.8, 1.0, 1.01)),
                (1.0, bass_heavy),
                (0.6, hats_busy),
            ]),
        );
        // Hardstyle: distorted kick dumps energy into the low mids.
        push(
            Genre::Hardstyle,
            trapezoid(bpm, 142.0, 147.0, 158.0, 163.0),
            evidence(&[
                (1.2, four_floor),
                (1.2, low_mid),
                (0.8, loud),
                (0.5, low),
            ]),
        );
        // Eurobeat / para-para: bright and busy at 150-160, four-on-floor.
        push(
            Genre::Eurobeat,
            trapezoid(bpm, 146.0, 150.0, 162.0, 166.0),
            evidence(&[
                (1.2, four_floor),
                (1.0, bright_hi),
                (0.8, hats_busy),
                (0.6, high_mid),
            ]),
        );
        // Anison / J-pop remix: a vocal-dominated bright pop mix pushed
        // onto a club four-on-floor, with poppy fills and claps.
        push(
            Genre::AnisonRemix,
            trapezoid(bpm, 126.0, 130.0, 146.0, 150.0),
            evidence(&[
                (1.2, high_mid),
                (1.0, four_floor),
                (0.8, bright_hi),
                (0.8, snares),
                (0.4, hats_groove),
            ]),
        );
        // Future core (J-core x future bass): hardcore-tempo four-on-floor
        // but melodic and bright where hardcore is dark and percussive.
        push(
            Genre::FutureCore,
            trapezoid(bpm, 162.0, 168.0, 182.0, 188.0),
            evidence(&[
                (1.2, four_floor),
                (1.2, bright_hi),
                (0.8, high_mid),
                (0.5, trapezoid(f.kick_density, 2.2, 2.6, 3.4, 3.8)),
            ]),
        );
        push(
            Genre::Hardcore,
            trapezoid(bpm, 155.0, 162.0, 196.0, 215.0),
            evidence(&[
                (1.2, four_floor),
                (1.0, trapezoid(f.kick_density, 2.0, 2.6, 10.0, 12.0)),
                (0.6, low_mid),
                (0.4, f.intensity),
            ]),
        );

        // ---- Breakbeat family ----------------------------------------
        push(
            Genre::Breakbeat,
            trapezoid(bpm, 120.0, 126.0, 138.0, 142.0),
            evidence(&[
                (1.2, breakbeat),
                (0.8, low),
                (0.8, snares),
                (0.4, warm),
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
        // Dubstep: half-time feel — heavy lows but sparse, non-grid kicks
        // and sparse hats (trap at the same tempo rolls busy hats).
        push(
            Genre::Dubstep,
            trapezoid(bpm, 132.0, 136.0, 146.0, 152.0),
            evidence(&[
                (1.0, breakbeat),
                (1.0, low),
                (0.8, trapezoid(f.kick_density, 0.0, 0.1, 1.5, 2.2)),
                (0.8, hats_sparse),
            ]),
        );
        // Trap: sparse 808 kicks under machine-gun hat rolls.
        push(
            Genre::Trap,
            trapezoid(bpm, 128.0, 134.0, 158.0, 165.0),
            evidence(&[
                (1.2, hats_busy),
                (1.0, bass_heavy),
                (0.8, breakbeat),
                (0.8, trapezoid(f.kick_density, 0.1, 0.3, 1.6, 2.4)),
            ]),
        );
        // Hyperflip / dariacore: jersey-derived kicks buried under
        // hyper-chopped samples — everything is maximal at once.
        push(
            Genre::Hyperflip,
            trapezoid(bpm, 138.0, 146.0, 164.0, 172.0),
            evidence(&[
                (1.2, trapezoid(f.snare_density, 2.0, 3.5, 12.0, 16.0)),
                (1.0, hats_busy),
                (0.8, bright_hi),
                (0.6, loud),
                (0.6, breakbeat),
                (0.5, trapezoid(f.kick_density, 1.8, 2.4, 6.0, 7.0)),
            ]),
        );
        // Future bass: bright supersaw chords, mid-bright but not the
        // ultra-bright sparkle of kawaii.
        push(
            Genre::FutureBass,
            trapezoid(bpm, 130.0, 138.0, 158.0, 168.0),
            evidence(&[
                (1.2, trapezoid(bright, 2200.0, 3000.0, 4800.0, 6500.0)),
                (1.0, high_mid),
                (0.8, breakbeat),
                (0.4, snares),
            ]),
        );
        push(
            Genre::KawaiiFutureBass,
            trapezoid(bpm, 128.0, 138.0, 160.0, 172.0),
            evidence(&[
                (1.2, trapezoid(bright, 3200.0, 4800.0, 12000.0, 16000.0)),
                (1.0, high),
                (0.5, 1.0 - 0.5 * four_floor),
            ]),
        );

        // ---- Downtempo family ----------------------------------------
        // Hip hop / R&B demand *positive* syncopation evidence: kicks
        // that were actually detected and dodge the grid. Without kick
        // data — or with a four-on-the-floor grid — the gate closes,
        // because a half-tempo BPM lock on a club track would otherwise
        // read as hip hop far too often.
        let syncopation = if f.kick_ibi_bpm > 0.0 { breakbeat } else { 0.0 };
        push(
            Genre::HipHop,
            trapezoid(bpm, 80.0, 86.0, 98.0, 104.0) * syncopation,
            evidence(&[
                (1.0, low),
                (0.8, dark),
                (0.8, trapezoid(f.kick_density, 0.4, 0.8, 2.2, 3.0)),
                // Busy 16th hats scream club music, not boom bap.
                (0.6, 1.0 - trapezoid(f.hat_density, 3.0, 5.0, 20.0, 24.0)),
            ]),
        );
        // R&B: slower and smoother than hip hop — softer drums, less
        // intensity, sparse hats.
        push(
            Genre::Rnb,
            trapezoid(bpm, 55.0, 62.0, 82.0, 88.0) * syncopation,
            evidence(&[
                (1.0, quiet),
                (0.8, dark),
                (0.6, hats_sparse),
            ]),
        );
        // Reggaeton: dembow — the kick stays on the grid (like a slow
        // four-on-floor) while snares syncopate over it.
        push(
            Genre::Reggaeton,
            trapezoid(bpm, 84.0, 90.0, 100.0, 105.0),
            evidence(&[
                (1.2, four_floor),
                (1.0, low),
                (0.8, snares),
                (0.5, warm),
            ]),
        );
        // Synthwave: steady mid-slow pulse with a dark analog wash.
        push(
            Genre::Synthwave,
            trapezoid(bpm, 82.0, 90.0, 112.0, 118.0),
            evidence(&[
                (0.8, four_floor),
                (1.0, dark),
                (0.8, low_mid),
                (0.5, trapezoid(f.kick_density, 0.5, 1.0, 2.2, 3.0)),
            ]),
        );
        out
    }

    /// Current smoothed scores, best first, normalized to shares of the
    /// total (0..1). Exposed so the UI can show why a genre was picked.
    pub fn ranked(&self, n: usize) -> Vec<(Genre, f32)> {
        let total: f32 = self.scores.values().filter(|v| **v > 0.0).sum();
        if total <= 1e-6 {
            return Vec::new();
        }
        let mut v: Vec<(Genre, f32)> = self
            .scores
            .iter()
            .filter(|(_, s)| **s > 0.01)
            .map(|(g, s)| (*g, *s / total))
            .collect();
        v.sort_by(|a, b| b.1.total_cmp(&a.1));
        v.truncate(n);
        v
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

    fn classify(f: &Features) -> Option<Genre> {
        let mut c = RuleBasedClassifier::default();
        feed(&mut c, f, 20)
    }

    fn deep_house() -> Features {
        Features {
            bpm: 121.0,
            bpm_confidence: 0.6,
            band_energy: [0.8, 0.4, 0.3, 0.2],
            kick_density: 2.0,
            snare_density: 0.5,
            hat_density: 2.0,
            kick_ibi_bpm: 121.0,
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
            snare_density: 2.0,
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
            snare_density: 1.5,
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
            snare_density: 1.0,
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
            snare_density: 2.0,
            hat_density: 3.0,
            kick_ibi_bpm: 174.0,
            kick_regularity: 0.85,
            centroid: 2500.0,
            intensity: 0.9,
        }
    }

    /// 172 BPM four-on-floor club track whose tempo detector locked onto
    /// 86 and whose kick grid reads noisy (regularity below the old 0.5
    /// octave-correction cutoff) — the real-world "everything is hip hop"
    /// failure mode.
    fn half_tempo_sloppy_club() -> Features {
        Features {
            bpm: 86.0,
            bpm_confidence: 0.55,
            band_energy: [0.85, 0.6, 0.45, 0.35],
            // Kick rate ~2x the detected beat rate backs up the IBI cue.
            kick_density: 2.7,
            snare_density: 1.5,
            hat_density: 4.0,
            kick_ibi_bpm: 168.0,
            kick_regularity: 0.45,
            centroid: 2300.0,
            intensity: 0.8,
        }
    }

    /// Dark low-heavy playback where beat detection found no usable kick
    /// grid: without positive syncopation evidence hip hop must not fire.
    fn no_kick_info_dark() -> Features {
        Features {
            bpm: 92.0,
            bpm_confidence: 0.4,
            band_energy: [0.8, 0.5, 0.3, 0.2],
            kick_density: 0.6,
            snare_density: 0.2,
            hat_density: 1.0,
            kick_ibi_bpm: 0.0,
            kick_regularity: 0.0,
            centroid: 1800.0,
            intensity: 0.5,
        }
    }

    fn house_128() -> Features {
        Features {
            bpm: 128.0,
            bpm_confidence: 0.7,
            band_energy: [0.8, 0.5, 0.6, 0.4],
            kick_density: 2.1,
            snare_density: 1.5,
            hat_density: 4.0,
            kick_ibi_bpm: 128.0,
            kick_regularity: 0.9,
            centroid: 3200.0,
            intensity: 0.7,
        }
    }

    fn psytrance() -> Features {
        Features {
            bpm: 142.0,
            bpm_confidence: 0.7,
            // Rolling bass keeps the low band saturated.
            band_energy: [0.95, 0.4, 0.35, 0.3],
            kick_density: 2.4,
            snare_density: 0.8,
            hat_density: 6.0,
            kick_ibi_bpm: 142.0,
            kick_regularity: 0.95,
            centroid: 2600.0,
            intensity: 0.8,
        }
    }

    fn eurobeat() -> Features {
        Features {
            bpm: 156.0,
            bpm_confidence: 0.7,
            band_energy: [0.6, 0.5, 0.7, 0.6],
            kick_density: 2.6,
            snare_density: 2.5,
            hat_density: 7.0,
            kick_ibi_bpm: 156.0,
            kick_regularity: 0.85,
            centroid: 4500.0,
            intensity: 0.8,
        }
    }

    fn trap() -> Features {
        Features {
            bpm: 140.0,
            bpm_confidence: 0.5,
            // 808 sub dominates; hats roll constantly, kicks are sparse.
            band_energy: [0.9, 0.3, 0.25, 0.35],
            kick_density: 0.8,
            snare_density: 1.2,
            hat_density: 8.0,
            kick_ibi_bpm: 100.0,
            kick_regularity: 0.25,
            centroid: 2400.0,
            intensity: 0.6,
        }
    }

    fn reggaeton() -> Features {
        Features {
            bpm: 95.0,
            bpm_confidence: 0.6,
            band_energy: [0.85, 0.45, 0.4, 0.3],
            kick_density: 1.6,
            snare_density: 2.5,
            hat_density: 2.0,
            kick_ibi_bpm: 95.0,
            kick_regularity: 0.85,
            centroid: 2800.0,
            intensity: 0.65,
        }
    }

    fn hardstyle() -> Features {
        Features {
            bpm: 152.0,
            bpm_confidence: 0.7,
            // Distorted kick dumps energy into the low mids.
            band_energy: [0.8, 0.9, 0.5, 0.35],
            kick_density: 2.5,
            snare_density: 1.0,
            hat_density: 3.0,
            kick_ibi_bpm: 152.0,
            kick_regularity: 0.9,
            centroid: 2200.0,
            intensity: 0.85,
        }
    }

    fn future_core() -> Features {
        Features {
            bpm: 175.0,
            bpm_confidence: 0.7,
            band_energy: [0.7, 0.5, 0.7, 0.6],
            kick_density: 2.9,
            snare_density: 2.0,
            hat_density: 5.0,
            kick_ibi_bpm: 175.0,
            kick_regularity: 0.9,
            centroid: 4500.0,
            intensity: 0.85,
        }
    }

    fn jersey_club() -> Features {
        Features {
            bpm: 140.0,
            bpm_confidence: 0.6,
            band_energy: [0.7, 0.5, 0.6, 0.5],
            // The 5-kick bounce: dense bursts that don't sit on a grid.
            kick_density: 3.6,
            snare_density: 2.5,
            hat_density: 4.0,
            kick_ibi_bpm: 210.0,
            kick_regularity: 0.45,
            centroid: 3200.0,
            intensity: 0.7,
        }
    }

    fn hyperflip() -> Features {
        Features {
            bpm: 155.0,
            bpm_confidence: 0.6,
            band_energy: [0.7, 0.6, 0.7, 0.7],
            // Everything maximal at once: chops, snares, hats.
            kick_density: 3.0,
            snare_density: 4.5,
            hat_density: 9.0,
            kick_ibi_bpm: 200.0,
            kick_regularity: 0.35,
            centroid: 4200.0,
            intensity: 0.9,
        }
    }

    fn anison_remix() -> Features {
        Features {
            bpm: 138.0,
            bpm_confidence: 0.6,
            // Vocals dominate the high mids over a club four-on-floor.
            band_energy: [0.65, 0.5, 0.8, 0.55],
            kick_density: 2.3,
            snare_density: 3.0,
            hat_density: 5.0,
            kick_ibi_bpm: 138.0,
            kick_regularity: 0.85,
            centroid: 3800.0,
            intensity: 0.75,
        }
    }

    fn net_pop() -> Features {
        Features {
            bpm: 122.0,
            bpm_confidence: 0.6,
            band_energy: [0.6, 0.5, 0.75, 0.5],
            kick_density: 2.0,
            snare_density: 2.2,
            hat_density: 4.0,
            kick_ibi_bpm: 122.0,
            kick_regularity: 0.8,
            centroid: 3600.0,
            intensity: 0.65,
        }
    }

    #[test]
    fn classifies_japanese_club_genres() {
        assert_eq!(classify(&future_core()), Some(Genre::FutureCore));
        assert_eq!(classify(&jersey_club()), Some(Genre::JerseyClub));
        assert_eq!(classify(&hyperflip()), Some(Genre::Hyperflip));
        assert_eq!(classify(&anison_remix()), Some(Genre::AnisonRemix));
        assert_eq!(classify(&net_pop()), Some(Genre::NetPop));
    }

    #[test]
    fn classifies_representative_genres() {
        assert_eq!(classify(&deep_house()), Some(Genre::DeepHouse));
        assert_eq!(classify(&hardcore()), Some(Genre::Hardcore));
        assert_eq!(classify(&kawaii()), Some(Genre::KawaiiFutureBass));
        assert_eq!(classify(&hip_hop()), Some(Genre::HipHop));
    }

    #[test]
    fn classifies_new_genres() {
        assert_eq!(classify(&psytrance()), Some(Genre::Psytrance));
        assert_eq!(classify(&eurobeat()), Some(Genre::Eurobeat));
        assert_eq!(classify(&trap()), Some(Genre::Trap));
        assert_eq!(classify(&reggaeton()), Some(Genre::Reggaeton));
        assert_eq!(classify(&hardstyle()), Some(Genre::Hardstyle));
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
        assert_eq!(classify(&half_tempo_hardcore()), Some(Genre::Hardcore));
    }

    #[test]
    fn sloppy_half_tempo_club_never_reads_as_hip_hop() {
        // Even when the kick grid reads too noisy for the regularity
        // cutoff, the kick-rate fallback must octave-correct instead of
        // dumping the track into hip hop.
        let mut c = RuleBasedClassifier::default();
        let got = feed(&mut c, &half_tempo_sloppy_club(), 20);
        assert!(got.is_some(), "should classify something");
        assert_ne!(c.current(), Genre::HipHop, "got {:?}", c.current());
    }

    #[test]
    fn no_kick_data_never_reads_as_hip_hop() {
        let mut c = RuleBasedClassifier::default();
        feed(&mut c, &no_kick_info_dark(), 20);
        assert_ne!(c.current(), Genre::HipHop, "got {:?}", c.current());
    }

    #[test]
    fn ranked_returns_normalized_shares() {
        let mut c = RuleBasedClassifier::default();
        feed(&mut c, &deep_house(), 20);
        let ranked = c.ranked(3);
        assert!(!ranked.is_empty());
        assert_eq!(ranked[0].0, Genre::DeepHouse);
        assert!(ranked[0].1 > 0.0 && ranked[0].1 <= 1.0);
        // Shares are descending.
        for w in ranked.windows(2) {
            assert!(w[0].1 >= w[1].1);
        }
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
