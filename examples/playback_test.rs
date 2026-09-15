//! End-to-end playback test: search → resolve → play via mpv → verify state.
//!
//! cargo run --example playback_test -- "bohemian rhapsody queen"

use std::time::Duration;

use lastwave::config::Config;
use lastwave::innertube;
use lastwave::mpv::{Mpv, MpvEvent};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let query = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "bohemian rhapsody queen".to_string());

    let cfg = innertube::config::scrape().await;
    let tracks = innertube::search::search_songs(&cfg, &query).await?;
    let first = tracks
        .iter()
        .find(|t| t.artist.to_lowercase().contains("queen"))
        .or(tracks.first())
        .expect("no results");
    println!("playing: {} — {}", first.title, first.artist);

    let mut cfg = cfg;
    let client = innertube::http_client();
    let stream = if let Ok(u) = std::env::var("LW_URL") {
        println!("playback_test: using LW_URL from env");
        lastwave::innertube::player::StreamFormat {
            itag: None,
            bitrate: None,
            url: u,
        }
    } else {
        innertube::resolve_stream(&client, &mut cfg, &first.video_id).await?
    };
    println!("stream itag={:?} bitrate={:?}", stream.itag, stream.bitrate);

    let config = Config::default();
    let (mpv, mut events) = Mpv::spawn(&config).await?;

    mpv.load(
        &stream.url,
        &format!("{} - {}", first.artist, first.title),
        false,
    )
    .await?;

    let mut end_reason: Option<String> = None;
    let deadline = tokio::time::sleep(Duration::from_secs(12));
    tokio::pin!(deadline);

    loop {
        tokio::select! {
            ev = events.recv() => match ev {
                Some(MpvEvent::EndFile { reason }) => {
                    println!("EndFile: reason={reason}");
                    if reason == "eof" { end_reason = Some(reason); break; }
                    if reason == "error" { break; }
                }
                Some(MpvEvent::FileLoaded) => {
                    println!("file-loaded event received ✓");
                }
                Some(MpvEvent::StateChanged) => {}
                None => break,
            },
            _ = &mut deadline => break,
        }
    }

    let state = mpv.state().read().await.clone();
    println!(
        "final state: pos={:?} dur={:?} paused={} idle={} title={:?}",
        state.time_pos, state.duration, state.paused, state.idle, state.media_title
    );
    let played = state.time_pos.unwrap_or(0.0) > 1.0;
    println!("played audio for 1s+: {played}");
    mpv.shutdown().await;
    if !played {
        anyhow::bail!("mpv did not actually play audio");
    }
    let _ = end_reason;
    println!("PASS");
    Ok(())
}
