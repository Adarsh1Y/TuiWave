use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum TrackSource {
    #[default]
    YtMusic,
    LocalFile(PathBuf),
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Track {
    pub video_id: String,
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    pub thumbnail_url: Option<String>,
    pub duration: Option<u32>,
    /// Category label from search rows ("Song", "Video", "Episode", ...).
    #[serde(default)]
    pub category: Option<String>,
    /// Artist browse id (channel id) when the row carries one.
    #[serde(default)]
    pub artist_id: Option<String>,
    /// Album browse id (MPRE...) when the row carries one.
    #[serde(default)]
    pub album_id: Option<String>,
    /// Primary browse target for non-song rows (album/artist/playlist card).
    #[serde(default)]
    pub browse_id: Option<String>,
    pub source: TrackSource,
}

impl Track {
    /// Stable identity for deduplication / like tracking.
    pub fn key(&self) -> String {
        match &self.source {
            TrackSource::YtMusic => {
                if !self.video_id.is_empty() {
                    self.video_id.clone()
                } else if let Some(browse_id) = &self.browse_id {
                    format!("browse:{browse_id}")
                } else {
                    format!("yt:{}", self.title)
                }
            }
            TrackSource::LocalFile(path) => format!("file:{}", path.display()),
        }
    }

    /// File extension of a local track (e.g. "FLAC") if any.
    pub fn source_path_extension(&self) -> Option<String> {
        match &self.source {
            TrackSource::LocalFile(path) => path
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| e.to_string()),
            TrackSource::YtMusic => None,
        }
    }
}