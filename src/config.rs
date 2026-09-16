use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum BackendKind {
    #[serde(rename = "mpv")]
    #[default]
    Mpv,
    #[serde(rename = "mpd")]
    Mpd,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MpdConfig {
    /// Prefer a Unix socket when set (`/run/user/1000/mpd/socket`).
    pub socket: Option<PathBuf>,
    pub host: String,
    pub port: u16,
    /// Optional MPD password sent on connect.
    pub password: Option<String>,
}

impl Default for MpdConfig {
    fn default() -> Self {
        Self {
            socket: None,
            host: "127.0.0.1".to_string(),
            port: 6600,
            password: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Codec {
    #[serde(rename = "best")]
    #[default]
    Best,
    #[serde(rename = "opus")]
    Opus,
    #[serde(rename = "aac")]
    Aac,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EqPreset {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    #[serde(default)]
    pub bass: i8,
    #[serde(default)]
    pub mid: i8,
    #[serde(default)]
    pub treble: i8,
}

impl EqPreset {
    /// mpv `af` filter chain for this preset. Empty means "clean / no filter".
    pub fn filter_chain(&self) -> String {
        let mut parts = Vec::new();
        if self.bass != 0 {
            parts.push(format!("bass=g={}", self.bass));
        }
        if self.mid != 0 {
            parts.push(format!("equalizer=f=500:w=100:g={}", self.mid));
        }
        if self.treble != 0 {
            parts.push(format!("treble=g={}", self.treble));
        }
        parts.join(",")
    }

    /// The curated pre-built presets shipped with the app.
    pub fn builtin() -> Vec<EqPreset> {
        vec![
            EqPreset { name: "Deep Bass".into(), bass: 12, mid: 0, treble: -2 },
            EqPreset { name: "Bass Boost".into(), bass: 8, mid: 1, treble: 0 },
            EqPreset { name: "Rock".into(), bass: 5, mid: 3, treble: 4 },
            EqPreset { name: "Pop".into(), bass: -1, mid: 3, treble: 3 },
            EqPreset { name: "Jazz".into(), bass: 4, mid: 1, treble: 2 },
            EqPreset { name: "Classical".into(), bass: 3, mid: 0, treble: 3 },
            EqPreset { name: "Vocal".into(), bass: 0, mid: 5, treble: 2 },
            EqPreset { name: "Treble Boost".into(), bass: -2, mid: 0, treble: 9 },
        ]
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EqConfig {
    /// Name of the active preset (matches a preset in `presets`), if any.
    pub preset: Option<String>,
    pub presets: Vec<EqPreset>,
}

impl Default for EqConfig {
    fn default() -> Self {
        Self {
            preset: None,
            presets: EqPreset::builtin(),
        }
    }
}

impl EqConfig {
    /// Add the built-in presets that are missing from the list. Called on load
    /// so existing config files pick up newly shipped presets too.
    pub fn merge_builtin(&mut self) {
        for p in EqPreset::builtin() {
            if !self.presets.iter().any(|x| x.name == p.name) {
                self.presets.push(p);
            }
        }
    }

    /// The filter chain for a named preset, if it exists.
    pub fn chain_for(&self, name: &str) -> Option<String> {
        self.presets
            .iter()
            .find(|p| p.name == name)
            .map(|p| p.filter_chain())
    }

    /// Name of the preset that cycling (`alt+e`) should land on next, or None
    /// to turn the EQ off (end of the list).
    pub fn next_preset(&self) -> Option<String> {
        if self.presets.is_empty() {
            return None;
        }
        let idx = self
            .preset
            .as_deref()
            .and_then(|n| self.presets.iter().position(|p| p.name == n));
        match idx {
            None => Some(self.presets[0].name.clone()),
            Some(i) if i + 1 < self.presets.len() => Some(self.presets[i + 1].name.clone()),
            Some(_) => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub volume: u8,
    #[serde(default)]
    pub mpv_path: String,
    #[serde(default)]
    pub cache_dir: PathBuf,
    #[serde(default)]
    pub autoplay: bool,
    #[serde(default)]
    pub codec: Codec,
    #[serde(default)]
    pub local_dirs: Vec<PathBuf>,
    #[serde(default)]
    pub eq: EqConfig,
    /// Restore the previous queue/position on startup.
    #[serde(default = "resume_default")]
    pub resume: bool,
    /// Playback engine: `mpv` (default) or a running `mpd` daemon.
    #[serde(default)]
    pub backend: BackendKind,
    /// MPD connection settings, used when `backend = "mpd"`.
    #[serde(default)]
    pub mpd: MpdConfig,
}

fn resume_default() -> bool {
    true
}

impl Default for Config {
    fn default() -> Self {
        Self {
            volume: 100,
            mpv_path: "mpv".to_string(),
            cache_dir: dirs::cache_dir()
                .unwrap_or_else(|| PathBuf::from("/tmp"))
                .join("lastwave"),
            autoplay: true,
            codec: Codec::default(),
            local_dirs: vec![dirs::audio_dir().unwrap_or_else(|| PathBuf::from("~/Music"))],
            eq: EqConfig::default(),
            resume: true,
            backend: BackendKind::default(),
            mpd: MpdConfig::default(),
        }
    }
}

impl Config {
    pub fn load() -> anyhow::Result<Self> {
        let path = Self::path()?;
        let cfg = match std::fs::read_to_string(&path) {
            Ok(raw) => {
                let mut cfg = toml::from_str::<Self>(&raw)
                    .map_err(|e| anyhow::anyhow!("invalid config {path:?}: {e}"))?;
                cfg.eq.merge_builtin();
                cfg
            }
            Err(_) => {
                let cfg = Self::default();
                let toml = toml::to_string_pretty(&cfg)?;
                let _ = crate::atomic::atomic_write(&path, toml.as_bytes());
                cfg
            }
        };
        std::fs::create_dir_all(cfg.art_cache_dir())?;
        std::fs::create_dir_all(cfg.playlists_dir())?;
        Ok(cfg)
    }

    pub fn save(&self) -> anyhow::Result<()> {
        let path = Self::path()?;
        let toml = toml::to_string_pretty(&self)?;
        crate::atomic::atomic_write(&path, toml.as_bytes())?;
        Ok(())
    }

    pub fn path() -> anyhow::Result<PathBuf> {
        Ok(Self::config_dir()?.join("config.toml"))
    }

    pub fn config_dir() -> anyhow::Result<PathBuf> {
        let dir = dirs::config_dir()
            .ok_or_else(|| anyhow::anyhow!("cannot resolve config directory"))?
            .join("lastwave");
        std::fs::create_dir_all(&dir)?;
        Ok(dir)
    }

    pub fn art_cache_dir(&self) -> PathBuf {
        self.cache_dir.join("art")
    }

    pub fn playlists_dir(&self) -> PathBuf {
        Self::config_dir().unwrap_or_default().join("playlists")
    }

    pub fn active_eq_chain(&self) -> Option<String> {
        let name = self.eq.preset.as_ref()?;
        self.eq.chain_for(name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eq_cycles_through_builtin_presets() {
        let eq = EqConfig::default();
        assert_eq!(eq.presets.len(), 8);
        assert_eq!(eq.next_preset().as_deref(), Some("Deep Bass"));
        let mut eq = eq;
        eq.preset = eq.next_preset();
        assert_eq!(eq.preset.as_deref(), Some("Deep Bass"));
        assert_eq!(eq.next_preset().as_deref(), Some("Bass Boost"));
        eq.preset = Some("Treble Boost".into());
        assert_eq!(eq.next_preset(), None, "last preset wraps to off");
    }

    #[test]
    fn eq_presets_have_distinct_chains() {
        let eq = EqConfig::default();
        let chains: std::collections::HashSet<String> =
            eq.presets.iter().map(|p| p.filter_chain()).collect();
        assert!(chains.len() >= eq.presets.len() - 2, "presets should differ");
    }

    #[test]
    fn merge_builtin_adds_missing_presets() {
        let mut eq = EqConfig {
            preset: None,
            presets: vec![EqPreset { name: "Custom".into(), bass: 1, mid: 2, treble: 3 }],
        };
        eq.merge_builtin();
        assert!(eq.presets.iter().any(|p| p.name == "Deep Bass"));
        assert!(eq.presets.iter().any(|p| p.name == "Custom"));
        let before = eq.presets.len();
        eq.merge_builtin();
        assert_eq!(eq.presets.len(), before, "merge is idempotent");
    }
}