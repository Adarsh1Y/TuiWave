use anyhow::Result;
use clap::Parser;
use lastwave::{config, mpv, tui};

#[derive(Parser, Debug)]
#[command(
    name = "lastwave",
    version,
    about = "LastWave-inspired YouTube Music TUI player for Linux"
)]
struct Args {
    /// Start volume (0-150), overrides config.
    #[arg(long, short = 'v')]
    volume: Option<u8>,

    /// Path to the mpv binary.
    #[arg(long)]
    mpv_path: Option<String>,
}

fn main() -> Result<()> {
    let args = Args::parse();

    let mut cfg = config::Config::load().unwrap_or_default();
    if let Some(v) = args.volume {
        cfg.volume = v.min(150);
    }
    if let Some(p) = args.mpv_path {
        cfg.mpv_path = p;
    }
    let cfg = cfg;

    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async move {
        let (mpv, mpv_events) = mpv::Mpv::spawn(&cfg).await?;
        tui::run(&cfg, mpv, mpv_events).await
    })
}
