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

export interface AnalyzerSettings {
  sensitivity: [number, number, number, number];
  min_interval_ms: [number, number, number, number];
  low_only: boolean;
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
  techno: "Techno",
  trance: "Trance",
  drum_and_bass: "Drum & Bass",
  dubstep: "Dubstep",
  hardcore: "Hardcore",
  kawaii_future_bass: "Kawaii Future Bass",
  hip_hop: "Hip Hop",
  ambient: "Ambient",
  unknown: "Auto / Unknown",
};

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
