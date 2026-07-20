//! Effect engine: turns beat events + intensity into 50 Hz light frames.
//! Multi-band mapping: each band lights its palette slot color on the
//! channels assigned to it (Sound2Light style).

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
    /// Bands each channel reacts to; empty = all bands.
    #[serde(default)]
    pub channel_bands: Vec<(u8, Vec<Band>)>,
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
            channel_bands: Vec::new(),
            strobe_on_peaks: true,
        }
    }
}

struct ChannelState {
    id: u8,
    color: Color,
    target: Color,
    brightness: f32,
    strobe_until_ms: f64,
}

pub struct EffectEngine {
    settings: EffectSettings,
    palette: Palette,
    channels: Vec<ChannelState>,
    intensity: f32,
    panic: Option<Panic>,
    clock_ms: f64,
    strobe_phase: bool,
    frozen: Option<LightFrame>,
}

impl EffectEngine {
    pub fn new(channel_ids: &[u8], palette: Palette, settings: EffectSettings) -> Self {
        let base = palette.slot(0);
        Self {
            channels: channel_ids
                .iter()
                .map(|&id| ChannelState {
                    id,
                    color: base,
                    target: base,
                    brightness: settings.brightness_min,
                    strobe_until_ms: 0.0,
                })
                .collect(),
            settings,
            palette,
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

    fn channel_reacts(&self, idx: usize, band: Band) -> bool {
        let id = self.channels[idx].id;
        match self
            .settings
            .channel_bands
            .iter()
            .find(|(cid, _)| *cid == id)
        {
            Some((_, bands)) if !bands.is_empty() => bands.contains(&band),
            _ => true,
        }
    }

    pub fn on_beat(&mut self, beat: &BandBeatEvent) {
        let color = self.palette.slot(self.settings.band_slots[beat.band.index()]);
        let s = &self.settings;
        let peak = s.brightness_min
            + (s.brightness_max - s.brightness_min) * beat.strength.clamp(0.2, 1.0);
        let strobe = s.strobe_on_peaks
            && beat.strength > 0.9
            && (s.mode == EffectMode::Strobe
                || (s.mode == EffectMode::Auto && self.intensity > 0.75));
        let prob = s.per_light_probability;
        let strobe_until = self.clock_ms + 120.0;

        for i in 0..self.channels.len() {
            if !self.channel_reacts(i, beat.band) {
                continue;
            }
            if prob < 1.0 && rand::random::<f32>() > prob {
                continue;
            }
            let ch = &mut self.channels[i];
            ch.target = color;
            ch.brightness = ch.brightness.max(peak);
            if strobe {
                ch.strobe_until_ms = strobe_until;
            }
        }
    }

    /// Advance time and produce the next frame; call at the stream rate.
    pub fn tick(&mut self, dt_ms: f64) -> LightFrame {
        self.clock_ms += dt_ms;
        if let Some(p) = self.panic {
            match p {
                Panic::Blackout => {
                    return LightFrame {
                        channels: self.channels.iter().map(|c| (c.id, Color::new(0, 0, 0))).collect(),
                    }
                }
                Panic::WhiteFlash => {
                    return LightFrame {
                        channels: self
                            .channels
                            .iter()
                            .map(|c| (c.id, Color::new(255, 255, 255)))
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
                    (ch.id, color.scaled(b))
                })
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core_types::Palette;

    fn engine() -> EffectEngine {
        let palette = Palette {
            name: "test".into(),
            colors: vec![
                Color::new(255, 0, 0),
                Color::new(0, 255, 0),
                Color::new(0, 0, 255),
                Color::new(255, 255, 0),
            ],
        };
        let mut settings = EffectSettings::default();
        settings.channel_bands = vec![(0, vec![Band::Low]), (1, vec![Band::High])];
        settings.color_fade_ms = 20.0;
        EffectEngine::new(&[0, 1], palette, settings)
    }

    fn beat(band: Band) -> BandBeatEvent {
        BandBeatEvent {
            band,
            strength: 1.0,
            timestamp_ms: 0,
        }
    }

    #[test]
    fn band_routing_hits_assigned_channel_only() {
        let mut e = engine();
        e.on_beat(&beat(Band::Low));
        let frame = e.tick(20.0);
        let ch0 = frame.channels.iter().find(|(id, _)| *id == 0).unwrap().1;
        let ch1 = frame.channels.iter().find(|(id, _)| *id == 1).unwrap().1;
        // Channel 0 flashes red (slot 0); channel 1 stays near floor.
        assert!(ch0.r > 150, "expected bright red on ch0, got {ch0:?}");
        assert!(ch1.r < 60 && ch1.g < 60, "ch1 should stay dim, got {ch1:?}");
    }

    #[test]
    fn brightness_decays_after_beat() {
        let mut e = engine();
        e.on_beat(&beat(Band::Low));
        let first = e.tick(20.0).channels[0].1;
        for _ in 0..60 {
            e.tick(20.0);
        }
        let later = e.tick(20.0).channels[0].1;
        assert!(later.r < first.r, "beat flash should decay: {first:?} -> {later:?}");
    }

    #[test]
    fn blackout_panic_overrides() {
        let mut e = engine();
        e.on_beat(&beat(Band::Low));
        e.set_panic(Some(Panic::Blackout));
        let frame = e.tick(20.0);
        assert!(frame.channels.iter().all(|(_, c)| *c == Color::new(0, 0, 0)));
    }
}
