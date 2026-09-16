//! Persistent playlists: a built-in "Liked Songs" playlist plus named
//! playlists saved from the queue. Stored as JSON under the config dir.

use std::path::PathBuf;

use crate::config::Config;
use crate::model::Track;

const LIKED_FILE: &str = "liked.json";

fn playlists_dir() -> anyhow::Result<PathBuf> {
    Ok(Config::config_dir()?.join("playlists"))
}

fn liked_path() -> anyhow::Result<PathBuf> {
    Ok(playlists_dir()?.join(LIKED_FILE))
}

fn playlist_path(name: &str) -> anyhow::Result<PathBuf> {
    Ok(playlists_dir()?.join(sanitize(name) + ".json"))
}

/// Keep playlist file names filesystem-safe.
fn sanitize(name: &str) -> String {
    let clean: String = name
        .trim()
        .chars()
        .filter(|c| !c.is_control() && *c != '/' && *c != '\\' && *c != '.')
        .collect();
    if clean.is_empty() {
        "playlist".to_string()
    } else {
        clean
    }
}

fn read_tracks(path: &PathBuf) -> anyhow::Result<Vec<Track>> {
    match std::fs::read_to_string(path) {
        Ok(raw) if !raw.trim().is_empty() => Ok(serde_json::from_str(&raw)?),
        _ => Ok(Vec::new()),
    }
}

pub fn load_liked() -> anyhow::Result<Vec<Track>> {
    read_tracks(&liked_path()?)
}

pub fn save_liked(tracks: &[Track]) -> anyhow::Result<()> {
    let path = liked_path()?;
    crate::atomic::atomic_write(&path, serde_json::to_string_pretty(tracks)?.as_bytes())?;
    Ok(())
}

/// Names of named playlists (not including the built-in Liked list).
pub fn list_playlists() -> anyhow::Result<Vec<String>> {
    let dir = playlists_dir()?;
    let mut names = Vec::new();
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Ok(names);
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if entry.path().extension().and_then(|e| e.to_str()) == Some("json")
            && name != LIKED_FILE
            && let Some(stem) = name.strip_suffix(".json")
        {
            names.push(stem.to_string());
        }
    }
    names.sort();
    Ok(names)
}

pub fn load_playlist(name: &str) -> anyhow::Result<Vec<Track>> {
    read_tracks(&playlist_path(name)?)
}

pub fn save_playlist(name: &str, tracks: &[Track]) -> anyhow::Result<()> {
    let path = playlist_path(name)?;
    crate::atomic::atomic_write(&path, serde_json::to_string_pretty(tracks)?.as_bytes())?;
    Ok(())
}

pub fn delete_playlist(name: &str) -> anyhow::Result<()> {
    std::fs::remove_file(playlist_path(name)?)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(id: &str) -> Track {
        Track {
            video_id: id.to_string(),
            title: id.to_string(),
            artist: "a".to_string(),
            album: None,
            thumbnail_url: None,
            duration: None,
            category: None,
            artist_id: None,
            album_id: None,
            browse_id: None,
            source: Default::default(),
        }
    }

    #[test]
    fn named_playlist_roundtrip() {
        let name = format!("unit-test-{}", std::process::id());
        save_playlist(&name, &[track("1"), track("2")]).unwrap();
        let back = load_playlist(&name).unwrap();
        assert_eq!(back.len(), 2);
        assert_eq!(back[0].video_id, "1");
        assert!(list_playlists().unwrap().iter().any(|n| n == &name));
        delete_playlist(&name).unwrap();
        assert!(load_playlist(&name).unwrap().is_empty());
    }

    #[test]
    fn sanitizes_names() {
        assert_eq!(sanitize("../evil name"), "evil name");
        assert_eq!(sanitize(""), "playlist");
    }

    #[test]
    fn liked_roundtrip_and_dedupe_key() {
        let t1 = track("vid");
        let t2 = track("vid");
        assert_eq!(t1.key(), t2.key());
        save_liked(&[t1]).unwrap();
        assert_eq!(load_liked().unwrap().len(), 1);
    }
}