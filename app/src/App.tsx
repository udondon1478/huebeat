import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import "./App.css";
import {
  AnalysisFrame,
  AppConfig,
  AudioDeviceInfo,
  BAND_LABELS,
  BANDS,
  BandBeatEvent,
  DiscoveredBridge,
  EntertainmentConfig,
  GENRE_LABELS,
  LightFrame,
  PairedBridge,
  Palette,
  PaletteEntry,
  TempoEstimate,
  rgbCss,
} from "./types";

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

function App() {
  const [config, setConfig] = useState<AppConfig | null>(null);
  const [running, setRunning] = useState(false);
  const [streaming, setStreaming] = useState(false);
  const [bpm, setBpm] = useState(0);
  const [bpmConfidence, setBpmConfidence] = useState(0);
  const [genre, setGenre] = useState("unknown");
  const [palette, setPalette] = useState<Palette | null>(null);
  const [palettes, setPalettes] = useState<PaletteEntry[]>([]);
  const [lights, setLights] = useState<LightFrame | null>(null);
  const [devices, setDevices] = useState<AudioDeviceInfo[]>([]);
  const [bridges, setBridges] = useState<DiscoveredBridge[]>([]);
  const [areas, setAreas] = useState<EntertainmentConfig[]>([]);
  const [pairing, setPairing] = useState(false);
  const [manualIp, setManualIp] = useState("");
  const [message, setMessage] = useState<string | null>(null);
  const [panic, setPanicState] = useState<string | null>(null);
  const [beatFlash, setBeatFlash] = useState<Record<string, number>>({});

  const frameRef = useRef<AnalysisFrame | null>(null);
  const pairingRef = useRef(false);

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
      listen<Palette>("engine:palette", (e) => setPalette(e.payload)),
      listen<LightFrame>("engine:lights", (e) => setLights(e.payload)),
      listen<BandBeatEvent>("engine:beat", (e) => {
        setBeatFlash((prev) => ({ ...prev, [e.payload.band]: Date.now() }));
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

  const now = Date.now();

  return (
    <main className="shell">
      <header className="topbar">
        <div className="brand">
          hue<span className="brand-accent">2</span>
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
          <h2>Analyzer</h2>
          <Spectrum frameRef={frameRef} />
          <div className="band-row">
            {BANDS.map((b) => {
              const active = now - (beatFlash[b] ?? 0) < 150;
              return (
                <div key={b} className={`band-indicator ${active ? "hit" : ""}`}>
                  {BAND_LABELS[b]}
                </div>
              );
            })}
          </div>
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
                低域のみでビート検出(エンジン再起動で反映)
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
