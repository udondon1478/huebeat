//! Shared types flowing through the huebeat pipeline:
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

/// Declares the `Genre` enum together with `ALL`, `as_str`, `from_id` and
/// the serde ids from one list, so a new genre cannot be half-added: the
/// list is the only place to edit, and forgetting it in `ALL` (which used
/// to compile fine and silently drop the genre from the palette UI and
/// from `palettes.toml` loading) is no longer possible.
macro_rules! genres {
    ($($variant:ident => $id:literal),+ $(,)?) => {
        /// Music genre families used for palette selection.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
        pub enum Genre {
            $(
                #[serde(rename = $id)]
                $variant,
            )+
        }

        impl Genre {
            /// Every genre, in display / palette-list order.
            pub const ALL: &'static [Genre] = &[$(Genre::$variant),+];

            pub fn as_str(self) -> &'static str {
                match self {
                    $(Genre::$variant => $id,)+
                }
            }

            /// Inverse of `as_str` (also matches the serde ids).
            pub fn from_id(id: &str) -> Option<Genre> {
                match id {
                    $($id => Some(Genre::$variant),)+
                    _ => None,
                }
            }
        }
    };
}

genres! {
    DeepHouse => "deep_house",
    House => "house",
    TechHouse => "tech_house",
    ElectroHouse => "electro_house",
    NuDisco => "nu_disco",
    NetPop => "net_pop",
    UkGarage => "uk_garage",
    JerseyClub => "jersey_club",
    Techno => "techno",
    Trance => "trance",
    Psytrance => "psytrance",
    Hardstyle => "hardstyle",
    Eurobeat => "eurobeat",
    AnisonRemix => "anison_remix",
    Breakbeat => "breakbeat",
    DrumAndBass => "drum_and_bass",
    Dubstep => "dubstep",
    Trap => "trap",
    Hyperflip => "hyperflip",
    FutureBass => "future_bass",
    FutureCore => "future_core",
    Hardcore => "hardcore",
    KawaiiFutureBass => "kawaii_future_bass",
    HipHop => "hip_hop",
    Rnb => "rnb",
    Reggaeton => "reggaeton",
    Synthwave => "synthwave",
    Ambient => "ambient",
    Unknown => "unknown",
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

#[cfg(test)]
mod tests {
    use super::*;

    /// `ALL`, `as_str`, `from_id` and the serde id all come from one macro
    /// list; this pins the contract the palette store and the UI rely on.
    #[test]
    fn genre_ids_roundtrip() {
        for &g in Genre::ALL {
            assert_eq!(Genre::from_id(g.as_str()), Some(g), "{g:?}");
            let json = serde_json::to_string(&g).unwrap();
            assert_eq!(json, format!("\"{}\"", g.as_str()), "{g:?}");
            assert_eq!(serde_json::from_str::<Genre>(&json).unwrap(), g);
        }
        assert_eq!(Genre::from_id("not_a_genre"), None);
    }

    #[test]
    fn genre_ids_are_unique() {
        let mut ids: Vec<&str> = Genre::ALL.iter().map(|g| g.as_str()).collect();
        let count = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), count, "duplicate genre id");
    }
}
