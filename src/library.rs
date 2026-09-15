//! Local FLAC / Opus / MP3 file library.

use std::path::{Path, PathBuf};

use crate::model::{Track, TrackSource};

const AUDIO_EXTS: [&str; 6] = ["flac", "opus", "ogg", "m4a", "mp3", "aac"];

fn is_audio_file(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| AUDIO_EXTS.iter().any(|x| e.eq_ignore_ascii_case(x)))
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk(&path, out);
        } else if is_audio_file(&path) {
            out.push(path);
        }
    }
}

/// Scan the configured directories for playable audio files.
pub fn scan_local_tracks(dirs: &[PathBuf]) -> Vec<Track> {
    let mut files = Vec::new();
    for dir in dirs {
        walk(dir, &mut files);
    }
    files.sort();

    files
        .into_iter()
        .map(|path| {
            let stem = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("unknown")
                .to_string();
            Track {
                video_id: String::new(),
                title: stem,
                artist: "Local file".to_string(),
                album: path.parent().and_then(|p| p.file_name()).and_then(|s| s.to_str()).map(str::to_string),
                thumbnail_url: None,
                duration: None,
                category: None,
                artist_id: None,
                source: TrackSource::LocalFile(path),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scans_flac_and_opus_only() {
        let dir = std::env::temp_dir().join(format!("lw-test-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        let files = ["a.flac", "b.opus", "c.ogg", "d.m4a", "e.mp3", "f.txt", "sub/g.FLAC"];
        for f in files {
            std::fs::write(dir.join(f), b"x").unwrap();
        }
        let tracks = scan_local_tracks(std::slice::from_ref(&dir));
        let names: Vec<String> = tracks.iter().map(|t| t.title.clone()).collect();
        assert_eq!(names, vec!["a", "b", "c", "d", "e", "g"]);
        assert!(tracks.iter().all(|t| matches!(t.source, TrackSource::LocalFile(_))));
        assert_eq!(tracks[0].key(), format!("file:{}", dir.join("a.flac").display()));
        std::fs::remove_dir_all(dir).ok();
    }
}