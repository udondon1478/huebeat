export type Band = "low" | "low_mid" | "high_mid" | "high";

export interface Color {
  r: number;
  g: number;
  b: number;
}

export interface Palette {
  name: string;
  colors: Color[];
}

export interface AnalysisFrame {
  timestamp_ms: number;
  rms: number;
  band_energy: [number, number, number, number];
  intensity: number;
  spectral_centroid: number;
  spectrum: number[];
  /** Per-band onset flux, normalized 0..1. */
  band_flux: [number, number, number, number];
  /** Effective beat threshold on the same 0..1 scale as band_flux. */
  band_threshold: [number, number, number, number];
  /** Running flux mean / std (normalized) for fader-drag → σ mapping. */
  band_flux_mean: [number, number, number, number];
  band_flux_std: [number, number, number, number];
  /** Raw slow-decay flux peak per band; meter position × this = raw flux. */
  band_flux_max: [number, number, number, number];
}

export interface BandBeatEvent {
  band: Band;
  strength: number;
  timestamp_ms: number;
}

export interface TempoEstimate {
  bpm: number;
  confidence: number;
  source: string;
}

export interface LightFrame {
  channels: [number, Color][];
}

export interface PairedBridge {
  ip: string;
  app_key: string;
  client_key: string;
}

export interface DiscoveredBridge {
  ip: string;
  name: string;
  bridge_id: string | null;
}

export interface EntertainmentConfig {
  id: string;
  name: string;
  channels: { channel_id: number; x: number; y: number; z: number }[];
}

export interface AudioDeviceInfo {
  id: string;
  name: string;
  kind: "loopback" | "input";
  is_default: boolean;
}

export type BandAssignment = "all" | "by_height" | "custom";

export interface EffectSettings {
  brightness_min: number;
  brightness_max: number;
  fade_ms: number;
  color_fade_ms: number;
  mode: "auto" | "glow" | "strobe";
  per_light_probability: number;
  band_slots: [number, number, number, number];
  assignment: BandAssignment;
  channel_bands: [number, Band[]][];
  chase_ms: number;
  strobe_on_peaks: boolean;
}

export type ThresholdMode = "auto" | "manual";

export interface AnalyzerSettings {
  sensitivity: [number, number, number, number];
  min_interval_ms: [number, number, number, number];
  low_only: boolean;
  threshold_mode: ThresholdMode;
  /** Fixed per-band threshold in raw flux units (manual mode); <= 0 = unset. */
  manual_threshold: [number, number, number, number];
}

export interface AppConfig {
  bridge: PairedBridge | null;
  entertainment_config_id: string | null;
  audio_device: string | null;
  osc_target: string | null;
  effects: EffectSettings;
  analyzer: AnalyzerSettings;
  palette_override: string | null;
}

export interface PaletteEntry {
  genre: string;
  palette: Palette;
}

export interface EngineStatus {
  running: boolean;
  streaming: boolean;
  genre: string;
  palette: Palette;
  bpm: number;
  message: string | null;
}

export const GENRE_LABELS: Record<string, string> = {
  deep_house: "Deep House",
  house: "House",
  tech_house: "Tech House",
  electro_house: "EDM / Big Room",
  nu_disco: "Nu Disco",
  net_pop: "Net Pop / J-Pop",
  uk_garage: "UK Garage",
  jersey_club: "Jersey Club",
  techno: "Techno",
  trance: "Trance",
  psytrance: "Psytrance",
  hardstyle: "Hardstyle",
  eurobeat: "Eurobeat",
  anison_remix: "アニソンRemix",
  breakbeat: "Breakbeat",
  drum_and_bass: "Drum & Bass",
  dubstep: "Dubstep",
  trap: "Trap",
  hyperflip: "Hyperflip",
  future_bass: "Future Bass",
  future_core: "Future Core",
  hardcore: "Hardcore",
  kawaii_future_bass: "Kawaii Future Bass",
  hip_hop: "Hip Hop",
  rnb: "R&B",
  reggaeton: "Reggaeton",
  synthwave: "Synthwave",
  ambient: "Ambient",
  unknown: "Auto / Unknown",
};

/** Top genre candidates emitted by the engine: [genre_id, share 0..1]. */
export type GenreScore = [string, number];

export const BANDS: Band[] = ["low", "low_mid", "high_mid", "high"];
export const BAND_LABELS: Record<Band, string> = {
  low: "LOW",
  low_mid: "L-MID",
  high_mid: "H-MID",
  high: "HIGH",
};

export function rgbCss(c: Color): string {
  return `rgb(${c.r}, ${c.g}, ${c.b})`;
}

export function colorToHex(c: Color): string {
  const h = (v: number) => v.toString(16).padStart(2, "0");
  return `#${h(c.r)}${h(c.g)}${h(c.b)}`;
}
