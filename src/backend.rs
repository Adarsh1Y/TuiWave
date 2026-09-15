use std::sync::Arc;

use anyhow::Result;
use tokio::sync::{RwLock, mpsc};

use crate::config::{BackendKind, Config};
use crate::mpd::MpdClient;
use crate::mpv::{Mpv, MpvEvent, PlaybackState};

/// Events from either playback engine. Kept identical to mpv's shape so the
/// TUI only ever reasons about one event vocabulary.
pub type Event = MpvEvent;

/// The active playback engine: either a spawned mpv or a connected MPD.
#[derive(Clone)]
pub enum Backend {
    Mpv(Mpv),
    Mpd(MpdClient),
}

impl std::fmt::Debug for Backend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Mpv(_) => "Backend::Mpv",
            Self::Mpd(_) => "Backend::Mpd",
        })
    }
}

impl Backend {
    /// Spawn/connect the engine selected by `cfg.backend`.
    pub async fn spawn(cfg: &Config) -> Result<(Self, mpsc::UnboundedReceiver<Event>)> {
        match cfg.backend {
            BackendKind::Mpv => {
                let (mpv, events) = Mpv::spawn(cfg).await?;
                Ok((Self::Mpv(mpv), events))
            }
            BackendKind::Mpd => {
                let (client, state, events) = MpdClient::connect(&cfg.mpd).await?;
                // Mirror the configured start volume into MPD.
                let _ = client.set_volume(cfg.volume as f64).await;
                let _ = state;
                Ok((Self::Mpd(client), events))
            }
        }
    }

    pub fn state(&self) -> Arc<RwLock<PlaybackState>> {
        match self {
            Self::Mpv(m) => m.state(),
            Self::Mpd(m) => m.state(),
        }
    }

    pub fn is_mpv(&self) -> bool {
        matches!(self, Self::Mpv(_))
    }

    /// EQ filters are an mpv-only concept.
    pub fn supports_eq(&self) -> bool {
        self.is_mpv()
    }

    pub async fn load(&self, url: &str, title: &str, paused: bool) -> Result<()> {
        match self {
            Self::Mpv(m) => m.load(url, title, paused).await,
            Self::Mpd(c) => c.load(url).await,
        }
    }

    pub async fn play_pause(&self) -> Result<()> {
        match self {
            Self::Mpv(m) => m.play_pause().await,
            Self::Mpd(c) => c.play_pause().await,
        }
    }

    pub async fn seek(&self, seconds: f64, absolute: bool) -> Result<()> {
        match self {
            Self::Mpv(m) => m.seek(seconds, absolute).await,
            Self::Mpd(c) => c.seek(seconds, absolute).await,
        }
    }

    pub async fn set_volume(&self, volume: f64) -> Result<()> {
        match self {
            Self::Mpv(m) => m.set_volume(volume).await,
            Self::Mpd(c) => c.set_volume(volume).await,
        }
    }

    /// Apply an EQ chain. Only meaningful for mpv.
    pub async fn set_af(&self, chain: &str) -> Result<()> {
        match self {
            Self::Mpv(m) => m.set_af(chain).await,
            Self::Mpd(_) => Ok(()),
        }
    }

    /// Cycle to the next mpv audio output; `Ok(None)` when unsupported.
    pub async fn next_audio_output(&self) -> Result<Option<String>> {
        match self {
            Self::Mpv(m) => m.next_audio_output().await,
            Self::Mpd(_) => Ok(None),
        }
    }

    pub async fn shutdown(&self) {
        match self {
            Self::Mpv(m) => m.shutdown().await,
            Self::Mpd(c) => c.shutdown().await,
        }
    }
}