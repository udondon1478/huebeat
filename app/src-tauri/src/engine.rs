//! Engine orchestration: audio capture thread -> analysis thread ->
//! conductor task (genre / palette / OSC / UI events) + 50 Hz light loop
//! (effects -> Hue entertainment stream + virtual light preview).

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use analysis::Analyzer;
use core_types::{
    AnalysisFrame, Band, BandBeatEvent, Genre, LightFrame, Palette, TempoEstimate,
};
use effects::{EffectEngine, EffectSettings, Panic};
use genre::{Features, GenreClassifier, RuleBasedClassifier};
use hue_client::{HueClient, PairedBridge};
use hue_stream::HueStreamer;
use palette::PaletteStore;
use serde::Serialize;
use tauri::Emitter;

use crate::config::AppConfig;

const LIGHT_TICK_MS: u64 = 20; // 50 Hz
const FEATURE_WINDOW_S: f32 = 8.0;

#[derive(Debug, Clone, Serialize)]
pub struct EngineStatus {
    pub running: bool,
    pub streaming: bool,
    pub genre: String,
    pub palette: Palette,
    pub bpm: f32,
    pub message: Option<String>,
}

enum AnalysisMsg {
    Frame(AnalysisFrame),
    Beat(BandBeatEvent),
    Tempo(TempoEstimate),
}

/// Rolling window used to build `genre::Features`.
struct FeatureAggregator {
    frames: VecDeque<AnalysisFrame>,
    beats: VecDeque<BandBeatEvent>,
    last_tempo: Option<TempoEstimate>,
}

impl FeatureAggregator {
    fn new() -> Self {
        Self {
            frames: VecDeque::new(),
            beats: VecDeque::new(),
            last_tempo: None,
        }
    }

    fn push_frame(&mut self, f: AnalysisFrame) {
        let cutoff = f.timestamp_ms.saturating_sub((FEATURE_WINDOW_S * 1000.0) as u64);
        self.frames.push_back(f);
        while self.frames.front().map(|x| x.timestamp_ms < cutoff).unwrap_or(false) {
            self.frames.pop_front();
        }
        while self.beats.front().map(|x| x.timestamp_ms < cutoff).unwrap_or(false) {
            self.beats.pop_front();
        }
    }

    fn features(&self) -> Option<Features> {
        if self.frames.len() < 50 {
            return None;
        }
        let n = self.frames.len() as f32;
        let mut band_energy = [0.0f32; 4];
        let mut centroid = 0.0f32;
        let mut intensity = 0.0f32;
        for f in &self.frames {
            for i in 0..4 {
                band_energy[i] += f.band_energy[i];
            }
            centroid += f.spectral_centroid;
            intensity += f.intensity;
        }
        for e in &mut band_energy {
            *e /= n;
        }
        let span_ms = self
            .frames
            .back()
            .map(|b| b.timestamp_ms - self.frames.front().unwrap().timestamp_ms)
            .unwrap_or(1)
            .max(1) as f32;
        let per_sec = |count: usize| count as f32 * 1000.0 / span_ms;
        let kicks = self.beats.iter().filter(|b| b.band == Band::Low).count();
        let hats = self.beats.iter().filter(|b| b.band == Band::High).count();
        Some(Features {
            bpm: self.last_tempo.map(|t| t.bpm).unwrap_or(0.0),
            bpm_confidence: self.last_tempo.map(|t| t.confidence).unwrap_or(0.0),
            band_energy,
            kick_density: per_sec(kicks),
            hat_density: per_sec(hats),
            centroid: centroid / n,
            intensity: intensity / n,
        })
    }
}

pub struct RunningEngine {
    stop: Arc<AtomicBool>,
    pub effect: Arc<Mutex<EffectEngine>>,
    pub streaming: Arc<AtomicBool>,
    pub current_genre: Arc<Mutex<Genre>>,
    pub current_palette: Arc<Mutex<Palette>>,
    pub last_bpm: Arc<Mutex<f32>>,
}

impl RunningEngine {
    pub fn stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
    }

    pub fn set_panic(&self, panic: Option<Panic>) {
        self.effect.lock().unwrap().set_panic(panic);
    }

    pub fn set_effect_settings(&self, settings: EffectSettings) {
        self.effect.lock().unwrap().set_settings(settings);
    }

    pub fn set_palette(&self, p: Palette) {
        *self.current_palette.lock().unwrap() = p.clone();
        self.effect.lock().unwrap().set_palette(p);
    }
}

fn genre_from_id(id: &str) -> Option<Genre> {
    [
        Genre::DeepHouse,
        Genre::House,
        Genre::Techno,
        Genre::Trance,
        Genre::DrumAndBass,
        Genre::Dubstep,
        Genre::Hardcore,
        Genre::KawaiiFutureBass,
        Genre::HipHop,
        Genre::Ambient,
        Genre::Unknown,
    ]
    .into_iter()
    .find(|g| g.as_str() == id)
}

/// Start the full pipeline. Streaming to the bridge is attempted when the
/// config has a bridge + entertainment area; otherwise the engine still
/// runs (analysis, OSC, virtual preview).
pub async fn start(
    app: tauri::AppHandle,
    config: AppConfig,
    palettes: Arc<Mutex<PaletteStore>>,
) -> Result<RunningEngine, String> {
    let stop = Arc::new(AtomicBool::new(false));
    let streaming = Arc::new(AtomicBool::new(false));

    // ---- Hue entertainment channels (fall back to 4 virtual channels).
    let mut channel_ids: Vec<u8> = vec![0, 1, 2, 3];
    let mut hue: Option<(HueClient, String, PairedBridge)> = None;
    if let (Some(bridge), Some(cfg_id)) = (&config.bridge, &config.entertainment_config_id) {
        match HueClient::new(bridge) {
            Ok(client) => match client.entertainment_configs().await {
                Ok(configs) => {
                    if let Some(ec) = configs.iter().find(|c| &c.id == cfg_id) {
                        channel_ids = ec.channels.iter().map(|c| c.channel_id).collect();
                        if channel_ids.is_empty() {
                            channel_ids = vec![0];
                        }
                        hue = Some((client, cfg_id.clone(), bridge.clone()));
                    }
                }
                Err(e) => tracing::warn!("entertainment config fetch failed: {e}"),
            },
            Err(e) => tracing::warn!("hue client init failed: {e}"),
        }
    }

    // ---- Initial palette.
    let override_genre = config
        .palette_override
        .as_deref()
        .and_then(genre_from_id);
    let initial_genre = override_genre.unwrap_or(Genre::Unknown);
    let initial_palette = palettes.lock().unwrap().palette_for(initial_genre);

    let effect = Arc::new(Mutex::new(EffectEngine::new(
        &channel_ids,
        initial_palette.clone(),
        config.effects.clone(),
    )));
    let current_genre = Arc::new(Mutex::new(initial_genre));
    let current_palette = Arc::new(Mutex::new(initial_palette));
    let last_bpm = Arc::new(Mutex::new(0.0f32));

    // ---- OSC.
    let osc = match &config.osc_target {
        Some(target) => match osc_io::OscSender::new(target) {
            Ok(s) => Some(s),
            Err(e) => {
                tracing::warn!("osc sender init failed: {e}");
                None
            }
        },
        None => None,
    };

    // ---- Audio capture + analysis on dedicated threads.
    let (raw_tx, raw_rx) = std::sync::mpsc::sync_channel::<Vec<f32>>(64);
    let (msg_tx, mut msg_rx) = tokio::sync::mpsc::unbounded_channel::<AnalysisMsg>();
    let (sr_tx, sr_rx) = std::sync::mpsc::channel::<Result<u32, String>>();

    // Capture thread owns the (!Send) cpal stream.
    {
        let stop = stop.clone();
        let device = config.audio_device.clone();
        std::thread::Builder::new()
            .name("audio-capture".into())
            .spawn(move || {
                let handle = audio_capture::start_capture(device.as_deref(), move |samples| {
                    // Drop frames instead of blocking the audio callback.
                    let _ = raw_tx.try_send(samples.to_vec());
                });
                match handle {
                    Ok(h) => {
                        let _ = sr_tx.send(Ok(h.sample_rate));
                        while !stop.load(Ordering::SeqCst) {
                            std::thread::sleep(std::time::Duration::from_millis(100));
                        }
                        drop(h);
                    }
                    Err(e) => {
                        let _ = sr_tx.send(Err(e.to_string()));
                    }
                }
            })
            .map_err(|e| e.to_string())?;
    }
    let sample_rate = sr_rx
        .recv_timeout(std::time::Duration::from_secs(10))
        .map_err(|_| "audio capture did not start in time".to_string())?
        .map_err(|e| format!("audio capture failed: {e}"))?;

    // Analysis thread: raw samples -> frames/beats/tempo messages.
    {
        let stop = stop.clone();
        let analyzer_cfg = config.analyzer.to_analyzer_config();
        let msg_tx = msg_tx.clone();
        std::thread::Builder::new()
            .name("analysis".into())
            .spawn(move || {
                let mut analyzer = Analyzer::new(sample_rate, analyzer_cfg);
                while !stop.load(Ordering::SeqCst) {
                    match raw_rx.recv_timeout(std::time::Duration::from_millis(200)) {
                        Ok(samples) => {
                            for hop in analyzer.feed(&samples) {
                                for beat in hop.beats {
                                    let _ = msg_tx.send(AnalysisMsg::Beat(beat));
                                }
                                if let Some(t) = hop.tempo {
                                    let _ = msg_tx.send(AnalysisMsg::Tempo(t));
                                }
                                let _ = msg_tx.send(AnalysisMsg::Frame(hop.frame));
                            }
                        }
                        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
                        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                    }
                }
            })
            .map_err(|e| e.to_string())?;
    }

    // ---- Conductor task: events -> genre/palette/OSC/UI.
    {
        let stop = stop.clone();
        let effect = effect.clone();
        let current_genre = current_genre.clone();
        let current_palette = current_palette.clone();
        let last_bpm = last_bpm.clone();
        let palettes = palettes.clone();
        let app = app.clone();
        let auto_genre = override_genre.is_none();
        tauri::async_runtime::spawn(async move {
            let mut aggregator = FeatureAggregator::new();
            let mut classifier = RuleBasedClassifier::default();
            let mut frame_counter: u64 = 0;
            let mut last_genre_update = std::time::Instant::now();
            while !stop.load(Ordering::SeqCst) {
                let msg = tokio::time::timeout(
                    std::time::Duration::from_millis(200),
                    msg_rx.recv(),
                )
                .await;
                let msg = match msg {
                    Ok(Some(m)) => m,
                    Ok(None) => break,
                    Err(_) => continue,
                };
                match msg {
                    AnalysisMsg::Beat(beat) => {
                        effect.lock().unwrap().on_beat(&beat);
                        if let Some(osc) = &osc {
                            let _ = osc.send_beat(beat.band, beat.strength);
                        }
                        let _ = app.emit("engine:beat", &beat);
                    }
                    AnalysisMsg::Tempo(t) => {
                        *last_bpm.lock().unwrap() = t.bpm;
                        aggregator.last_tempo = Some(t);
                        if let Some(osc) = &osc {
                            let _ = osc.send_bpm(t.bpm);
                        }
                        let _ = app.emit("engine:tempo", &t);
                    }
                    AnalysisMsg::Frame(frame) => {
                        effect.lock().unwrap().set_intensity(frame.intensity);
                        frame_counter += 1;
                        // ~31 Hz to the UI is plenty.
                        if frame_counter % 3 == 0 {
                            let _ = app.emit("engine:analysis", &frame);
                        }
                        if frame_counter % 24 == 0 {
                            if let Some(osc) = &osc {
                                let _ = osc.send_intensity(frame.intensity);
                            }
                        }
                        aggregator.push_frame(frame);

                        if auto_genre && last_genre_update.elapsed().as_millis() > 1000 {
                            last_genre_update = std::time::Instant::now();
                            if let Some(features) = aggregator.features() {
                                if let Some(new_genre) = classifier.update(&features) {
                                    let p =
                                        palettes.lock().unwrap().palette_for(new_genre);
                                    *current_genre.lock().unwrap() = new_genre;
                                    *current_palette.lock().unwrap() = p.clone();
                                    effect.lock().unwrap().set_palette(p.clone());
                                    if let Some(osc) = &osc {
                                        let _ = osc.send_genre(new_genre.as_str());
                                        let _ = osc.send_palette(&p);
                                    }
                                    let _ = app
                                        .emit("engine:genre", new_genre.as_str());
                                    let _ = app.emit("engine:palette", &p);
                                }
                            }
                        }
                    }
                }
            }
        });
    }

    // ---- Light loop: 50 Hz effects tick -> Hue stream + UI preview.
    {
        let stop = stop.clone();
        let effect = effect.clone();
        let streaming_flag = streaming.clone();
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            let mut streamer: Option<HueStreamer> = None;
            let mut config_id_for_stop: Option<(HueClient, String)> = None;
            if let Some((client, cfg_id, bridge)) = hue {
                match connect_stream(&client, &cfg_id, &bridge).await {
                    Ok(s) => {
                        streamer = Some(s);
                        streaming_flag.store(true, Ordering::SeqCst);
                        config_id_for_stop = Some((client, cfg_id));
                    }
                    Err(e) => {
                        tracing::warn!("entertainment stream connect failed: {e}");
                        let _ = app.emit("engine:status-message", format!("Hue接続失敗: {e}"));
                    }
                }
            }

            let mut ticker =
                tokio::time::interval(std::time::Duration::from_millis(LIGHT_TICK_MS));
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            let mut tick_n: u64 = 0;
            while !stop.load(Ordering::SeqCst) {
                ticker.tick().await;
                tick_n += 1;
                let frame: LightFrame =
                    effect.lock().unwrap().tick(LIGHT_TICK_MS as f64);
                if let Some(s) = streamer.as_mut() {
                    if let Err(e) = s.send(&frame).await {
                        tracing::warn!("stream send failed: {e}");
                        streaming_flag.store(false, Ordering::SeqCst);
                        streamer = None;
                    }
                }
                // Virtual preview at 25 Hz.
                if tick_n % 2 == 0 {
                    let _ = app.emit("engine:lights", &frame);
                }
            }

            if let Some(s) = streamer.take() {
                s.close().await;
            }
            if let Some((client, cfg_id)) = config_id_for_stop {
                let _ = client.stop_streaming(&cfg_id).await;
            }
            streaming_flag.store(false, Ordering::SeqCst);
        });
    }

    Ok(RunningEngine {
        stop,
        effect,
        streaming,
        current_genre,
        current_palette,
        last_bpm,
    })
}

async fn connect_stream(
    client: &HueClient,
    config_id: &str,
    bridge: &PairedBridge,
) -> Result<HueStreamer, String> {
    // Preferred PSK identity is the hue-application-id, but some firmwares
    // don't return it; the application key works as identity on those.
    let app_id = match client.application_id().await {
        Ok(id) => id,
        Err(e) => {
            tracing::warn!("application id unavailable ({e}); using app key as PSK identity");
            bridge.app_key.clone()
        }
    };
    client
        .start_streaming(config_id)
        .await
        .map_err(|e| e.to_string())?;
    match HueStreamer::connect(&bridge.ip, &app_id, &bridge.client_key, config_id).await {
        Ok(s) => Ok(s),
        Err(e) => {
            let _ = client.stop_streaming(config_id).await;
            Err(e.to_string())
        }
    }
}

/// Standalone rainbow test pattern (Phase 1 verification, no audio needed).
pub async fn test_pattern(
    bridge: &PairedBridge,
    config_id: &str,
    seconds: u64,
) -> Result<(), String> {
    let client = HueClient::new(bridge).map_err(|e| e.to_string())?;
    let configs = client
        .entertainment_configs()
        .await
        .map_err(|e| e.to_string())?;
    let channels: Vec<u8> = configs
        .iter()
        .find(|c| c.id == config_id)
        .map(|c| c.channels.iter().map(|ch| ch.channel_id).collect())
        .unwrap_or_else(|| vec![0]);

    let mut streamer = connect_stream(&client, config_id, bridge).await?;
    let mut ticker = tokio::time::interval(std::time::Duration::from_millis(LIGHT_TICK_MS));
    let steps = seconds * 1000 / LIGHT_TICK_MS;
    for i in 0..steps {
        ticker.tick().await;
        let t = i as f32 * LIGHT_TICK_MS as f32 / 1000.0;
        let frame = LightFrame {
            channels: channels
                .iter()
                .enumerate()
                .map(|(idx, &id)| {
                    let hue_deg = (t * 90.0 + idx as f32 * 60.0) % 360.0;
                    (id, hsv(hue_deg, 1.0, 1.0))
                })
                .collect(),
        };
        streamer.send(&frame).await.map_err(|e| e.to_string())?;
    }
    streamer.close().await;
    client
        .stop_streaming(config_id)
        .await
        .map_err(|e| e.to_string())?;
    Ok(())
}

fn hsv(h: f32, s: f32, v: f32) -> core_types::Color {
    let c = v * s;
    let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
    let m = v - c;
    let (r, g, b) = match (h as u32) / 60 % 6 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    core_types::Color::new(
        ((r + m) * 255.0) as u8,
        ((g + m) * 255.0) as u8,
        ((b + m) * 255.0) as u8,
    )
}
