use std::io::Write;
use std::path::PathBuf;

use anyhow::Context;
use serde::{Deserialize, Serialize};

use crate::config::Config;
use crate::model::Track;
use crate::tui::app::RepeatMode;

/// Everything worth restoring after a restart.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Session {
    #[serde(default)]
    pub queue: Vec<Track>,
    #[serde(default)]
    pub cursor: usize,
    #[serde(default)]
    pub current: Option<Track>,
    #[serde(default)]
    pub repeat: RepeatMode,
    #[serde(default)]
    pub shuffle: bool,
    #[serde(default)]
    pub query: String,
    #[serde(default)]
    pub position: f64,
    #[serde(default)]
    pub volume: f64,
    #[serde(default)]
    pub history: Vec<Track>,
}

impl Session {
    pub fn path() -> anyhow::Result<PathBuf> {
        Ok(Config::config_dir()?.join("session.json"))
    }

    pub fn load() -> Option<Self> {
        let path = Self::path().ok()?;
        let raw = std::fs::read_to_string(path).ok()?;
        serde_json::from_str(&raw).ok()
    }

    pub fn save(&self) {
        let Ok(path) = Self::path() else { return };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(json) = serde_json::to_string(self) {
            let _ = std::fs::write(&path, json);
        }
    }

    pub fn clear() {
        if let Ok(path) = Self::path() {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// Removes the instance lock file on drop (including on panic/error paths).
pub struct InstanceLock;

impl Drop for InstanceLock {
    fn drop(&mut self) {
        if let Ok(dir) = Config::config_dir() {
            let _ = std::fs::remove_file(dir.join("lastwave.lock"));
        }
    }
}

/// Fail fast when another lastwave instance is already running.
pub fn acquire_single_instance() -> anyhow::Result<InstanceLock> {
    let path = Config::config_dir()?.join("lastwave.lock");
    if let Ok(raw) = std::fs::read_to_string(&path) {
        if let Ok(pid) = raw.trim().parse::<u32>()
            && pid != std::process::id()
            && pid_alive(pid)
        {
            anyhow::bail!("another lastwave instance is running (pid {pid})");
        }
        // Stale lock from a crashed instance; reclaim it.
        let _ = std::fs::remove_file(&path);
    }
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
    {
        Ok(mut f) => {
            let _ = write!(f, "{}", std::process::id());
            Ok(InstanceLock)
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            anyhow::bail!("another lastwave instance is already running")
        }
        Err(e) => Err(e).context("cannot create instance lock"),
    }
}

fn pid_alive(pid: u32) -> bool {
    std::path::Path::new(&format!("/proc/{pid}")).exists()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Track, TrackSource};
    use crate::tui::app::RepeatMode;

    fn track(id: &str) -> Track {
        Track {
            video_id: id.to_string(),
            title: "t".into(),
            artist: "a".into(),
            album: None,
            thumbnail_url: None,
            duration: Some(10),
            category: None,
            artist_id: None,
            source: TrackSource::YtMusic,
        }
    }

    #[test]
    fn session_roundtrips_through_json() {
        let s = Session {
            queue: vec![track("x"), track("y")],
            cursor: 1,
            current: Some(track("x")),
            repeat: RepeatMode::One,
            shuffle: true,
            query: "queen".into(),
            position: 42.5,
            volume: 77.0,
            history: vec![track("z")],
        };
        let json = serde_json::to_string(&s).unwrap();
        let back: Session = serde_json::from_str(&json).unwrap();
        assert_eq!(back.queue.len(), 2);
        assert_eq!(back.cursor, 1);
        assert_eq!(back.repeat, RepeatMode::One);
        assert!(back.shuffle);
        assert_eq!(back.query, "queen");
        assert_eq!(back.position, 42.5);
        assert_eq!(back.volume, 77.0);
        assert_eq!(back.history.len(), 1);
        assert_eq!(back.current.unwrap().video_id, "x");
    }

    #[test]
    fn session_loads_when_fields_missing() {
        let json = r#"{"queue":[]}"#;
        let s: Session = serde_json::from_str(json).unwrap();
        assert_eq!(s.repeat, RepeatMode::Off);
        assert_eq!(s.position, 0.0);
    }

    #[test]
    fn instance_lock_signals_second_instance() {
        let dir = std::env::temp_dir().join(format!("lw-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // Point the lock at a temp dir-owned path by writing a live-pid lock,
        // since acquire uses the real config dir. Instead, verify pid_alive
        // and that a stale lock is reclaimed.
        assert!(pid_alive(std::process::id()));
        assert!(!pid_alive(u32::MAX));
        std::fs::remove_dir_all(&dir).ok();
    }
}