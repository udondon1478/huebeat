//! Real-time music analysis: multi-band onset (beat) detection via
//! per-band spectral flux with adaptive thresholds, BPM estimation via
//! autocorrelation of the onset envelope, plus loudness/intensity tracking.

use std::collections::VecDeque;
use std::sync::Arc;

use core_types::{
    AnalysisFrame, Band, BandBeatEvent, BandConfig, TempoEstimate, TempoSource, ThresholdMode,
};
use rustfft::num_complex::Complex;
use rustfft::Fft;

pub const FFT_SIZE: usize = 2048;
pub const HOP_SIZE: usize = 512;
/// UI spectrum resolution (log-spaced bins).
pub const SPECTRUM_BINS: usize = 32;

const FLUX_HISTORY: usize = 128;
const ONSET_ENVELOPE_LEN: usize = 1024; // ~11 s at 93.75 Hz hop rate
const TEMPO_RECALC_HOPS: usize = 47; // ~0.5 s
const BPM_MIN: f32 = 60.0;
const BPM_MAX: f32 = 200.0;

#[derive(Debug, Clone)]
pub struct AnalyzerConfig {
    pub bands: BandConfig,
    /// Threshold in standard deviations above mean flux; lower = more
    /// sensitive. Per band.
    pub sensitivity: [f32; 4],
    /// Refractory period per band in ms.
    pub min_interval_ms: [f32; 4],
    /// Only detect beats in the low band (LightBeat's "bass only" mode).
    pub low_only: bool,
    /// Adaptive (Auto) vs fixed (Manual) beat threshold.
    pub threshold_mode: ThresholdMode,
    /// Fixed per-band threshold in raw flux units, used in Manual mode;
    /// <= 0 means unset and falls back to the adaptive threshold.
    pub manual_threshold: [f32; 4],
}

impl Default for AnalyzerConfig {
    fn default() -> Self {
        Self {
            bands: BandConfig::default(),
            sensitivity: [2.2, 2.4, 2.4, 2.4],
            min_interval_ms: [150.0, 100.0, 100.0, 80.0],
            low_only: false,
            threshold_mode: ThresholdMode::Auto,
            manual_threshold: [0.0; 4],
        }
    }
}

/// Result of feeding one hop of audio.
pub struct HopOutput {
    pub frame: AnalysisFrame,
    pub beats: Vec<BandBeatEvent>,
    pub tempo: Option<TempoEstimate>,
}

struct BandState {
    bin_range: (usize, usize),
    flux_history: VecDeque<f32>,
    last_beat_ms: f64,
    energy_max: f32,
    /// Slow-decay flux peak used to normalize flux/threshold for the UI.
    flux_max: f32,
}

pub struct Analyzer {
    config: AnalyzerConfig,
    sample_rate: f32,
    fft: Arc<dyn Fft<f32>>,
    window: Vec<f32>,
    fft_buf: Vec<Complex<f32>>,
    scratch: Vec<Complex<f32>>,
    /// Sliding input buffer of FFT_SIZE samples.
    input: Vec<f32>,
    pending: Vec<f32>,
    prev_mag: Vec<f32>,
    bands: Vec<BandState>,
    total_samples: u64,
    hop_count: u64,
    // BPM
    onset_env: VecDeque<f32>,
    bpm_smoothed: f32,
    bpm_confidence: f32,
    bpm_outlier_count: u32,
    // Intensity / AGC
    rms_env: f32,
    intensity: f32,
    rms_max: f32,
    spectrum_max: f32,
    spectrum_ranges: Vec<(usize, usize)>,
}

impl Analyzer {
    pub fn new(sample_rate: u32, config: AnalyzerConfig) -> Self {
        let sr = sample_rate as f32;
        let mut planner = rustfft::FftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(FFT_SIZE);
        let scratch_len = fft.get_inplace_scratch_len();
        let window: Vec<f32> = (0..FFT_SIZE)
            .map(|i| {
                let x = std::f32::consts::PI * 2.0 * i as f32 / FFT_SIZE as f32;
                0.5 * (1.0 - x.cos())
            })
            .collect();

        let hz_per_bin = sr / FFT_SIZE as f32;
        let bin_for = |hz: f32| ((hz / hz_per_bin).round() as usize).clamp(1, FFT_SIZE / 2);
        let bands = (0..4)
            .map(|i| BandState {
                bin_range: (
                    bin_for(config.bands.edges[i]),
                    bin_for(config.bands.edges[i + 1]),
                ),
                flux_history: VecDeque::with_capacity(FLUX_HISTORY),
                last_beat_ms: f64::NEG_INFINITY,
                energy_max: 1e-6,
                flux_max: 1e-6,
            })
            .collect();

        // Log-spaced UI spectrum ranges 40 Hz .. 16 kHz.
        let lo = 40.0f32;
        let hi = 16000.0f32.min(sr / 2.0 - hz_per_bin);
        let spectrum_ranges = (0..SPECTRUM_BINS)
            .map(|i| {
                let f0 = lo * (hi / lo).powf(i as f32 / SPECTRUM_BINS as f32);
                let f1 = lo * (hi / lo).powf((i + 1) as f32 / SPECTRUM_BINS as f32);
                let (a, b) = (bin_for(f0), bin_for(f1).max(bin_for(f0) + 1));
                (a, b)
            })
            .collect();

        Self {
            config,
            sample_rate: sr,
            fft,
            window,
            fft_buf: vec![Complex::default(); FFT_SIZE],
            scratch: vec![Complex::default(); scratch_len],
            input: vec![0.0; FFT_SIZE],
            pending: Vec::with_capacity(HOP_SIZE * 4),
            prev_mag: vec![0.0; FFT_SIZE / 2],
            bands,
            total_samples: 0,
            hop_count: 0,
            onset_env: VecDeque::with_capacity(ONSET_ENVELOPE_LEN),
            bpm_smoothed: 0.0,
            bpm_confidence: 0.0,
            bpm_outlier_count: 0,
            rms_env: 0.0,
            intensity: 0.0,
            rms_max: 1e-4,
            spectrum_max: 1e-6,
            spectrum_ranges,
        }
    }

    /// Apply new settings in place, preserving all runtime state
    /// (flux history, flux_max, refractory clocks, BPM smoothing) so that
    /// live tuning — e.g. throttled fader drags — never resets detection
    /// warmup or the UI normalization scale.
    pub fn set_config(&mut self, config: AnalyzerConfig) {
        let hz_per_bin = self.sample_rate / FFT_SIZE as f32;
        let bin_for = |hz: f32| ((hz / hz_per_bin).round() as usize).clamp(1, FFT_SIZE / 2);
        for (i, band) in self.bands.iter_mut().enumerate() {
            band.bin_range = (
                bin_for(config.bands.edges[i]),
                bin_for(config.bands.edges[i + 1]),
            );
        }
        self.config = config;
    }

    /// Feed mono samples; returns one `HopOutput` per completed hop.
    pub fn feed(&mut self, samples: &[f32]) -> Vec<HopOutput> {
        let mut out = Vec::new();
        self.pending.extend_from_slice(samples);
        while self.pending.len() >= HOP_SIZE {
            let hop: Vec<f32> = self.pending.drain(..HOP_SIZE).collect();
            out.push(self.process_hop(&hop));
        }
        out
    }

    fn process_hop(&mut self, hop: &[f32]) -> HopOutput {
        self.total_samples += hop.len() as u64;
        self.hop_count += 1;
        let now_ms = self.total_samples as f64 * 1000.0 / self.sample_rate as f64;

        // Slide input window.
        self.input.copy_within(HOP_SIZE.., 0);
        self.input[FFT_SIZE - HOP_SIZE..].copy_from_slice(hop);

        for (i, (&s, &w)) in self.input.iter().zip(self.window.iter()).enumerate() {
            self.fft_buf[i] = Complex::new(s * w, 0.0);
        }
        self.fft.process_with_scratch(&mut self.fft_buf, &mut self.scratch);

        let half = FFT_SIZE / 2;
        let mut centroid_num = 0.0f32;
        let mut centroid_den = 0.0f32;
        let hz_per_bin = self.sample_rate / FFT_SIZE as f32;
        let mut mags = vec![0.0f32; half];
        for i in 1..half {
            let m = self.fft_buf[i].norm();
            mags[i] = m;
            centroid_num += m * i as f32 * hz_per_bin;
            centroid_den += m;
        }
        let centroid = if centroid_den > 1e-9 {
            centroid_num / centroid_den
        } else {
            0.0
        };

        // Per-band flux + adaptive-threshold beat detection.
        let mut beats = Vec::new();
        let mut band_energy = [0.0f32; 4];
        let mut onset_env_val = 0.0f32;
        let mut band_flux = [0.0f32; 4];
        for (bi, band) in self.bands.iter().enumerate() {
            let (a, b) = band.bin_range;
            let mut flux = 0.0f32;
            for i in a..b.min(half) {
                let d = mags[i] - self.prev_mag[i];
                if d > 0.0 {
                    flux += d;
                }
            }
            // Normalize flux by band width so sensitivities are comparable.
            let width = (b - a).max(1) as f32;
            band_flux[bi] = flux / width.sqrt();
        }
        let total_flux: f32 = band_flux.iter().sum();
        let mut ui_flux = [0.0f32; 4];
        let mut ui_threshold = [0.0f32; 4];
        let mut ui_mean = [0.0f32; 4];
        let mut ui_std = [0.0f32; 4];
        let mut ui_flux_max = [0.0f32; 4];
        for (bi, band) in self.bands.iter_mut().enumerate() {
            let (a, b) = band.bin_range;
            let flux = band_flux[bi];
            let mut energy = 0.0f32;
            for i in a..b.min(half) {
                energy += mags[i] * mags[i];
            }

            band.energy_max = (band.energy_max * 0.9995).max(energy).max(1e-6);
            band_energy[bi] = (energy / band.energy_max).clamp(0.0, 1.0).powf(0.5);

            let n = band.flux_history.len().max(1) as f32;
            let mean: f32 = band.flux_history.iter().sum::<f32>() / n;
            let var: f32 =
                band.flux_history.iter().map(|x| (x - mean) * (x - mean)).sum::<f32>() / n;
            let std = var.sqrt().max(mean * 0.1 + 1e-6);

            if bi == 0 {
                onset_env_val += flux;
            } else if bi == 1 {
                onset_env_val += flux * 0.5;
            }

            let enabled = !self.config.low_only || bi == 0;
            let auto_threshold = mean + self.config.sensitivity[bi] * std;
            // Manual mode pins the threshold at a fixed raw flux level
            // (constant-gain premise); an unset (<= 0) band stays adaptive.
            let manual = self.config.threshold_mode == ThresholdMode::Manual
                && self.config.manual_threshold[bi] > 0.0;
            let threshold = if manual {
                self.config.manual_threshold[bi]
            } else {
                auto_threshold
            };
            // Normalize flux + threshold onto a shared 0..1 scale for the UI
            // fader; the slow-decay peak tracks both so the fader line always
            // stays within the visible meter.
            band.flux_max = (band.flux_max * 0.9995).max(flux).max(threshold).max(1e-6);
            ui_flux[bi] = (flux / band.flux_max).clamp(0.0, 1.0);
            ui_threshold[bi] = (threshold / band.flux_max).clamp(0.0, 1.0);
            ui_mean[bi] = (mean / band.flux_max).clamp(0.0, 1.0);
            ui_std[bi] = (std / band.flux_max).max(1e-4);
            ui_flux_max[bi] = band.flux_max;
            // Gate out spectral leakage: a band only fires when it carries a
            // meaningful share of this hop's total onset energy, so a kick
            // doesn't trigger the (otherwise silent) high band and vice versa.
            let dominant = flux >= total_flux * 0.15;
            if enabled
                && dominant
                && (manual || band.flux_history.len() >= FLUX_HISTORY / 4)
                && flux > threshold
                && now_ms - band.last_beat_ms >= self.config.min_interval_ms[bi] as f64
            {
                band.last_beat_ms = now_ms;
                let strength = (((flux - mean) / std) / 6.0).clamp(0.05, 1.5);
                beats.push(BandBeatEvent {
                    band: Band::ALL[bi],
                    strength,
                    timestamp_ms: now_ms as u64,
                });
            }

            if band.flux_history.len() >= FLUX_HISTORY {
                band.flux_history.pop_front();
            }
            band.flux_history.push_back(flux);
        }
        self.prev_mag.copy_from_slice(&mags);

        if self.onset_env.len() >= ONSET_ENVELOPE_LEN {
            self.onset_env.pop_front();
        }
        self.onset_env.push_back(onset_env_val);

        // RMS + intensity envelope with slow-decay AGC.
        let rms = (hop.iter().map(|s| s * s).sum::<f32>() / hop.len() as f32).sqrt();
        self.rms_max = (self.rms_max * 0.99997).max(rms).max(1e-4);
        let norm_rms = (rms / self.rms_max).clamp(0.0, 1.0);
        let coef = if norm_rms > self.rms_env { 0.3 } else { 0.005 };
        self.rms_env += (norm_rms - self.rms_env) * coef;
        self.intensity = self.rms_env;

        // UI spectrum.
        let mut spectrum = Vec::with_capacity(SPECTRUM_BINS);
        for &(a, b) in &self.spectrum_ranges {
            let v: f32 = mags[a..b.min(half)].iter().sum::<f32>() / (b - a) as f32;
            self.spectrum_max = (self.spectrum_max * 0.9999).max(v);
            spectrum.push(((v / self.spectrum_max).clamp(0.0, 1.0)).powf(0.4));
        }

        let tempo = if self.hop_count % TEMPO_RECALC_HOPS as u64 == 0 {
            self.estimate_tempo()
        } else {
            None
        };

        HopOutput {
            frame: AnalysisFrame {
                timestamp_ms: now_ms as u64,
                rms: norm_rms,
                band_energy,
                intensity: self.intensity,
                spectral_centroid: centroid,
                spectrum,
                band_flux: ui_flux,
                band_threshold: ui_threshold,
                band_flux_mean: ui_mean,
                band_flux_std: ui_std,
                band_flux_max: ui_flux_max,
            },
            beats,
            tempo,
        }
    }

    fn estimate_tempo(&mut self) -> Option<TempoEstimate> {
        let n = self.onset_env.len();
        if n < ONSET_ENVELOPE_LEN / 2 {
            return None;
        }
        let env: Vec<f32> = self.onset_env.iter().copied().collect();
        let mean = env.iter().sum::<f32>() / n as f32;
        let centered: Vec<f32> = env.iter().map(|x| x - mean).collect();
        let hop_rate = self.sample_rate / HOP_SIZE as f32;

        let lag_min = (hop_rate * 60.0 / BPM_MAX).floor() as usize;
        let lag_max = (hop_rate * 60.0 / BPM_MIN).ceil() as usize;
        if lag_max + 8 >= n {
            return None;
        }

        let acf0: f32 = centered.iter().map(|x| x * x).sum::<f32>().max(1e-9);
        let acf_at = |lag: usize| -> f32 {
            let mut s = 0.0f32;
            for i in lag..n {
                s += centered[i] * centered[i - lag];
            }
            s / acf0
        };

        // Score each lag together with its half/double harmonics so the
        // fundamental beat period wins over its subdivisions. A mild
        // log-domain prior centred on club tempos breaks octave ties
        // toward 120-140 instead of their halves.
        let prior = |lag: f32| -> f32 {
            let bpm = 60.0 * hop_rate / lag;
            let x = (bpm / 128.0).log2() / 0.9;
            (-0.5 * x * x).exp()
        };
        let mut best_lag = 0usize;
        let mut best_score = f32::NEG_INFINITY;
        for lag in lag_min..=lag_max {
            let mut score = acf_at(lag);
            let double = lag * 2;
            if double < n - 8 {
                score += 0.5 * acf_at(double);
            }
            if score > 0.0 {
                score *= prior(lag as f32);
            }
            if score > best_score {
                best_score = score;
                best_lag = lag;
            }
        }
        if best_lag == 0 || best_score <= 0.0 {
            return None;
        }

        // Parabolic interpolation around the peak for sub-hop precision.
        let y0 = acf_at(best_lag.saturating_sub(1));
        let y1 = acf_at(best_lag);
        let y2 = acf_at(best_lag + 1);
        let denom = y0 - 2.0 * y1 + y2;
        let delta = if denom.abs() > 1e-9 {
            (0.5 * (y0 - y2) / denom).clamp(-0.5, 0.5)
        } else {
            0.0
        };
        let lag = best_lag as f32 + delta;
        let mut bpm = 60.0 * hop_rate / lag;

        // Fold into a sane dance-music range. The ceiling must stay at or
        // above BPM_MAX: folding e.g. 195 down to 97.5 would push hardcore
        // tracks straight into the hip-hop tempo range.
        while bpm < 70.0 {
            bpm *= 2.0;
        }
        while bpm > BPM_MAX {
            bpm /= 2.0;
        }

        let confidence = (y1.max(0.0)).clamp(0.0, 1.0);

        if self.bpm_smoothed <= 0.0 {
            self.bpm_smoothed = bpm;
        } else if (bpm - self.bpm_smoothed).abs() < 4.0 {
            self.bpm_smoothed += (bpm - self.bpm_smoothed) * 0.25;
            self.bpm_outlier_count = 0;
        } else {
            // Require several consistent readings before jumping tempo.
            self.bpm_outlier_count += 1;
            if self.bpm_outlier_count >= 4 {
                self.bpm_smoothed = bpm;
                self.bpm_outlier_count = 0;
            }
        }
        self.bpm_confidence = confidence;

        Some(TempoEstimate {
            bpm: self.bpm_smoothed,
            confidence,
            source: TempoSource::Detector,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: u32 = 48000;

    /// Synthesize `secs` of audio with decaying tone bursts at `bpm`.
    fn beat_track(bpm: f32, tone_hz: f32, secs: f32) -> Vec<f32> {
        let n = (SR as f32 * secs) as usize;
        let period = 60.0 / bpm;
        let mut out = vec![0.0f32; n];
        let burst_len = (SR as f32 * 0.12) as usize;
        let mut t = 0.0f32;
        while t < secs {
            let start = (t * SR as f32) as usize;
            for i in 0..burst_len {
                let idx = start + i;
                if idx >= n {
                    break;
                }
                let x = i as f32 / SR as f32;
                let attack = (i as f32 / (SR as f32 * 0.005)).min(1.0);
                let env = attack * (-x * 30.0).exp();
                out[idx] += env * (2.0 * std::f32::consts::PI * tone_hz * x).sin() * 0.8;
            }
            t += period;
        }
        out
    }

    fn run(samples: &[f32]) -> (Vec<BandBeatEvent>, Option<TempoEstimate>) {
        let mut a = Analyzer::new(SR, AnalyzerConfig::default());
        let mut beats = Vec::new();
        let mut tempo = None;
        for chunk in samples.chunks(1024) {
            for hop in a.feed(chunk) {
                beats.extend(hop.beats);
                if let Some(t) = hop.tempo {
                    tempo = Some(t);
                }
            }
        }
        (beats, tempo)
    }

    #[test]
    fn detects_bpm_of_kick_pattern() {
        let audio = beat_track(128.0, 55.0, 15.0);
        let (_, tempo) = run(&audio);
        let tempo = tempo.expect("tempo should be estimated");
        assert!(
            (tempo.bpm - 128.0).abs() < 3.0,
            "expected ~128 bpm, got {}",
            tempo.bpm
        );
    }

    #[test]
    fn kick_fires_low_band_not_high() {
        let audio = beat_track(120.0, 55.0, 8.0);
        let (beats, _) = run(&audio);
        let low = beats.iter().filter(|b| b.band == Band::Low).count();
        let high = beats.iter().filter(|b| b.band == Band::High).count();
        // 8 s at 120 bpm = 16 kicks; allow detector warm-up misses.
        assert!(low >= 10, "low band should fire on kicks, got {low}");
        assert!(high <= 2, "high band should stay quiet, got {high}");
    }

    #[test]
    fn hats_fire_high_band_not_low() {
        let audio = beat_track(120.0, 8000.0, 8.0);
        let (beats, _) = run(&audio);
        let low = beats.iter().filter(|b| b.band == Band::Low).count();
        let high = beats.iter().filter(|b| b.band == Band::High).count();
        assert!(high >= 10, "high band should fire on hats, got {high}");
        assert!(low <= 2, "low band should stay quiet, got {low}");
    }

    fn run_with(cfg: AnalyzerConfig, samples: &[f32]) -> (Vec<BandBeatEvent>, Vec<AnalysisFrame>) {
        let mut a = Analyzer::new(SR, cfg);
        let mut beats = Vec::new();
        let mut frames = Vec::new();
        for chunk in samples.chunks(1024) {
            for hop in a.feed(chunk) {
                beats.extend(hop.beats);
                frames.push(hop.frame);
            }
        }
        (beats, frames)
    }

    #[test]
    fn manual_mode_fires_at_fixed_threshold() {
        let audio = beat_track(120.0, 55.0, 8.0);
        // Learn the raw low-band flux peak from an auto-mode pass.
        let (_, frames) = run_with(AnalyzerConfig::default(), &audio);
        let peak = frames
            .iter()
            .map(|f| f.band_flux[0] * f.band_flux_max[0])
            .fold(0.0f32, f32::max);
        assert!(peak > 0.0);

        let mut cfg = AnalyzerConfig::default();
        cfg.threshold_mode = ThresholdMode::Manual;
        cfg.manual_threshold = [peak * 0.5, 0.0, 0.0, 0.0];
        let (beats, _) = run_with(cfg.clone(), &audio);
        let low = beats.iter().filter(|b| b.band == Band::Low).count();
        assert!(low >= 10, "manual threshold at half peak should fire on kicks, got {low}");

        cfg.manual_threshold[0] = peak * 10.0;
        let (beats, _) = run_with(cfg, &audio);
        let low = beats.iter().filter(|b| b.band == Band::Low).count();
        assert_eq!(low, 0, "threshold far above peak must never fire, got {low}");
    }

    #[test]
    fn manual_mode_unset_falls_back_to_auto() {
        let audio = beat_track(120.0, 55.0, 8.0);
        let mut cfg = AnalyzerConfig::default();
        cfg.threshold_mode = ThresholdMode::Manual; // all thresholds unset (0)
        let (beats, _) = run_with(cfg, &audio);
        let low = beats.iter().filter(|b| b.band == Band::Low).count();
        assert!(low >= 10, "unset manual thresholds should behave like auto, got {low}");
    }

    #[test]
    fn set_config_preserves_runtime_state() {
        let audio = beat_track(120.0, 55.0, 2.0);
        let mut a = Analyzer::new(SR, AnalyzerConfig::default());
        for chunk in audio.chunks(1024) {
            a.feed(chunk);
        }
        let hist_len = a.bands[0].flux_history.len();
        let flux_max = a.bands[0].flux_max;
        assert!(hist_len > 0);
        let mut cfg = AnalyzerConfig::default();
        cfg.sensitivity[0] = 3.0;
        a.set_config(cfg);
        assert_eq!(a.bands[0].flux_history.len(), hist_len);
        assert_eq!(a.bands[0].flux_max, flux_max);
    }

    #[test]
    fn low_only_mode_suppresses_other_bands() {
        let mut cfg = AnalyzerConfig::default();
        cfg.low_only = true;
        let audio = beat_track(120.0, 8000.0, 6.0);
        let mut a = Analyzer::new(SR, cfg);
        let mut beats = Vec::new();
        for chunk in audio.chunks(1024) {
            for hop in a.feed(chunk) {
                beats.extend(hop.beats);
            }
        }
        assert!(beats.iter().all(|b| b.band == Band::Low));
    }
}
