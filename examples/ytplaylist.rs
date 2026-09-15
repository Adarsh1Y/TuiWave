//! Live playlist fetch test.
//!
//! cargo run --example ytplaylist -- "https://music.youtube.com/playlist?list=PLFA20DD1A5D12F245"

use lastwave::innertube;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let input = std::env::args().nth(1).unwrap_or_else(|| {
        "https://music.youtube.com/playlist?list=PL62S1knDp7HarDGfLfF_OiYUshgM77Cwc".to_string()
    });

    let cfg = innertube::config::scrape().await;
    let client = innertube::http_client();
    let playlist = innertube::playlist::fetch_playlist(&client, &cfg, &input).await?;
    println!(
        "playlist: '{}' — {} tracks",
        playlist.title, playlist.track_count
    );
    for (i, t) in playlist.tracks.iter().enumerate().take(10) {
        println!(
            "{i}: {} — {} [{}] {}",
            t.title,
            t.artist,
            t.album.clone().unwrap_or_default(),
            t.video_id
        );
    }
    Ok(())
}