//! Persistent app configuration (TOML in the Tauri app-config dir).

use std::path::PathBuf;

use effects::EffectSettings;
use hue_client::PairedBridge;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalyzerSettings {
    /// Threshold in std-devs above mean flux per band; lower = more beats.
    pub sensitivity: [f32; 4],
    pub min_interval_ms: [f32; 4],
    /// LightBeat-style "bass only" beat detection.
    pub low_only: bool,
    // Per-field defaults: the struct has no container-level
    // #[serde(default)], so an existing config.toml written before these
    // keys existed must still parse (a failure would silently drop the
    // whole file, including bridge pairing).
    /// Adaptive (auto) vs fixed (manual) beat threshold.
    #[serde(default)]
    pub threshold_mode: core_types::ThresholdMode,
    /// Fixed per-band threshold in raw flux units (manual mode);
    /// <= 0 = unset, falls back to the adaptive threshold.
    #[serde(default)]
    pub manual_threshold: [f32; 4],
}

impl Default for AnalyzerSettings {
    fn default() -> Self {
        let d = analysis::AnalyzerConfig::default();
        Self {
            sensitivity: d.sensitivity,
            min_interval_ms: d.min_interval_ms,
            low_only: d.low_only,
            threshold_mode: d.threshold_mode,
            manual_threshold: d.manual_threshold,
        }
    }
}

impl AnalyzerSettings {
    pub fn to_analyzer_config(&self) -> analysis::AnalyzerConfig {
        analysis::AnalyzerConfig {
            bands: Default::default(),
            sensitivity: self.sensitivity,
            min_interval_ms: self.min_interval_ms,
            low_only: self.low_only,
            threshold_mode: self.threshold_mode,
            manual_threshold: self.manual_threshold,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AppConfig {
    pub bridge: Option<PairedBridge>,
    pub entertainment_config_id: Option<String>,
    /// See `audio_capture::list_devices` ids; None = default loopback.
    pub audio_device: Option<String>,
    /// "host:port" OSC destination; None disables OSC output.
    pub osc_target: Option<String>,
    #[serde(default)]
    pub effects: EffectSettings,
    #[serde(default)]
    pub analyzer: AnalyzerSettings,
    /// Manual palette override: a genre id whose palette is forced,
    /// disabling automatic genre switching. None = auto.
    pub palette_override: Option<String>,
}

pub fn config_path(app_config_dir: &std::path::Path) -> PathBuf {
    app_config_dir.join("config.toml")
}

pub fn palettes_path(app_config_dir: &std::path::Path) -> PathBuf {
    app_config_dir.join("palettes.toml")
}

pub fn load(app_config_dir: &std::path::Path) -> AppConfig {
    let path = config_path(app_config_dir);
    match std::fs::read_to_string(&path) {
        Ok(text) => toml::from_str(&text).unwrap_or_else(|e| {
            tracing::warn!("config parse error, using defaults: {e}");
            AppConfig::default()
        }),
        Err(_) => AppConfig::default(),
    }
}

pub fn save(app_config_dir: &std::path::Path, config: &AppConfig) -> std::io::Result<()> {
    std::fs::create_dir_all(app_config_dir)?;
    let text = toml::to_string_pretty(config)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    std::fs::write(config_path(app_config_dir), text)
}
