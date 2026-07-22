# huebeat

DJ パフォーマンス向けのオーディオリアクティブ Philips Hue 照明ソフト。
LightBeat の後継として、Hue **Entertainment API(DTLS/UDP, 50Hz)** ベースでゼロから設計。

## 機能

- **マルチバンド・ビート検出** — 低音 / 中低音 / 中高音 / 高音の4帯域で独立にオンセット検出し、帯域ごとにパレットの色を発火(Sound2Light 方式)
- **リアルタイム BPM 検出** — オンセット包絡の自己相関 + ハーモニクス評価
- **ジャンル自動推定** — BPM × スペクトル特徴のルールベース分類で 27 ジャンル(House / Techno / Trance / Psytrance / Hardstyle / Eurobeat / DnB / Dubstep / Trap / Future Bass / Hip Hop / Reggaeton / Synthwave など)を判別し、カラーパレットを自動切替。Future Core / Jersey Club / Hyperflip / アニソンRemix / Net Pop といった日本のサブカルクラブシーン系ジャンルにも対応。将来 ONNX モデルに差替可能な設計
- **カラーパレット編集** — ジャンルごとの4色(帯域対応)を UI から自由に変更・即時反映。ワンクリックで既定色に復元可能
- **Hue Entertainment ストリーミング** — DTLS-PSK / 50Hz、サブ100msレイテンシ。REST v2(CLIP v2)でペアリング・エリア管理
- **OSC 送信** — `/hue2/bpm` `/hue2/beat` `/hue2/genre` `/hue2/palette` `/hue2/intensity`
- **ライブ用パニック** — Blackout / White Flash / Freeze
- **LightBeat 機能パリティ** — 輝度レンジ、フェード速度、ライト毎反応確率、低域のみ検出モード、Glow/Strobe、パレット編集、設定自動保存
- 仮想ライトプレビュー(ブリッジなしでも動作確認可能)

## 構成

```
crates/
  core-types     共有型 (Band, BeatEvent, Palette, LightFrame, ...)
  audio-capture  cpal: WASAPI ループバック / ライン入力
  analysis       FFT・4帯域スペクトラルフラックス・BPM 推定
  genre          ルールベースジャンル分類(trait で ONNX 差替可)
  palette        ジャンル→パレット対応 (TOML 永続化)
  hue-client     mDNS 探索・ペアリング・CLIP v2 REST
  hue-stream     Entertainment API (DTLS-PSK + HueStream v2)
  effects        エフェクトエンジン(帯域→色マッピング、パニック等)
  osc-io         OSC 送信 (rosc)
app/             Tauri v2 + React UI
```

## 開発

要件: Rust (stable), Node.js 20+, gen2 Hue ブリッジ + エンタメエリア(実機連携時)

```sh
cd app
npm install
npm run tauri dev    # 開発起動
npm run tauri build  # 配布ビルド
```

テスト:

```sh
cargo test --workspace --exclude app
```

ブリッジ疎通診断(/auth/v2・エリア・DTLS ハンドシェイク・レインボー送信を順に検証):

```sh
cargo run -p diag -- <bridge_ip> <app_key> <client_key> <entertainment_config_id>
```

## 使い方

1. START の前に **Setup** でオーディオ入力(既定=再生デバイスのループバック)を選択
2. Hue ブリッジを「検索」→「ペアリング」(本体のリンクボタンを押す)
3. エンタメエリアを選択(Hue 公式アプリで事前作成)→「テストパターン」で疎通確認
4. START — 音楽を再生すると BPM / ジャンル / パレットが追従
5. OSC 送信先を設定すると Resolume / TouchDesigner 等と連携可能

## ロードマップ

- Ableton Link 同期(rusty_link)・ビートグリッド量子化エフェクト
- OSC / MIDI 受信、シーンバンク、タップテンポ
- ドロップ / ビルドアップ検出
- ONNX によるジャンル推定 v2、Art-Net 出力
