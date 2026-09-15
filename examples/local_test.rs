use std::time::Duration;

use lastwave::config::{Config, EqConfig};
use lastwave::mpv::{Mpv, MpvEvent};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let file = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "/tmp/opencode/test440.flac".to_string());

    let mut config = Config::default();
    config.eq = EqConfig {
        preset: Some("Deep Bass".to_string()),
        presets: config.eq.presets,
    };
    let (mpv, mut events) = Mpv::spawn(&config).await?;

    let chain = config.active_eq_chain().unwrap();
    println!("requested chain: {chain}");
    mpv.set_af(&chain).await?;

    mpv.load(&file, &format!("local-file: {file}"), false).await?;
    tokio::time::sleep(Duration::from_millis(800)).await;
    println!("af 800ms after load: {:?}", mpv.get_af().await.ok());

    mpv.set_af(&chain).await?;
    tokio::time::sleep(Duration::from_millis(600)).await;
    println!("af after re-apply mid-play: {:?}", mpv.get_af().await.ok());
    mpv.set_af("").await?;
    tokio::time::sleep(Duration::from_millis(600)).await;
    println!("af after reset: {:?}", mpv.get_af().await.ok());
    mpv.set_af(&chain).await?;

    let mut played = false;
    let deadline = tokio::time::sleep(Duration::from_secs(6));
    tokio::pin!(deadline);
    loop {
        tokio::select! {
            ev = events.recv() => match ev {
                Some(MpvEvent::EndFile { reason }) => {
                    println!("EndFile: reason={reason}");
                    if reason == "eof" { break; }
                    if reason == "error" { anyhow::bail!("mpv end-file error for local file"); }
                }
                Some(MpvEvent::FileLoaded) => {
                    println!("file-loaded ✓");
                    println!("af while playing: {:?}", mpv.get_af().await.ok());
                }
                Some(MpvEvent::StateChanged) => {
                    let s = mpv.state().read().await.clone();
                    if s.time_pos.unwrap_or(0.0) > 1.0 { played = true; }
                }
                None => break,
            },
            _ = &mut deadline => break,
        }
    }

    let af = mpv.get_af().await?;
    println!("local playback played={played} af={af}");

    let state = mpv.state().read().await.clone();
    println!("title={:?} dur={:?}", state.media_title, state.duration);
    mpv.shutdown().await;
    if !played {
        anyhow::bail!("local file did not play");
    }
    println!("PASS");
    Ok(())
}