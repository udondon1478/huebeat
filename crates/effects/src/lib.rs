//! Effect engine: turns beat events + intensity into 50 Hz light frames.
//!
//! Position-aware multi-band mapping: entertainment-area channel positions
//! (x = left/right, z = floor/ceiling, each -1..1) drive which band a light
//! reacts to (kick at the floor, hats at the ceiling) and a left-to-right
//! chase offset within a band group.

use core_types::{Band, BandBeatEvent, Color, LightFrame, Palette};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EffectMode {
    /// Pick glow vs strobe from intensity.
    Auto,
    Glow,
    Strobe,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Panic {
    Blackout,
    WhiteFlash,
    Freeze,
}

/// How bands are routed to lights.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BandAssignment {
    /// Every light reacts to every band (LightBeat-style).
    All,
    /// Split lights by height: lowest lights take the low band, highest
    /// take the high band. Uses entertainment-area positions.
    ByHeight,
    /// Explicit per-channel mapping via `channel_bands`.
    Custom,
}

/// A light channel with its entertainment-area position.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct ChannelInfo {
    pub id: u8,
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

fn default_assignment() -> BandAssignment {
    BandAssignment::ByHeight
}

fn default_chase_ms() -> f32 {
    80.0
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EffectSettings {
    /// Idle floor brightness 0..1.
    pub brightness_min: f32,
    /// Beat peak brightness 0..1.
    pub brightness_max: f32,
    /// Time for a beat flash to decay to ~37 %.
    pub fade_ms: f32,
    /// How fast colors blend toward their target.
    pub color_fade_ms: f32,
    pub mode: EffectMode,
    /// Chance 0..1 that a given light reacts to a given beat.
    pub per_light_probability: f32,
    /// Palette slot used per band (low, low-mid, high-mid, high).
    pub band_slots: [usize; 4],
    /// Band routing strategy.
    #[serde(default = "default_assignment")]
    pub assignment: BandAssignment,
    /// Bands each channel reacts to (Custom assignment); empty = all bands.
    #[serde(default)]
    pub channel_bands: Vec<(u8, Vec<Band>)>,
    /// Left-to-right stagger of a beat flash across the reacting lights.
    /// 0 disables the chase.
    #[serde(default = "default_chase_ms")]
    pub chase_ms: f32,
    /// Extra white flicker on very strong hits (Auto/Strobe modes).
    pub strobe_on_peaks: bool,
}

impl Default for EffectSettings {
    fn default() -> Self {
        Self {
            brightness_min: 0.08,
            brightness_max: 1.0,
            fade_ms: 350.0,
            color_fade_ms: 120.0,
            mode: EffectMode::Auto,
            per_light_probability: 1.0,
            band_slots: [0, 1, 2, 3],
            assignment: default_assignment(),
            channel_bands: Vec::new(),
            chase_ms: default_chase_ms(),
            strobe_on_peaks: true,
        }
    }
}

struct ChannelState {
    info: ChannelInfo,
    /// Which bands this channel reacts to.
    bands: [bool; 4],
    color: Color,
    target: Color,
    brightness: f32,
    strobe_until_ms: f64,
}

/// A beat flash scheduled in the future (chase stagger).
struct PendingFlash {
    due_ms: f64,
    channel: usize,
    color: Color,
    peak: f32,
    strobe: bool,
}

pub struct EffectEngine {
    settings: EffectSettings,
    palette: Palette,
    channels: Vec<ChannelState>,
    pending: Vec<PendingFlash>,
    intensity: f32,
    panic: Option<Panic>,
    clock_ms: f64,
    strobe_phase: bool,
    frozen: Option<LightFrame>,
}

/// Height-based band routing: rank channels by z and spread them across the
/// four bands; bands that end up with no channel are folded onto the
/// nearest occupied group so every beat lights something.
fn assign_by_height(channels: &[ChannelInfo]) -> Vec<[bool; 4]> {
    let n = channels.len();
    let mut order: Vec<usize> = (0..n).collect();
    // Stable spread even when heights are identical: tie-break on x, id.
    order.sort_by(|&a, &b| {
        let ca = &channels[a];
        let cb = &channels[b];
        ca.z
            .total_cmp(&cb.z)
            .then(ca.x.total_cmp(&cb.x))
            .then(ca.id.cmp(&cb.id))
    });

    let mut buckets = vec![0usize; n];
    for (rank, &idx) in order.iter().enumerate() {
        let r = if n <= 1 { 0.5 } else { rank as f32 / (n - 1) as f32 };
        buckets[idx] = (r * 3.0).round() as usize;
    }

    let mut masks = vec![[false; 4]; n];
    for (idx, &b) in buckets.iter().enumerate() {
        masks[idx][b] = true;
    }
    // Fold empty bands onto the nearest occupied bucket.
    for band in 0..4usize {
        if buckets.iter().any(|&b| b == band) {
            continue;
        }
        let nearest = buckets
            .iter()
            .copied()
            .min_by_key(|&b| (b as i32 - band as i32).abs())
            .unwrap_or(0);
        for (idx, &b) in buckets.iter().enumerate() {
            if b == nearest {
                masks[idx][band] = true;
            }
        }
    }
    masks
}

fn compute_masks(channels: &[ChannelInfo], settings: &EffectSettings) -> Vec<[bool; 4]> {
    match settings.assignment {
        BandAssignment::All => vec![[true; 4]; channels.len()],
        BandAssignment::ByHeight => assign_by_height(channels),
        BandAssignment::Custom => channels
            .iter()
            .map(|ch| {
                match settings
                    .channel_bands
                    .iter()
                    .find(|(cid, _)| *cid == ch.id)
                {
                    Some((_, bands)) if !bands.is_empty() => {
                        let mut m = [false; 4];
                        for b in bands {
                            m[b.index()] = true;
                        }
                        m
                    }
                    _ => [true; 4],
                }
            })
            .collect(),
    }
}

impl EffectEngine {
    pub fn new(channels: &[ChannelInfo], palette: Palette, settings: EffectSettings) -> Self {
        let base = palette.slot(0);
        let masks = compute_masks(channels, &settings);
        Self {
            channels: channels
                .iter()
                .zip(masks)
                .map(|(info, bands)| ChannelState {
                    info: *info,
                    bands,
                    color: base,
                    target: base,
                    brightness: settings.brightness_min,
                    strobe_until_ms: 0.0,
                })
                .collect(),
            settings,
            palette,
            pending: Vec::new(),
            intensity: 0.0,
            panic: None,
            clock_ms: 0.0,
            strobe_phase: false,
            frozen: None,
        }
    }

    pub fn set_palette(&mut self, palette: Palette) {
        self.palette = palette;
    }

    pub fn set_settings(&mut self, settings: EffectSettings) {
        self.settings = settings;
        let infos: Vec<ChannelInfo> = self.channels.iter().map(|c| c.info).collect();
        let masks = compute_masks(&infos, &self.settings);
        for (ch, mask) in self.channels.iter_mut().zip(masks) {
            ch.bands = mask;
        }
    }

    pub fn set_panic(&mut self, panic: Option<Panic>) {
        if panic == Some(Panic::Freeze) {
            self.frozen = Some(self.render());
        } else {
            self.frozen = None;
        }
        self.panic = panic;
    }

    pub fn set_intensity(&mut self, intensity: f32) {
        self.intensity = intensity.clamp(0.0, 1.0);
    }

    pub fn on_beat(&mut self, beat: &BandBeatEvent) {
        let s = &self.settings;
        let color = self.palette.slot(s.band_slots[beat.band.index()]);
        let peak = s.brightness_min
            + (s.brightness_max - s.brightness_min) * beat.strength.clamp(0.2, 1.0);
        let strobe = s.strobe_on_peaks
            && beat.strength > 0.9
            && (s.mode == EffectMode::Strobe
                || (s.mode == EffectMode::Auto && self.intensity > 0.75));
        let prob = s.per_light_probability;
        let chase_ms = s.chase_ms.max(0.0) as f64;

        let mut reacting: Vec<usize> = (0..self.channels.len())
            .filter(|&i| self.channels[i].bands[beat.band.index()])
            .filter(|_| prob >= 1.0 || rand::random::<f32>() <= prob)
            .collect();
        // Left-to-right chase order.
        reacting.sort_by(|&a, &b| self.channels[a].info.x.total_cmp(&self.channels[b].info.x));

        let count = reacting.len();
        for (i, idx) in reacting.into_iter().enumerate() {
            let delay = if chase_ms > 0.0 && count > 1 {
                chase_ms * i as f64 / (count - 1) as f64
            } else {
                0.0
            };
            if delay <= 0.0 {
                self.apply_flash(idx, color, peak, strobe);
            } else {
                self.pending.push(PendingFlash {
                    due_ms: self.clock_ms + delay,
                    channel: idx,
                    color,
                    peak,
                    strobe,
                });
            }
        }
    }

    fn apply_flash(&mut self, idx: usize, color: Color, peak: f32, strobe: bool) {
        let strobe_until = self.clock_ms + 120.0;
        let ch = &mut self.channels[idx];
        ch.target = color;
        ch.brightness = ch.brightness.max(peak);
        if strobe {
            ch.strobe_until_ms = strobe_until;
        }
    }

    /// Advance time and produce the next frame; call at the stream rate.
    pub fn tick(&mut self, dt_ms: f64) -> LightFrame {
        self.clock_ms += dt_ms;

        // Fire chase flashes that have come due.
        let mut due = Vec::new();
        self.pending.retain(|p| {
            if p.due_ms <= self.clock_ms {
                due.push((p.channel, p.color, p.peak, p.strobe));
                false
            } else {
                true
            }
        });
        for (idx, color, peak, strobe) in due {
            self.apply_flash(idx, color, peak, strobe);
        }

        if let Some(p) = self.panic {
            match p {
                Panic::Blackout => {
                    return LightFrame {
                        channels: self
                            .channels
                            .iter()
                            .map(|c| (c.info.id, Color::new(0, 0, 0)))
                            .collect(),
                    }
                }
                Panic::WhiteFlash => {
                    return LightFrame {
                        channels: self
                            .channels
                            .iter()
                            .map(|c| (c.info.id, Color::new(255, 255, 255)))
                            .collect(),
                    }
                }
                Panic::Freeze => {
                    if let Some(f) = &self.frozen {
                        return f.clone();
                    }
                }
            }
        }

        self.strobe_phase = !self.strobe_phase;
        let s = &self.settings;
        let floor = s.brightness_min
            + (s.brightness_max - s.brightness_min) * 0.25 * self.intensity;
        let decay = (-(dt_ms as f32) / s.fade_ms.max(20.0)).exp();
        let color_step = (dt_ms as f32 / s.color_fade_ms.max(20.0)).min(1.0);

        for ch in &mut self.channels {
            ch.brightness = floor + (ch.brightness - floor) * decay;
            ch.color = ch.color.lerp(ch.target, color_step);
        }
        self.render()
    }

    fn render(&self) -> LightFrame {
        LightFrame {
            channels: self
                .channels
                .iter()
                .map(|ch| {
                    let mut b = ch.brightness.clamp(0.0, 1.0);
                    let mut color = ch.color;
                    if self.clock_ms < ch.strobe_until_ms {
                        if self.strobe_phase {
                            color = Color::new(255, 255, 255);
                        } else {
                            b = 0.0;
                        }
                    }
                    (ch.info.id, color.scaled(b))
                })
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core_types::Palette;

    fn palette() -> Palette {
        Palette {
            name: "test".into(),
            colors: vec![
                Color::new(255, 0, 0),
                Color::new(0, 255, 0),
                Color::new(0, 0, 255),
                Color::new(255, 255, 0),
            ],
        }
    }

    fn ch(id: u8, x: f32, z: f32) -> ChannelInfo {
        ChannelInfo { id, x, y: 0.0, z }
    }

    fn beat(band: Band) -> BandBeatEvent {
        BandBeatEvent {
            band,
            strength: 1.0,
            timestamp_ms: 0,
        }
    }

    fn color_of(frame: &LightFrame, id: u8) -> Color {
        frame.channels.iter().find(|(cid, _)| *cid == id).unwrap().1
    }

    fn no_chase_settings() -> EffectSettings {
        EffectSettings {
            chase_ms: 0.0,
            color_fade_ms: 20.0,
            ..Default::default()
        }
    }

    #[test]
    fn by_height_routes_kick_to_floor_and_hats_to_ceiling() {
        // Four lights bottom to top.
        let chans = [ch(0, 0.0, -1.0), ch(1, 0.0, -0.3), ch(2, 0.0, 0.3), ch(3, 0.0, 1.0)];
        let mut e = EffectEngine::new(&chans, palette(), no_chase_settings());

        e.on_beat(&beat(Band::Low));
        let f = e.tick(20.0);
        assert!(color_of(&f, 0).r > 100, "floor light should flash red on kick");
        assert!(color_of(&f, 3).r < 60, "ceiling light must not react to kick");

        e.on_beat(&beat(Band::High));
        let f = e.tick(20.0);
        let top = color_of(&f, 3);
        assert!(top.r > 100 && top.g > 100, "ceiling light should flash yellow on hats");
    }

    #[test]
    fn empty_bands_fold_to_nearest_group() {
        // Only two lights: every band must still reach one of them.
        let chans = [ch(0, 0.0, -1.0), ch(1, 0.0, 1.0)];
        let mut e = EffectEngine::new(&chans, palette(), no_chase_settings());
        e.on_beat(&beat(Band::LowMid));
        let f = e.tick(20.0);
        let lit = f.channels.iter().any(|(_, c)| c.g > 100);
        assert!(lit, "low-mid beat should light the nearest (bottom) group");
    }

    #[test]
    fn chase_staggers_flashes_left_to_right() {
        let chans = [ch(0, -1.0, 0.0), ch(1, 1.0, 0.0)];
        let mut settings = no_chase_settings();
        settings.assignment = BandAssignment::All;
        settings.chase_ms = 100.0;
        let mut e = EffectEngine::new(&chans, palette(), settings);
        e.on_beat(&beat(Band::Low));
        let f = e.tick(20.0);
        let left = color_of(&f, 0);
        let right = color_of(&f, 1);
        assert!(left.r > 100, "left light flashes immediately");
        assert!(right.r < 60, "right light is still waiting on the chase");
        for _ in 0..5 {
            e.tick(20.0);
        }
        let f = e.tick(20.0);
        assert!(color_of(&f, 1).r > 60, "right light flashes after the stagger");
    }

    #[test]
    fn custom_assignment_still_works() {
        let chans = [ch(0, 0.0, 0.0), ch(1, 0.0, 0.0)];
        let mut settings = no_chase_settings();
        settings.assignment = BandAssignment::Custom;
        settings.channel_bands = vec![(0, vec![Band::Low]), (1, vec![Band::High])];
        let mut e = EffectEngine::new(&chans, palette(), settings);
        e.on_beat(&beat(Band::Low));
        let f = e.tick(20.0);
        assert!(color_of(&f, 0).r > 100);
        assert!(color_of(&f, 1).r < 60);
    }

    #[test]
    fn blackout_panic_overrides() {
        let chans = [ch(0, 0.0, 0.0)];
        let mut e = EffectEngine::new(&chans, palette(), no_chase_settings());
        e.on_beat(&beat(Band::Low));
        e.set_panic(Some(Panic::Blackout));
        let frame = e.tick(20.0);
        assert!(frame.channels.iter().all(|(_, c)| *c == Color::new(0, 0, 0)));
    }
}
