//! Fallback stream resolution via the `yt-dlp` executable.
//!
//! YouTube's live bot-gating of the InnerTube protocol occasionally outruns
//! the direct/nsig path. yt-dlp bundles its own JS challenge solvers, so it
//! keeps working even when our direct resolution does not. This module shells
//! out to it only as a last resort and feeds the resulting raw audio URL to
//! the mpv engine (which is unchanged).

use anyhow::Context;

use super::player::StreamFormat;

const WATCH_PREFIX: &str = "https://www.youtube.com/watch?v=";

/// Resolve a playable audio URL with the `yt-dlp` binary.
pub async fn resolve(ytdl_path: &str, video_id: &str) -> anyhow::Result<StreamFormat> {
    let watch = format!("{WATCH_PREFIX}{video_id}");
    let output = tokio::process::Command::new(ytdl_path)
        .args([
            "--no-warnings",
            "--no-playlist",
            "-f",
            "bestaudio/b",
            "-g",
            &watch,
        ])
        .output()
        .await
        .with_context(|| format!("failed to execute {ytdl_path}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let tail: Vec<&str> = stderr.lines().rev().take(3).collect();
        anyhow::bail!("{ytdl_path} failed for {video_id}: {}", tail.join(" | "));
    }

    let text = String::from_utf8_lossy(&output.stdout);
    let url = text
        .lines()
        .map(str::trim)
        .find(|l| l.starts_with("http"))
        .context("yt-dlp returned no stream URL")?
        .to_string();

    Ok(StreamFormat {
        url,
        itag: None,
        bitrate: None,
        mime: None,
    })
}
