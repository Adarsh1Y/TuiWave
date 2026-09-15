use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub volume: u8,
    pub mpv_path: String,
    pub cache_dir: PathBuf,
    pub autoplay: bool,
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
        Ok(cfg)
    }

    pub fn path() -> anyhow::Result<PathBuf> {
        let dir = dirs::config_dir()
            .ok_or_else(|| anyhow::anyhow!("cannot resolve config directory"))?
            .join("lastwave");
        Ok(dir.join("config.toml"))
    }

    pub fn art_cache_dir(&self) -> PathBuf {
        self.cache_dir.join("art")
    }
}
