//! Live smoke test: scrape config, search YT Music, resolve a stream.
//!
//! Run with: cargo run --example smoke -- "guns n roses"

use lastwave::innertube;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let query = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "queen bohemian rhapsody".to_string());

    let cfg = innertube::config::scrape().await;
    println!(
        "cfg: key={} ver={} visitor={}",
        cfg.api_key, cfg.client_version, cfg.visitor_data
    );

    let tracks = innertube::search::search_songs(&cfg, &query).await?;
    println!("found {} results", tracks.len());
    for (i, t) in tracks.iter().enumerate().take(10) {
        println!(
            "{i}: {} — {} [{}] {}",
            t.title,
            t.artist,
            t.album.clone().unwrap_or_default(),
            t.video_id
        );
    }

    if let Some(first) = tracks.first() {
        let mut cfg = cfg;
        let client = innertube::http_client();
        match innertube::resolve_stream(&client, &mut cfg, &first.video_id, Default::default())
            .await
        {
            Ok(fmt) => {
                println!(
                    "stream: itag={:?} bitrate={:?} url={}",
                    fmt.itag,
                    fmt.bitrate,
                    &fmt.url[..fmt.url.len().min(120)]
                );
                let resp = client.get(&fmt.url).send().await?;
                let range = resp
                    .headers()
                    .get("content-range")
                    .map(|v| v.to_str().unwrap_or("?").to_string())
                    .unwrap_or_else(|| "none".to_string());
                let status = resp.status();
                let head = resp.bytes().await?;
                println!(
                    "download check: status={status} bytes={} range-ok={}",
                    head.len(),
                    range
                );
                if !status.is_success() && head.len() < 1000 {
                    println!(
                        "body: {}",
                        String::from_utf8_lossy(&head[..head.len().min(200)])
                    );
                }
            }
            Err(e) => {
                println!("stream resolution FAILED: {e:#}");
                std::process::exit(2);
            }
        }
    }

    Ok(())
}
