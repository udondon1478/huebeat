mod config;
mod engine;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use config::AppConfig;
use core_types::{Color, Palette};
use effects::Panic;
use hue_client::{DiscoveredBridge, EntertainmentConfig, PairedBridge};
use palette::PaletteStore;
use serde::Serialize;
use tauri::{Manager, State};

struct AppState {
    config_dir: std::path::PathBuf,
    config: Mutex<AppConfig>,
    palettes: Arc<Mutex<PaletteStore>>,
    engine: Mutex<Option<engine::RunningEngine>>,
}

impl AppState {
    fn save_config(&self) {
        let cfg = self.config.lock().unwrap().clone();
        if let Err(e) = config::save(&self.config_dir, &cfg) {
            tracing::error!("config save failed: {e}");
        }
    }

    fn save_palettes(&self) {
        let path = config::palettes_path(&self.config_dir);
        if let Err(e) = self.palettes.lock().unwrap().save(&path) {
            tracing::error!("palette save failed: {e}");
        }
    }
}

#[tauri::command]
fn list_audio_devices() -> Vec<audio_capture::AudioDeviceInfo> {
    audio_capture::list_devices()
}

#[tauri::command]
async fn discover_bridges() -> Result<Vec<DiscoveredBridge>, String> {
    tauri::async_runtime::spawn_blocking(|| {
        hue_client::discover_bridges(Duration::from_secs(3)).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[derive(Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
enum PairResult {
    Paired { bridge: PairedBridge },
    WaitingForButton,
}

#[tauri::command]
async fn pair_bridge(state: State<'_, AppState>, ip: String) -> Result<PairResult, String> {
    match hue_client::pair(&ip, "hue2#desktop").await {
        Ok(bridge) => {
            state.config.lock().unwrap().bridge = Some(bridge.clone());
            state.save_config();
            Ok(PairResult::Paired { bridge })
        }
        Err(hue_client::HueError::LinkButtonNotPressed) => Ok(PairResult::WaitingForButton),
        Err(e) => Err(e.to_string()),
    }
}

#[tauri::command]
async fn list_entertainment_configs(
    state: State<'_, AppState>,
) -> Result<Vec<EntertainmentConfig>, String> {
    let bridge = state
        .config
        .lock()
        .unwrap()
        .bridge
        .clone()
        .ok_or("bridge not paired")?;
    let client = hue_client::HueClient::new(&bridge).map_err(|e| e.to_string())?;
    client.entertainment_configs().await.map_err(|e| e.to_string())
}

#[tauri::command]
fn get_config(state: State<'_, AppState>) -> AppConfig {
    state.config.lock().unwrap().clone()
}

#[tauri::command]
fn set_config(state: State<'_, AppState>, config: AppConfig) {
    {
        let mut cfg = state.config.lock().unwrap();
        *cfg = config;
    }
    state.save_config();
    // Live-apply effect settings when running.
    let cfg = state.config.lock().unwrap().clone();
    if let Some(engine) = state.engine.lock().unwrap().as_ref() {
        engine.set_effect_settings(cfg.effects);
    }
}

#[derive(Serialize)]
struct PaletteEntry {
    genre: String,
    palette: Palette,
}

#[tauri::command]
fn get_palettes(state: State<'_, AppState>) -> Vec<PaletteEntry> {
    let store = state.palettes.lock().unwrap();
    let mut out: Vec<PaletteEntry> = store
        .genre_map
        .iter()
        .map(|(g, p)| PaletteEntry {
            genre: g.as_str().to_string(),
            palette: p.clone(),
        })
        .collect();
    out.sort_by(|a, b| a.genre.cmp(&b.genre));
    out
}

#[tauri::command]
fn set_genre_palette(
    state: State<'_, AppState>,
    genre_id: String,
    name: String,
    colors: Vec<String>,
) -> Result<(), String> {
    let genre = genre_id_to_genre(&genre_id).ok_or("unknown genre")?;
    let colors: Vec<Color> = colors
        .iter()
        .filter_map(|h| Color::from_hex(h))
        .collect();
    if colors.is_empty() {
        return Err("no valid colors".into());
    }
    let palette = Palette { name, colors };
    state
        .palettes
        .lock()
        .unwrap()
        .genre_map
        .insert(genre, palette.clone());
    state.save_palettes();
    // Live-apply if this genre is currently active.
    if let Some(engine) = state.engine.lock().unwrap().as_ref() {
        if *engine.current_genre.lock().unwrap() == genre {
            engine.set_palette(palette);
        }
    }
    Ok(())
}

fn genre_id_to_genre(id: &str) -> Option<core_types::Genre> {
    use core_types::Genre::*;
    [
        DeepHouse,
        House,
        Techno,
        Trance,
        DrumAndBass,
        Dubstep,
        Hardcore,
        KawaiiFutureBass,
        HipHop,
        Ambient,
        Unknown,
    ]
    .into_iter()
    .find(|g| g.as_str() == id)
}

#[tauri::command]
fn set_palette_override(state: State<'_, AppState>, genre_id: Option<String>) {
    {
        let mut cfg = state.config.lock().unwrap();
        cfg.palette_override = genre_id.clone();
    }
    state.save_config();
    if let (Some(id), Some(engine)) = (genre_id, state.engine.lock().unwrap().as_ref()) {
        if let Some(g) = genre_id_to_genre(&id) {
            let p = state.palettes.lock().unwrap().palette_for(g);
            engine.set_palette(p);
        }
    }
}

#[tauri::command]
fn set_panic(state: State<'_, AppState>, mode: Option<String>) -> Result<(), String> {
    let panic = match mode.as_deref() {
        None => None,
        Some("blackout") => Some(Panic::Blackout),
        Some("white_flash") => Some(Panic::WhiteFlash),
        Some("freeze") => Some(Panic::Freeze),
        Some(other) => return Err(format!("unknown panic mode {other}")),
    };
    if let Some(engine) = state.engine.lock().unwrap().as_ref() {
        engine.set_panic(panic);
    }
    Ok(())
}

#[derive(Serialize)]
struct StartResult {
    streaming: bool,
}

#[tauri::command]
async fn start_engine(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<StartResult, String> {
    if state.engine.lock().unwrap().is_some() {
        return Err("engine already running".into());
    }
    let cfg = state.config.lock().unwrap().clone();
    let running = engine::start(app, cfg, state.palettes.clone()).await?;
    // Give the stream connect a moment so the UI gets an accurate flag.
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let streaming = running.streaming.load(std::sync::atomic::Ordering::SeqCst);
    *state.engine.lock().unwrap() = Some(running);
    Ok(StartResult { streaming })
}

#[tauri::command]
fn stop_engine(state: State<'_, AppState>) {
    if let Some(engine) = state.engine.lock().unwrap().take() {
        engine.stop();
    }
}

#[tauri::command]
fn engine_status(state: State<'_, AppState>) -> engine::EngineStatus {
    let guard = state.engine.lock().unwrap();
    match guard.as_ref() {
        Some(e) => engine::EngineStatus {
            running: true,
            streaming: e.streaming.load(std::sync::atomic::Ordering::SeqCst),
            genre: e.current_genre.lock().unwrap().as_str().to_string(),
            palette: e.current_palette.lock().unwrap().clone(),
            bpm: *e.last_bpm.lock().unwrap(),
            message: None,
        },
        None => engine::EngineStatus {
            running: false,
            streaming: false,
            genre: "unknown".into(),
            palette: palette::default_palette(core_types::Genre::Unknown),
            bpm: 0.0,
            message: None,
        },
    }
}

#[tauri::command]
async fn run_test_pattern(state: State<'_, AppState>, seconds: u64) -> Result<(), String> {
    let (bridge, config_id) = {
        let cfg = state.config.lock().unwrap();
        (
            cfg.bridge.clone().ok_or("bridge not paired")?,
            cfg.entertainment_config_id
                .clone()
                .ok_or("entertainment area not selected")?,
        )
    };
    if state.engine.lock().unwrap().is_some() {
        return Err("stop the engine before running the test pattern".into());
    }
    engine::test_pattern(&bridge, &config_id, seconds.clamp(1, 30)).await
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tracing_subscriber::fmt::init();
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let config_dir = app
                .path()
                .app_config_dir()
                .expect("app config dir unavailable");
            let cfg = config::load(&config_dir);
            let palettes = PaletteStore::load(&config::palettes_path(&config_dir))
                .unwrap_or_default();
            app.manage(AppState {
                config_dir,
                config: Mutex::new(cfg),
                palettes: Arc::new(Mutex::new(palettes)),
                engine: Mutex::new(None),
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            list_audio_devices,
            discover_bridges,
            pair_bridge,
            list_entertainment_configs,
            get_config,
            set_config,
            get_palettes,
            set_genre_palette,
            set_palette_override,
            set_panic,
            start_engine,
            stop_engine,
            engine_status,
            run_test_pattern,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
