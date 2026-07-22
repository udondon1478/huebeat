import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import "./App.css";
import {
  AnalysisFrame,
  AnalyzerSettings,
  AppConfig,
  AudioDeviceInfo,
  BAND_LABELS,
  BANDS,
  BandBeatEvent,
  Color,
  DiscoveredBridge,
  EntertainmentConfig,
  GENRE_LABELS,
  GenreScore,
  LightFrame,
  PairedBridge,
  Palette,
  PaletteEntry,
  TempoEstimate,
  ThresholdMode,
  colorToHex,
  rgbCss,
} from "./types";

function hexToColor(hex: string): Color {
  const v = parseInt(hex.replace("#", ""), 16);
  return { r: (v >> 16) & 0xff, g: (v >> 8) & 0xff, b: v & 0xff };
}

function Spectrum({ frameRef }: { frameRef: React.MutableRefObject<AnalysisFrame | null> }) {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  useEffect(() => {
    let raf = 0;
    const draw = () => {
      raf = requestAnimationFrame(draw);
      const canvas = canvasRef.current;
      const frame = frameRef.current;
      if (!canvas) return;
      const ctx = canvas.getContext("2d");
      if (!ctx) return;
      const { width, height } = canvas;
      ctx.clearRect(0, 0, width, height);
      if (!frame) return;
      const n = frame.spectrum.length;
      const bw = width / n;
      for (let i = 0; i < n; i++) {
        const v = frame.spectrum[i];
        const h = v * (height - 4);
        const hue = 200 + (i / n) * 140;
        ctx.fillStyle = `hsl(${hue}, 90%, ${35 + v * 30}%)`;
        ctx.fillRect(i * bw + 1, height - h, bw - 2, h);
      }
    };
    draw();
    return () => cancelAnimationFrame(raf);
  }, [frameRef]);
  return <canvas ref={canvasRef} width={560} height={140} className="spectrum" />;
}

const SIGMA_MIN = 0.5;
const SIGMA_MAX = 6.0;
const BAND_COLORS = ["#ff5d73", "#ffb454", "#4dd08c", "#59b7ff"];

/**
 * Sound2Light-style per-band meters: live onset flux as a vertical bar with
 * a threshold fader line overlaid. When interactive, dragging the line
 * either maps its meter position back to a sensitivity in σ (auto mode) or
 * sets a fixed threshold in raw flux units (manual mode).
 */
function BandMeters({
  frameRef,
  beatFlashRef,
  sensitivity,
  mode,
  interactive,
  onSensitivity,
  onManualThreshold,
}: {
  frameRef: React.MutableRefObject<AnalysisFrame | null>;
  beatFlashRef: React.MutableRefObject<Record<string, number>>;
  sensitivity: [number, number, number, number];
  mode: ThresholdMode;
  interactive: boolean;
  onSensitivity: (index: number, sigma: number, commit: boolean) => void;
  onManualThreshold: (index: number, raw: number, commit: boolean) => void;
}) {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const dragRef = useRef<{ band: number; p: number } | null>(null);
  const sensRef = useRef(sensitivity);
  sensRef.current = sensitivity;
  const modeRef = useRef(mode);
  modeRef.current = mode;

  // A mode / interactivity switch mid-drag would leave a stale grab behind.
  useEffect(() => {
    dragRef.current = null;
  }, [mode, interactive]);

  useEffect(() => {
    let raf = 0;
    const draw = () => {
      raf = requestAnimationFrame(draw);
      const canvas = canvasRef.current;
      if (!canvas) return;
      const ctx = canvas.getContext("2d");
      if (!ctx) return;
      const { width, height } = canvas;
      const frame = frameRef.current;
      const now = Date.now();
      ctx.clearRect(0, 0, width, height);
      const colW = width / 4;
      const meterTop = 4;
      const meterBottom = height - 20;
      const meterH = meterBottom - meterTop;

      for (let i = 0; i < 4; i++) {
        const x0 = i * colW + 8;
        const w = colW - 16;
        const hit = now - (beatFlashRef.current[BANDS[i]] ?? 0) < 150;

        // Track.
        ctx.fillStyle = "#12121d";
        ctx.fillRect(x0, meterTop, w, meterH);

        if (frame) {
          // Flux bar.
          const flux = frame.band_flux[i];
          const barH = flux * meterH;
          ctx.fillStyle = hit ? "#ffffff" : BAND_COLORS[i];
          ctx.globalAlpha = hit ? 1 : 0.85;
          ctx.fillRect(x0, meterBottom - barH, w, barH);
          ctx.globalAlpha = 1;

          // Mean line (faint reference).
          const meanY = meterBottom - frame.band_flux_mean[i] * meterH;
          ctx.strokeStyle = "rgba(255,255,255,0.25)";
          ctx.setLineDash([3, 3]);
          ctx.beginPath();
          ctx.moveTo(x0, meanY);
          ctx.lineTo(x0 + w, meanY);
          ctx.stroke();
          ctx.setLineDash([]);

          // Threshold fader line: while dragging show the grabbed position,
          // otherwise the live effective threshold from the analyzer.
          const drag = dragRef.current;
          const p = drag?.band === i ? drag.p : frame.band_threshold[i];
          const y = meterBottom - p * meterH;
          ctx.strokeStyle = drag?.band === i ? "#ffe14d" : "#ffcf3d";
          ctx.lineWidth = drag?.band === i ? 3 : 2;
          ctx.beginPath();
          ctx.moveTo(x0 - 4, y);
          ctx.lineTo(x0 + w + 4, y);
          ctx.stroke();
          ctx.lineWidth = 1;
          // Handle triangles at both ends.
          ctx.fillStyle = ctx.strokeStyle;
          ctx.beginPath();
          ctx.moveTo(x0 - 4, y - 5);
          ctx.lineTo(x0 - 4, y + 5);
          ctx.lineTo(x0 + 3, y);
          ctx.closePath();
          ctx.fill();
          ctx.beginPath();
          ctx.moveTo(x0 + w + 4, y - 5);
          ctx.lineTo(x0 + w + 4, y + 5);
          ctx.lineTo(x0 + w - 3, y);
          ctx.closePath();
          ctx.fill();
        }

        // Label + readout: σ in auto mode, meter percent in manual mode.
        const drag = dragRef.current;
        const linePos =
          drag?.band === i ? drag.p : frame ? frame.band_threshold[i] : null;
        const readout =
          modeRef.current === "manual"
            ? linePos !== null
              ? `${Math.round(linePos * 100)}%`
              : "--"
            : `${sensRef.current[i].toFixed(1)}σ`;
        ctx.fillStyle = hit ? "#ffffff" : "#8a8aa3";
        ctx.font = "700 11px sans-serif";
        ctx.textAlign = "center";
        ctx.fillText(`${BAND_LABELS[BANDS[i]]}  ${readout}`, x0 + w / 2, height - 6);
      }
    };
    draw();
    return () => cancelAnimationFrame(raf);
  }, [frameRef, beatFlashRef]);

  const posFromEvent = (e: React.PointerEvent<HTMLCanvasElement>) => {
    const canvas = canvasRef.current!;
    const rect = canvas.getBoundingClientRect();
    const x = ((e.clientX - rect.left) / rect.width) * canvas.width;
    const y = ((e.clientY - rect.top) / rect.height) * canvas.height;
    const band = Math.min(3, Math.max(0, Math.floor(x / (canvas.width / 4))));
    const meterTop = 4;
    const meterBottom = canvas.height - 20;
    const p = (meterBottom - y) / (meterBottom - meterTop);
    return { band, p: Math.min(1, Math.max(0.02, p)) };
  };

  const sigmaFromPos = (band: number, p: number): number | null => {
    const frame = frameRef.current;
    if (!frame) return null;
    const std = Math.max(frame.band_flux_std[band], 1e-4);
    const sigma = (p - frame.band_flux_mean[band]) / std;
    return Math.min(SIGMA_MAX, Math.max(SIGMA_MIN, sigma));
  };

  const emitDrag = (band: number, p: number, commit: boolean) => {
    if (modeRef.current === "manual") {
      const frame = frameRef.current;
      if (!frame) return;
      onManualThreshold(band, p * frame.band_flux_max[band], commit);
    } else {
      const sigma = sigmaFromPos(band, p);
      if (sigma !== null) onSensitivity(band, sigma, commit);
    }
  };

  const onPointerDown = (e: React.PointerEvent<HTMLCanvasElement>) => {
    if (!interactive || !frameRef.current) return;
    const { band, p } = posFromEvent(e);
    dragRef.current = { band, p };
    e.currentTarget.setPointerCapture(e.pointerId);
    emitDrag(band, p, false);
  };

  const onPointerMove = (e: React.PointerEvent<HTMLCanvasElement>) => {
    const drag = dragRef.current;
    if (!drag) return;
    const { p } = posFromEvent(e);
    drag.p = p;
    emitDrag(drag.band, p, false);
  };

  const onPointerUp = (e: React.PointerEvent<HTMLCanvasElement>) => {
    const drag = dragRef.current;
    if (!drag) return;
    dragRef.current = null;
    emitDrag(drag.band, drag.p, true);
    e.currentTarget.releasePointerCapture(e.pointerId);
  };

  return (
    <canvas
      ref={canvasRef}
      width={560}
      height={170}
      className="band-meters"
      style={{ cursor: interactive ? "ns-resize" : "default" }}
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={onPointerUp}
      onPointerCancel={onPointerUp}
      title={
        interactive
          ? "黄色いラインをドラッグしてビート検出閾値を調整"
          : "ビート検出モニター(調整は詳細設定モードを有効化)"
      }
    />
  );
}

function App() {
  const [config, setConfig] = useState<AppConfig | null>(null);
  const [running, setRunning] = useState(false);
  const [streaming, setStreaming] = useState(false);
  const [bpm, setBpm] = useState(0);
  const [bpmConfidence, setBpmConfidence] = useState(0);
  const [genre, setGenre] = useState("unknown");
  const [genreScores, setGenreScores] = useState<GenreScore[]>([]);
  const [palette, setPalette] = useState<Palette | null>(null);
  const [palettes, setPalettes] = useState<PaletteEntry[]>([]);
  const [editGenreId, setEditGenreId] = useState("unknown");
  const [resetAllArmed, setResetAllArmed] = useState(false);
  const [lights, setLights] = useState<LightFrame | null>(null);
  const [devices, setDevices] = useState<AudioDeviceInfo[]>([]);
  const [bridges, setBridges] = useState<DiscoveredBridge[]>([]);
  const [areas, setAreas] = useState<EntertainmentConfig[]>([]);
  const [pairing, setPairing] = useState(false);
  const [manualIp, setManualIp] = useState("");
  const [message, setMessage] = useState<string | null>(null);
  const [panic, setPanicState] = useState<string | null>(null);
  const [advancedMode, setAdvancedMode] = useState(
    () =>
      (localStorage.getItem("huebeat:advancedMode") ??
        localStorage.getItem("hue2:advancedMode")) === "1"
  );

  const frameRef = useRef<AnalysisFrame | null>(null);
  const beatFlashRef = useRef<Record<string, number>>({});
  const pairingRef = useRef(false);
  const sensSendTimer = useRef<number | null>(null);
  const pendingConfigRef = useRef<AppConfig | null>(null);
  const configRef = useRef<AppConfig | null>(null);
  configRef.current = config;
  const paletteSendTimer = useRef<number | null>(null);
  const pendingPaletteRef = useRef<PaletteEntry | null>(null);
  const resetAllTimer = useRef<number | null>(null);

  const refreshAreas = useCallback(async () => {
    try {
      setAreas(await invoke<EntertainmentConfig[]>("list_entertainment_configs"));
    } catch {
      /* bridge unreachable — leave list empty */
    }
  }, []);

  const loadConfig = useCallback(async () => {
    const cfg = await invoke<AppConfig>("get_config");
    setConfig(cfg);
    if (cfg.bridge) refreshAreas();
  }, [refreshAreas]);

  useEffect(() => {
    loadConfig();
    invoke<AudioDeviceInfo[]>("list_audio_devices").then(setDevices);
    invoke<PaletteEntry[]>("get_palettes").then(setPalettes);

    const unsubs: Promise<() => void>[] = [
      listen<AnalysisFrame>("engine:analysis", (e) => {
        frameRef.current = e.payload;
      }),
      listen<TempoEstimate>("engine:tempo", (e) => {
        setBpm(e.payload.bpm);
        setBpmConfidence(e.payload.confidence);
      }),
      listen<string>("engine:genre", (e) => setGenre(e.payload)),
      listen<GenreScore[]>("engine:genre-scores", (e) => setGenreScores(e.payload)),
      listen<Palette>("engine:palette", (e) => setPalette(e.payload)),
      listen<LightFrame>("engine:lights", (e) => setLights(e.payload)),
      listen<BandBeatEvent>("engine:beat", (e) => {
        beatFlashRef.current[e.payload.band] = Date.now();
      }),
      listen<string>("engine:status-message", (e) => setMessage(e.payload)),
    ];
    return () => {
      unsubs.forEach((p) => p.then((f) => f()));
    };
  }, [loadConfig]);

  const updateConfig = async (patch: Partial<AppConfig>) => {
    if (!config) return;
    const next = { ...config, ...patch };
    setConfig(next);
    await invoke("set_config", { config: next });
  };

  // Fader drags fire continuously; update local state immediately but send
  // to the backend at most every ~120 ms, plus a final send on release.
  const applyAnalyzerLive = useCallback(
    (patch: (a: AnalyzerSettings) => AnalyzerSettings, commit: boolean) => {
      const base = pendingConfigRef.current ?? configRef.current;
      if (!base) return;
      const next = { ...base, analyzer: patch(base.analyzer) };
      pendingConfigRef.current = next;
      setConfig(next);
      const flush = () => {
        if (pendingConfigRef.current) {
          invoke("set_config", { config: pendingConfigRef.current });
        }
      };
      if (commit) {
        if (sensSendTimer.current !== null) {
          clearTimeout(sensSendTimer.current);
          sensSendTimer.current = null;
        }
        flush();
        pendingConfigRef.current = null;
      } else if (sensSendTimer.current === null) {
        sensSendTimer.current = window.setTimeout(() => {
          sensSendTimer.current = null;
          flush();
        }, 120);
      }
    },
    []
  );

  const applySensitivity = useCallback(
    (index: number, sigma: number, commit: boolean) => {
      applyAnalyzerLive((a) => {
        const sensitivity = [...a.sensitivity] as [number, number, number, number];
        sensitivity[index] = Math.round(sigma * 20) / 20;
        return { ...a, sensitivity };
      }, commit);
    },
    [applyAnalyzerLive]
  );

  const applyManualThreshold = useCallback(
    (index: number, raw: number, commit: boolean) => {
      applyAnalyzerLive((a) => {
        const manual_threshold = [...a.manual_threshold] as [number, number, number, number];
        // 4 significant digits keeps config.toml tidy.
        manual_threshold[index] = Number(raw.toPrecision(4));
        return { ...a, manual_threshold };
      }, commit);
    },
    [applyAnalyzerLive]
  );

  const toggleAdvanced = (on: boolean) => {
    setAdvancedMode(on);
    localStorage.setItem("huebeat:advancedMode", on ? "1" : "0");
  };

  // Switching to manual freezes the threshold where the adaptive line sits
  // right now (seamless); with no live frame the stored values remain and
  // any unset (0) band keeps adaptive behavior on the backend.
  const setThresholdMode = (mode: ThresholdMode) => {
    if (!config) return;
    const analyzer = { ...config.analyzer, threshold_mode: mode };
    const frame = frameRef.current;
    if (mode === "manual" && frame) {
      analyzer.manual_threshold = frame.band_threshold.map((t, i) =>
        Number((t * frame.band_flux_max[i]).toPrecision(4))
      ) as [number, number, number, number];
    }
    updateConfig({ analyzer });
  };

  const start = async () => {
    setMessage(null);
    try {
      const res = await invoke<{ streaming: boolean }>("start_engine");
      setRunning(true);
      setStreaming(res.streaming);
      if (!res.streaming) {
        setMessage(
          config?.bridge
            ? "エンジン起動(Hueストリーム未接続 — 仮想プレビューのみ)"
            : "エンジン起動(ブリッジ未設定 — 仮想プレビューのみ)"
        );
      }
    } catch (e) {
      setMessage(String(e));
    }
  };

  const stop = async () => {
    await invoke("stop_engine");
    setRunning(false);
    setStreaming(false);
    setPanicState(null);
    setGenreScores([]);
  };

  const discover = async () => {
    setMessage("ブリッジ検索中…");
    try {
      const found = await invoke<DiscoveredBridge[]>("discover_bridges");
      setBridges(found);
      setMessage(found.length ? null : "ブリッジが見つかりません(IP直接入力も可)");
    } catch (e) {
      setMessage(String(e));
    }
  };

  const pair = async (ip: string) => {
    setPairing(true);
    pairingRef.current = true;
    setMessage("ブリッジ本体のリンクボタンを押してください…(30秒待機)");
    for (let i = 0; i < 15 && pairingRef.current; i++) {
      try {
        const res = await invoke<{ status: string; bridge?: PairedBridge }>("pair_bridge", { ip });
        if (res.status === "paired") {
          setMessage("ペアリング成功");
          setPairing(false);
          pairingRef.current = false;
          await loadConfig();
          return;
        }
      } catch (e) {
        setMessage(String(e));
        setPairing(false);
        pairingRef.current = false;
        return;
      }
      await new Promise((r) => setTimeout(r, 2000));
    }
    setPairing(false);
    pairingRef.current = false;
    setMessage("ペアリングがタイムアウトしました");
  };

  const testPattern = async () => {
    setMessage("テストパターン送信中(5秒)…");
    try {
      await invoke("run_test_pattern", { seconds: 5 });
      setMessage("テストパターン完了");
    } catch (e) {
      setMessage(String(e));
    }
  };

  const togglePanic = async (mode: string) => {
    const next = panic === mode ? null : mode;
    setPanicState(next);
    await invoke("set_panic", { mode: next });
  };

  const setOverride = async (genreId: string) => {
    const value = genreId === "" ? null : genreId;
    await invoke("set_palette_override", { genreId: value });
    updateConfig({ palette_override: value });
  };

  // ---- Palette editing ----------------------------------------------
  // The genre whose palette is lighting the room right now: an override
  // wins, otherwise the detected genre.
  const activePaletteId = config?.palette_override ?? genre;

  /** Keep the topbar swatches fresh when the edited genre is active. */
  const syncActivePalette = (entry: PaletteEntry) => {
    if (entry.genre === activePaletteId) setPalette(entry.palette);
  };

  const flushPalette = () => {
    if (paletteSendTimer.current !== null) {
      clearTimeout(paletteSendTimer.current);
      paletteSendTimer.current = null;
    }
    const pending = pendingPaletteRef.current;
    if (!pending) return;
    pendingPaletteRef.current = null;
    invoke("set_genre_palette", {
      genreId: pending.genre,
      name: pending.palette.name,
      colors: pending.palette.colors.map(colorToHex),
    });
  };

  // Color pickers fire continuously while dragging; update local state
  // immediately but send to the backend at most every ~200 ms.
  const applyPaletteColor = (entry: PaletteEntry, slot: number, hex: string) => {
    const colors = entry.palette.colors.map((c, i) => (i === slot ? hexToColor(hex) : c));
    const updated: PaletteEntry = {
      genre: entry.genre,
      palette: { ...entry.palette, colors },
    };
    setPalettes((ps) => ps.map((p) => (p.genre === entry.genre ? updated : p)));
    syncActivePalette(updated);
    if (pendingPaletteRef.current && pendingPaletteRef.current.genre !== entry.genre) {
      flushPalette();
    }
    pendingPaletteRef.current = updated;
    if (paletteSendTimer.current === null) {
      paletteSendTimer.current = window.setTimeout(() => {
        paletteSendTimer.current = null;
        flushPalette();
      }, 200);
    }
  };

  const resetGenrePalette = async (genreId: string) => {
    pendingPaletteRef.current = null;
    const p = await invoke<Palette>("reset_genre_palette", { genreId });
    const updated: PaletteEntry = { genre: genreId, palette: p };
    setPalettes((ps) => ps.map((e) => (e.genre === genreId ? updated : e)));
    syncActivePalette(updated);
  };

  // Two-step guard: the first click arms the button, a second click
  // within 4 s actually resets every genre palette.
  const resetAllPalettes = async () => {
    if (!resetAllArmed) {
      setResetAllArmed(true);
      resetAllTimer.current = window.setTimeout(() => setResetAllArmed(false), 4000);
      return;
    }
    if (resetAllTimer.current !== null) {
      clearTimeout(resetAllTimer.current);
      resetAllTimer.current = null;
    }
    setResetAllArmed(false);
    pendingPaletteRef.current = null;
    const entries = await invoke<PaletteEntry[]>("reset_all_palettes");
    setPalettes(entries);
    const active = entries.find((e) => e.genre === activePaletteId);
    if (active) setPalette(active.palette);
  };

  const editEntry = palettes.find((p) => p.genre === editGenreId) ?? null;

  return (
    <main className="shell">
      <header className="topbar">
        <div className={`brand ${running ? "live" : ""}`}>
          <svg className="brand-mark" viewBox="0 0 28 28" aria-hidden="true">
            <defs>
              <linearGradient id="brand-grad" x1="0" y1="0" x2="1" y2="1">
                <stop offset="0%" stopColor="#ff2d78" />
                <stop offset="100%" stopColor="#00d4ff" />
              </linearGradient>
            </defs>
            <rect width="28" height="28" rx="8" fill="url(#brand-grad)" />
            <g fill="#fff">
              <rect className="eq eq1" x="4.75" y="15" width="3.5" height="8" rx="1.75" />
              <rect className="eq eq2" x="9.75" y="8" width="3.5" height="15" rx="1.75" />
              <rect className="eq eq3" x="14.75" y="12" width="3.5" height="11" rx="1.75" />
              <rect className="eq eq4" x="19.75" y="17" width="3.5" height="6" rx="1.75" />
            </g>
          </svg>
          <span className="brand-word">
            hue<span className="brand-accent">beat</span>
          </span>
        </div>
        <div className="bpm-box">
          <span className="bpm-value">{bpm > 0 ? bpm.toFixed(1) : "--"}</span>
          <span className="bpm-label">BPM {bpmConfidence > 0 && `(${Math.round(bpmConfidence * 100)}%)`}</span>
        </div>
        <div className="genre-box">
          <span className="genre-chip">{GENRE_LABELS[genre] ?? genre}</span>
          <div className="palette-swatches">
            {(palette?.colors ?? []).map((c, i) => (
              <span key={i} className="swatch" style={{ background: rgbCss(c) }} />
            ))}
          </div>
        </div>
        <div className="topbar-right">
          <span className={`stream-dot ${streaming ? "on" : ""}`} title="Hue streaming" />
          <button className={`run-btn ${running ? "stop" : "start"}`} onClick={running ? stop : start}>
            {running ? "STOP" : "START"}
          </button>
        </div>
      </header>

      {message && <div className="message">{message}</div>}

      <div className="grid">
        <section className="panel viz">
          <div className="panel-head">
            <h2>Analyzer</h2>
            <label className="check inline">
              <input
                type="checkbox"
                checked={advancedMode}
                onChange={(e) => toggleAdvanced(e.target.checked)}
              />
              詳細設定モード
            </label>
          </div>
          <Spectrum frameRef={frameRef} />
          {config && (
            <BandMeters
              frameRef={frameRef}
              beatFlashRef={beatFlashRef}
              sensitivity={config.analyzer.sensitivity}
              mode={config.analyzer.threshold_mode}
              interactive={advancedMode}
              onSensitivity={applySensitivity}
              onManualThreshold={applyManualThreshold}
            />
          )}
          <p className="hint">
            {advancedMode
              ? "メーター = 各帯域のオンセット量 / 黄ライン = 検出閾値(ドラッグで調整・超えるとビート発火)"
              : "メーター = 各帯域のオンセット量 / 黄ライン = 検出閾値(自動調整・調整は詳細設定モード)"}
          </p>
          {genreScores.length > 0 && (
            <div className="genre-scores">
              <span className="genre-scores-label">ジャンル推定</span>
              {genreScores.map(([id, share]) => (
                <span key={id} className={`genre-score-chip ${id === genre ? "top" : ""}`}>
                  {GENRE_LABELS[id] ?? id} <b>{Math.round(share * 100)}%</b>
                </span>
              ))}
            </div>
          )}
          {advancedMode && config && (
            <div className="advanced-settings">
              <div className="mode-row">
                <span className="field-label">閾値モード</span>
                <label className="check inline">
                  <input
                    type="radio"
                    name="threshold-mode"
                    checked={config.analyzer.threshold_mode === "auto"}
                    onChange={() => setThresholdMode("auto")}
                  />
                  自動(適応)
                </label>
                <label className="check inline">
                  <input
                    type="radio"
                    name="threshold-mode"
                    checked={config.analyzer.threshold_mode === "manual"}
                    onChange={() => setThresholdMode("manual")}
                  />
                  手動(固定・入力ゲイン一定前提)
                </label>
              </div>
              <p className="hint">
                {config.analyzer.threshold_mode === "manual"
                  ? "閾値は固定(音量変化に追従しません)。フェーダーで各帯域のレベルを直接設定 / 間隔: 連続発火を抑える最小時間"
                  : "閾値: 低いほど敏感に発火 / 間隔: 同帯域の連続発火を抑える最小時間"}
              </p>
              {BANDS.map((b, i) => (
                <div key={b} className="band-tune-row">
                  <span className="band-tune-label">{BAND_LABELS[b]}</span>
                  {config.analyzer.threshold_mode === "manual" ? (
                    <span className="manual-note">閾値: フェーダーで調整</span>
                  ) : (
                    <label>
                      閾値 {config.analyzer.sensitivity[i].toFixed(1)}σ
                      <input
                        type="range"
                        min={0.5}
                        max={6.0}
                        step={0.1}
                        value={config.analyzer.sensitivity[i]}
                        onChange={(e) => {
                          const sensitivity = [...config.analyzer.sensitivity] as [number, number, number, number];
                          sensitivity[i] = Number(e.target.value);
                          updateConfig({ analyzer: { ...config.analyzer, sensitivity } });
                        }}
                      />
                    </label>
                  )}
                  <label>
                    間隔 {Math.round(config.analyzer.min_interval_ms[i])}ms
                    <input
                      type="range"
                      min={40}
                      max={400}
                      step={10}
                      value={config.analyzer.min_interval_ms[i]}
                      onChange={(e) => {
                        const min_interval_ms = [...config.analyzer.min_interval_ms] as [number, number, number, number];
                        min_interval_ms[i] = Number(e.target.value);
                        updateConfig({ analyzer: { ...config.analyzer, min_interval_ms } });
                      }}
                    />
                  </label>
                </div>
              ))}
            </div>
          )}
          <h2>Lights</h2>
          <div className="lights-row">
            {(lights?.channels ?? []).map(([id, c]) => (
              <div
                key={id}
                className="light-bulb"
                style={{ background: rgbCss(c), boxShadow: `0 0 24px 4px ${rgbCss(c)}` }}
              >
                <span>{id}</span>
              </div>
            ))}
            {!lights && <div className="lights-placeholder">エンジン起動でプレビュー表示</div>}
          </div>
          <div className="panic-row">
            <button className={`panic ${panic === "blackout" ? "active" : ""}`} onClick={() => togglePanic("blackout")}>
              BLACKOUT
            </button>
            <button className={`panic ${panic === "white_flash" ? "active" : ""}`} onClick={() => togglePanic("white_flash")}>
              WHITE
            </button>
            <button className={`panic ${panic === "freeze" ? "active" : ""}`} onClick={() => togglePanic("freeze")}>
              FREEZE
            </button>
          </div>
        </section>

        <section className="panel setup">
          <h2>Setup</h2>
          <label>
            オーディオ入力
            <select
              value={config?.audio_device ?? ""}
              onChange={(e) => updateConfig({ audio_device: e.target.value || null })}
            >
              <option value="">既定の再生デバイス(ループバック)</option>
              {devices.map((d) => (
                <option key={d.id} value={d.id}>
                  {d.kind === "loopback" ? "[LOOP] " : "[IN] "}
                  {d.name}
                </option>
              ))}
            </select>
          </label>

          <div className="bridge-block">
            <div className="row">
              <span className="field-label">Hueブリッジ</span>
              <button onClick={discover}>検索</button>
            </div>
            {config?.bridge ? (
              <div className="bridge-paired">{config.bridge.ip} とペアリング済み</div>
            ) : (
              <>
                {bridges.map((b) => (
                  <div key={b.ip} className="row">
                    <span>{b.ip}</span>
                    <button disabled={pairing} onClick={() => pair(b.ip)}>
                      ペアリング
                    </button>
                  </div>
                ))}
                <div className="row">
                  <input
                    placeholder="IPを直接入力 (例 192.168.1.10)"
                    value={manualIp}
                    onChange={(e) => setManualIp(e.target.value)}
                  />
                  <button disabled={pairing || !manualIp} onClick={() => pair(manualIp)}>
                    ペアリング
                  </button>
                </div>
              </>
            )}
            {config?.bridge && (
              <label>
                エンタメエリア
                <div className="row">
                  <select
                    value={config?.entertainment_config_id ?? ""}
                    onChange={(e) => updateConfig({ entertainment_config_id: e.target.value || null })}
                  >
                    <option value="">未選択</option>
                    {areas.map((a) => (
                      <option key={a.id} value={a.id}>
                        {a.name} ({a.channels.length}ch)
                      </option>
                    ))}
                  </select>
                  <button onClick={refreshAreas}>更新</button>
                </div>
              </label>
            )}
            {config?.bridge && config?.entertainment_config_id && (
              <button onClick={testPattern} disabled={running}>
                テストパターン(5秒)
              </button>
            )}
          </div>

          <label>
            OSC送信先 (host:port / 空欄で無効)
            <input
              placeholder="127.0.0.1:9000"
              defaultValue={config?.osc_target ?? ""}
              onBlur={(e) => updateConfig({ osc_target: e.target.value || null })}
            />
          </label>

          <label>
            パレット
            <select value={config?.palette_override ?? ""} onChange={(e) => setOverride(e.target.value)}>
              <option value="">自動(ジャンル追従)</option>
              {palettes.map((p) => (
                <option key={p.genre} value={p.genre}>
                  {GENRE_LABELS[p.genre] ?? p.genre} 固定
                </option>
              ))}
            </select>
          </label>

          <div className="palette-editor">
            <div className="row">
              <span className="field-label">パレット編集</span>
              <select value={editGenreId} onChange={(e) => setEditGenreId(e.target.value)}>
                {palettes.map((p) => (
                  <option key={p.genre} value={p.genre}>
                    {GENRE_LABELS[p.genre] ?? p.genre}
                    {p.genre === activePaletteId ? " ●" : ""}
                  </option>
                ))}
              </select>
            </div>
            {editEntry && (
              <>
                <div className="palette-colors">
                  {editEntry.palette.colors.map((c, i) => (
                    <label key={i} className="palette-color">
                      <input
                        type="color"
                        value={colorToHex(c)}
                        onChange={(e) => applyPaletteColor(editEntry, i, e.target.value)}
                      />
                      <span>{i < BANDS.length ? BAND_LABELS[BANDS[i]] : `#${i + 1}`}</span>
                    </label>
                  ))}
                </div>
                <p className="hint">
                  色は帯域に対応(LOW=キック等)。変更は即保存・適用されます
                </p>
                <div className="row">
                  <button onClick={() => resetGenrePalette(editEntry.genre)}>
                    既定色に戻す
                  </button>
                  <button
                    className={resetAllArmed ? "danger" : ""}
                    onClick={resetAllPalettes}
                  >
                    {resetAllArmed ? "もう一度クリックで実行" : "全ジャンルを既定に戻す"}
                  </button>
                </div>
              </>
            )}
          </div>
        </section>

        <section className="panel effects">
          <h2>Effects</h2>
          {config && (
            <>
              <label>
                モード
                <select
                  value={config.effects.mode}
                  onChange={(e) =>
                    updateConfig({ effects: { ...config.effects, mode: e.target.value as "auto" | "glow" | "strobe" } })
                  }
                >
                  <option value="auto">Auto</option>
                  <option value="glow">Glow</option>
                  <option value="strobe">Strobe</option>
                </select>
              </label>
              <label>
                帯域→ライト割当
                <select
                  value={config.effects.assignment}
                  onChange={(e) =>
                    updateConfig({
                      effects: { ...config.effects, assignment: e.target.value as "all" | "by_height" | "custom" },
                    })
                  }
                >
                  <option value="by_height">高さで自動(低音=下・高音=上)</option>
                  <option value="all">全ライトが全帯域に反応</option>
                  <option value="custom">カスタム(設定ファイル)</option>
                </select>
              </label>
              <label>
                チェイス(左→右の時間差) {Math.round(config.effects.chase_ms)}ms
                <input
                  type="range"
                  min={0}
                  max={300}
                  step={10}
                  value={config.effects.chase_ms}
                  onChange={(e) => updateConfig({ effects: { ...config.effects, chase_ms: Number(e.target.value) } })}
                />
              </label>
              <label>
                最小輝度 {Math.round(config.effects.brightness_min * 100)}%
                <input
                  type="range"
                  min={0}
                  max={0.6}
                  step={0.01}
                  value={config.effects.brightness_min}
                  onChange={(e) =>
                    updateConfig({ effects: { ...config.effects, brightness_min: Number(e.target.value) } })
                  }
                />
              </label>
              <label>
                最大輝度 {Math.round(config.effects.brightness_max * 100)}%
                <input
                  type="range"
                  min={0.2}
                  max={1}
                  step={0.01}
                  value={config.effects.brightness_max}
                  onChange={(e) =>
                    updateConfig({ effects: { ...config.effects, brightness_max: Number(e.target.value) } })
                  }
                />
              </label>
              <label>
                フェード {Math.round(config.effects.fade_ms)}ms
                <input
                  type="range"
                  min={80}
                  max={1200}
                  step={10}
                  value={config.effects.fade_ms}
                  onChange={(e) => updateConfig({ effects: { ...config.effects, fade_ms: Number(e.target.value) } })}
                />
              </label>
              <label>
                ライト反応確率 {Math.round(config.effects.per_light_probability * 100)}%
                <input
                  type="range"
                  min={0.1}
                  max={1}
                  step={0.05}
                  value={config.effects.per_light_probability}
                  onChange={(e) =>
                    updateConfig({ effects: { ...config.effects, per_light_probability: Number(e.target.value) } })
                  }
                />
              </label>
              <label className="check">
                <input
                  type="checkbox"
                  checked={config.analyzer.low_only}
                  onChange={(e) =>
                    updateConfig({ analyzer: { ...config.analyzer, low_only: e.target.checked } })
                  }
                />
                低域のみでビート検出
              </label>
              <label className="check">
                <input
                  type="checkbox"
                  checked={config.effects.strobe_on_peaks}
                  onChange={(e) =>
                    updateConfig({ effects: { ...config.effects, strobe_on_peaks: e.target.checked } })
                  }
                />
                ピーク時ストロボ
              </label>
            </>
          )}
        </section>
      </div>
    </main>
  );
}

export default App;
