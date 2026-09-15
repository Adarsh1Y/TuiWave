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
    pub name: String,
    pub bass: i8,
    pub mid: i8,
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
            presets: vec![EqPreset {
                name: "Deep Bass".to_string(),
                bass: 12,
                mid: 0,
                treble: -2,
            }],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub volume: u8,
    pub mpv_path: String,
    pub cache_dir: PathBuf,
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
                toml::from_str(&raw).map_err(|e| anyhow::anyhow!("invalid config {path:?}: {e}"))?
            }
            Err(_) => {
                let cfg = Self::default();
                if let Some(dir) = path.parent() {
                    std::fs::create_dir_all(dir)?;
                }
                let toml = toml::to_string_pretty(&cfg)?;
                std::fs::write(&path, toml)?;
                cfg
            }
        };
        std::fs::create_dir_all(cfg.art_cache_dir())?;
        std::fs::create_dir_all(cfg.playlists_dir())?;
        Ok(cfg)
    }

    pub fn save(&self) -> anyhow::Result<()> {
        let path = Self::path()?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let toml = toml::to_string_pretty(&self)?;
        std::fs::write(&path, toml)?;
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
        self.eq.presets.iter().find(|p| &p.name == name).map(|p| p.filter_chain())
    }
}