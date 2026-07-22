//! Shared types flowing through the hue2 pipeline:
//! audio capture -> analysis -> (genre, effects) -> hue-stream / osc.

use serde::{Deserialize, Serialize};

/// Frequency band for multi-band beat detection (Sound2Light style).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Band {
    /// Kick / bass, default 20-150 Hz
    Low,
    /// Snare body / toms, default 150-800 Hz
    LowMid,
    /// Claps / vocals / synth stabs, default 800-4000 Hz
    HighMid,
    /// Hi-hats / cymbals / air, default 4000-16000 Hz
    High,
}

impl Band {
    pub const ALL: [Band; 4] = [Band::Low, Band::LowMid, Band::HighMid, Band::High];

    pub fn index(self) -> usize {
        match self {
            Band::Low => 0,
            Band::LowMid => 1,
            Band::HighMid => 2,
            Band::High => 3,
        }
    }
}

/// Band edge frequencies in Hz; `edges[i]..edges[i+1]` is band i.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BandConfig {
    pub edges: [f32; 5],
}

impl Default for BandConfig {
    fn default() -> Self {
        Self {
            edges: [20.0, 150.0, 800.0, 4000.0, 16000.0],
        }
    }
}

/// How the beat-detection threshold is derived.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThresholdMode {
    /// Adaptive: mean + sensitivity·std over recent flux (gain-robust).
    #[default]
    Auto,
    /// Fixed per-band threshold in raw flux units; assumes constant
    /// input gain. Bands with an unset (<= 0) value fall back to Auto.
    Manual,
}

/// A beat (onset) detected in one frequency band.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct BandBeatEvent {
    pub band: Band,
    /// Relative onset strength, roughly 0..1 (can exceed 1 on hard hits).
    pub strength: f32,
    /// Milliseconds since engine start.
    pub timestamp_ms: u64,
}

/// Per-hop analysis snapshot published to UI / effects at the hop rate.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalysisFrame {
    pub timestamp_ms: u64,
    /// Overall RMS level 0..1 (post auto-gain).
    pub rms: f32,
    /// Smoothed per-band energy 0..1.
    pub band_energy: [f32; 4],
    /// Slow-moving loudness envelope 0..1 ("dynamic intensity").
    pub intensity: f32,
    /// Spectral centroid in Hz.
    pub spectral_centroid: f32,
    /// Coarse spectrum (log-spaced bins, 0..1) for UI display.
    pub spectrum: Vec<f32>,
    /// Per-band onset flux, normalized 0..1 against a slow-decay peak.
    pub band_flux: [f32; 4],
    /// Effective beat threshold (mean + sensitivity·std) on the same
    /// normalized scale as `band_flux`, so the UI can draw it as a fader
    /// over the live meter.
    pub band_threshold: [f32; 4],
    /// Running flux mean / std on the normalized scale; lets the UI map a
    /// dragged fader position back to a sensitivity in σ.
    pub band_flux_mean: [f32; 4],
    pub band_flux_std: [f32; 4],
    /// Raw (unnormalized) slow-decay flux peak per band — the scale factor
    /// behind the normalized values above. Multiplying a 0..1 meter
    /// position by this yields a threshold in raw flux units.
    pub band_flux_max: [f32; 4],
}

/// Current tempo estimate.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct TempoEstimate {
    pub bpm: f32,
    /// 0..1 confidence of the autocorrelation peak.
    pub confidence: f32,
    /// Source of the estimate (detector, tap tempo, Ableton Link, OSC).
    pub source: TempoSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TempoSource {
    Detector,
    Tap,
    AbletonLink,
    Osc,
}

/// Music genre families used for palette selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Genre {
    DeepHouse,
    House,
    TechHouse,
    ElectroHouse,
    NuDisco,
    NetPop,
    UkGarage,
    JerseyClub,
    Techno,
    Trance,
    Psytrance,
    Hardstyle,
    Eurobeat,
    AnisonRemix,
    Breakbeat,
    DrumAndBass,
    Dubstep,
    Trap,
    Hyperflip,
    FutureBass,
    FutureCore,
    Hardcore,
    KawaiiFutureBass,
    HipHop,
    Rnb,
    Reggaeton,
    Synthwave,
    Ambient,
    Unknown,
}

impl Genre {
    /// Every genre, in display / palette-list order.
    pub const ALL: [Genre; 29] = [
        Genre::DeepHouse,
        Genre::House,
        Genre::TechHouse,
        Genre::ElectroHouse,
        Genre::NuDisco,
        Genre::NetPop,
        Genre::UkGarage,
        Genre::JerseyClub,
        Genre::Techno,
        Genre::Trance,
        Genre::Psytrance,
        Genre::Hardstyle,
        Genre::Eurobeat,
        Genre::AnisonRemix,
        Genre::Breakbeat,
        Genre::DrumAndBass,
        Genre::Dubstep,
        Genre::Trap,
        Genre::Hyperflip,
        Genre::FutureBass,
        Genre::FutureCore,
        Genre::Hardcore,
        Genre::KawaiiFutureBass,
        Genre::HipHop,
        Genre::Rnb,
        Genre::Reggaeton,
        Genre::Synthwave,
        Genre::Ambient,
        Genre::Unknown,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Genre::DeepHouse => "deep_house",
            Genre::House => "house",
            Genre::TechHouse => "tech_house",
            Genre::ElectroHouse => "electro_house",
            Genre::NuDisco => "nu_disco",
            Genre::NetPop => "net_pop",
            Genre::UkGarage => "uk_garage",
            Genre::JerseyClub => "jersey_club",
            Genre::Techno => "techno",
            Genre::Trance => "trance",
            Genre::Psytrance => "psytrance",
            Genre::Hardstyle => "hardstyle",
            Genre::Eurobeat => "eurobeat",
            Genre::AnisonRemix => "anison_remix",
            Genre::Breakbeat => "breakbeat",
            Genre::DrumAndBass => "drum_and_bass",
            Genre::Dubstep => "dubstep",
            Genre::Trap => "trap",
            Genre::Hyperflip => "hyperflip",
            Genre::FutureBass => "future_bass",
            Genre::FutureCore => "future_core",
            Genre::Hardcore => "hardcore",
            Genre::KawaiiFutureBass => "kawaii_future_bass",
            Genre::HipHop => "hip_hop",
            Genre::Rnb => "rnb",
            Genre::Reggaeton => "reggaeton",
            Genre::Synthwave => "synthwave",
            Genre::Ambient => "ambient",
            Genre::Unknown => "unknown",
        }
    }

    /// Inverse of `as_str` (also matches the serde snake_case ids).
    pub fn from_id(id: &str) -> Option<Genre> {
        Self::ALL.iter().copied().find(|g| g.as_str() == id)
    }
}

/// sRGB color, 0..255.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Color {
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    pub fn from_hex(hex: &str) -> Option<Self> {
        let hex = hex.trim_start_matches('#');
        if hex.len() != 6 {
            return None;
        }
        let v = u32::from_str_radix(hex, 16).ok()?;
        Some(Self::new((v >> 16) as u8, (v >> 8) as u8, v as u8))
    }

    pub fn to_hex(self) -> String {
        format!("#{:02x}{:02x}{:02x}", self.r, self.g, self.b)
    }

    /// Linear interpolation in RGB space, t in 0..1.
    pub fn lerp(self, other: Color, t: f32) -> Color {
        let t = t.clamp(0.0, 1.0);
        let l = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * t).round() as u8;
        Color::new(l(self.r, other.r), l(self.g, other.g), l(self.b, other.b))
    }

    pub fn scaled(self, factor: f32) -> Color {
        let f = factor.clamp(0.0, 1.0);
        Color::new(
            (self.r as f32 * f) as u8,
            (self.g as f32 * f) as u8,
            (self.b as f32 * f) as u8,
        )
    }
}

/// A named color palette; slot order matters (bands map to slots).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Palette {
    pub name: String,
    pub colors: Vec<Color>,
}

impl Palette {
    /// Color for a palette slot, wrapping if the palette has fewer slots.
    pub fn slot(&self, i: usize) -> Color {
        if self.colors.is_empty() {
            Color::new(255, 255, 255)
        } else {
            self.colors[i % self.colors.len()]
        }
    }
}

/// One 50 Hz frame of light output: RGB per entertainment channel.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LightFrame {
    /// (channel_id, color) pairs; channel ids come from the Hue
    /// entertainment configuration.
    pub channels: Vec<(u8, Color)>,
}

/// Events published on the engine bus.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EngineEvent {
    Beat(BandBeatEvent),
    Analysis(AnalysisFrame),
    Tempo(TempoEstimate),
    GenreChanged { genre: Genre },
    PaletteChanged { palette: Palette },
}
