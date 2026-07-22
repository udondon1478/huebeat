//! Genre -> color palette mapping with TOML persistence.
//! Palette slot order matters: bands map to slots
//! (slot 0 = Low/kick, 1 = LowMid, 2 = HighMid, 3 = High).

use std::collections::HashMap;
use std::path::Path;

use core_types::{Color, Genre, Palette};
use serde::{Deserialize, Serialize};

fn pal(name: &str, hex: [&str; 4]) -> Palette {
    Palette {
        name: name.to_string(),
        colors: hex.iter().map(|h| Color::from_hex(h).unwrap()).collect(),
    }
}

/// Built-in palette for a genre (blue-ish deep house, red hardcore,
/// pink kawaii future bass, ...).
pub fn default_palette(genre: Genre) -> Palette {
    match genre {
        Genre::DeepHouse => pal("Deep House", ["#0f52ba", "#00a6a6", "#4361ee", "#7fd8ff"]),
        Genre::House => pal("House", ["#ff8c00", "#ffd166", "#ef476f", "#ffe8c2"]),
        Genre::TechHouse => pal("Tech House", ["#00b894", "#0984e3", "#55efc4", "#dfe6e9"]),
        Genre::ElectroHouse => {
            pal("EDM / Big Room", ["#ff3d00", "#00e5ff", "#ffea00", "#f5f5f5"])
        }
        Genre::NuDisco => pal("Nu Disco", ["#ff6f61", "#ffd700", "#40e0d0", "#ff9ff3"]),
        Genre::NetPop => pal("Net Pop", ["#ff8fb1", "#7ec8ff", "#b5ead7", "#fff5ba"]),
        Genre::UkGarage => pal("UK Garage", ["#8e2de2", "#ff6ec7", "#00c9a7", "#f0e6ff"]),
        Genre::JerseyClub => pal("Jersey Club", ["#ff5e00", "#9d4edd", "#ffc300", "#ffe6f2"]),
        Genre::Techno => pal("Techno", ["#6f00ff", "#00e5ff", "#3a0ca3", "#e0e0ff"]),
        Genre::Trance => pal("Trance", ["#2437ff", "#8a2be2", "#00bfff", "#e6ccff"]),
        Genre::Psytrance => pal("Psytrance", ["#76ff03", "#d500f9", "#00e5ff", "#ffea00"]),
        Genre::Hardstyle => pal("Hardstyle", ["#ff0033", "#2929ff", "#ff6600", "#e0e0ff"]),
        Genre::Eurobeat => pal("Eurobeat", ["#ff2079", "#00f0ff", "#ffe600", "#ff9ecd"]),
        Genre::AnisonRemix => {
            pal("Anison Remix", ["#ff3d81", "#3dc9ff", "#ffd23d", "#c86bff"])
        }
        Genre::Breakbeat => pal("Breakbeat", ["#f77f00", "#3d348b", "#f5cb5c", "#e8e8e8"]),
        Genre::DrumAndBass => pal("Drum & Bass", ["#00c853", "#aeea00", "#00bfa5", "#ccff90"]),
        Genre::Dubstep => pal("Dubstep", ["#39ff14", "#7c4dff", "#00e676", "#b388ff"]),
        Genre::Trap => pal("Trap", ["#9d00ff", "#ff1361", "#38006b", "#f3e5f5"]),
        Genre::Hyperflip => pal("Hyperflip", ["#ff00cc", "#00ffcc", "#7c4dff", "#f8ff66"]),
        Genre::FutureBass => pal("Future Bass", ["#00d2ff", "#ff7eb3", "#7afcff", "#feff9c"]),
        Genre::FutureCore => pal("Future Core", ["#00b3ff", "#3d5afe", "#7df9ff", "#e3f6ff"]),
        Genre::Hardcore => pal("Hardcore", ["#ff1744", "#ff6d00", "#d50000", "#ffab91"]),
        Genre::KawaiiFutureBass => {
            pal("Kawaii Future Bass", ["#ff4fd8", "#ff9ecd", "#b388ff", "#fff0f7"])
        }
        Genre::HipHop => pal("Hip Hop", ["#ff9100", "#ffd54f", "#8d6e63", "#fff3e0"]),
        Genre::Rnb => pal("R&B", ["#7b2cbf", "#c77dff", "#3c096c", "#e0aaff"]),
        Genre::Reggaeton => pal("Reggaeton", ["#ff9f1c", "#e71d36", "#2ec4b6", "#fff3b0"]),
        Genre::Synthwave => pal("Synthwave", ["#ff2975", "#00fff9", "#8c1eff", "#f222ff"]),
        Genre::Ambient => pal("Ambient", ["#4dd0e1", "#b2ebf2", "#9fa8da", "#e8eaf6"]),
        Genre::Unknown => pal("Auto", ["#ff0055", "#00c8ff", "#aa00ff", "#ffee00"]),
    }
}

/// Serializable palette store: per-genre palettes plus user customs.
pub struct PaletteStore {
    pub genre_map: HashMap<Genre, Palette>,
    pub custom: Vec<Palette>,
}

impl Default for PaletteStore {
    fn default() -> Self {
        Self {
            genre_map: Genre::ALL
                .iter()
                .map(|&g| (g, default_palette(g)))
                .collect(),
            custom: Vec::new(),
        }
    }
}

#[derive(Serialize, Deserialize)]
struct PaletteToml {
    name: String,
    colors: Vec<String>,
}

#[derive(Serialize, Deserialize, Default)]
struct StoreToml {
    #[serde(default)]
    genres: HashMap<String, PaletteToml>,
    #[serde(default)]
    custom: Vec<PaletteToml>,
}

fn to_toml(p: &Palette) -> PaletteToml {
    PaletteToml {
        name: p.name.clone(),
        colors: p.colors.iter().map(|c| c.to_hex()).collect(),
    }
}

fn from_toml(p: &PaletteToml) -> Palette {
    Palette {
        name: p.name.clone(),
        colors: p
            .colors
            .iter()
            .filter_map(|h| Color::from_hex(h))
            .collect(),
    }
}

impl PaletteStore {
    pub fn palette_for(&self, genre: Genre) -> Palette {
        self.genre_map
            .get(&genre)
            .cloned()
            .unwrap_or_else(|| default_palette(genre))
    }

    /// Restore one genre's palette to its built-in default and return it.
    pub fn reset_genre(&mut self, genre: Genre) -> Palette {
        let p = default_palette(genre);
        self.genre_map.insert(genre, p.clone());
        p
    }

    /// Restore every genre palette to its built-in default.
    pub fn reset_all(&mut self) {
        self.genre_map = Genre::ALL
            .iter()
            .map(|&g| (g, default_palette(g)))
            .collect();
    }

    pub fn load(path: &Path) -> std::io::Result<Self> {
        let text = std::fs::read_to_string(path)?;
        let parsed: StoreToml = toml::from_str(&text)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        let mut store = PaletteStore::default();
        for (key, pt) in &parsed.genres {
            if let Some(g) = Genre::from_id(key) {
                store.genre_map.insert(g, from_toml(pt));
            }
        }
        store.custom = parsed.custom.iter().map(from_toml).collect();
        Ok(store)
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        let out = StoreToml {
            genres: self
                .genre_map
                .iter()
                .map(|(g, p)| (g.as_str().to_string(), to_toml(p)))
                .collect(),
            custom: self.custom.iter().map(to_toml).collect(),
        };
        let text = toml::to_string_pretty(&out)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(path, text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let dir = std::env::temp_dir().join("hue2-palette-test");
        let path = dir.join("palettes.toml");
        let mut store = PaletteStore::default();
        store.genre_map.insert(
            Genre::DeepHouse,
            pal("My Deep", ["#000011", "#000022", "#000033", "#000044"]),
        );
        store.save(&path).unwrap();
        let loaded = PaletteStore::load(&path).unwrap();
        assert_eq!(loaded.palette_for(Genre::DeepHouse).name, "My Deep");
        assert_eq!(
            loaded.palette_for(Genre::Hardcore).name,
            default_palette(Genre::Hardcore).name
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
