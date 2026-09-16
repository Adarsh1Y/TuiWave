use anyhow::Result;
use clap::Parser;
use lastwave::{backend, config, state, tui};

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

    /// Restore the previous session (queue, position, history) on start.
    #[arg(long)]
    resume: bool,

    /// Start fresh, ignoring any saved session.
    #[arg(long, conflicts_with = "resume")]
    no_resume: bool,

    /// Start and play a local file or search query immediately.
    #[arg(long)]
    play: Option<String>,
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
    let resume = if args.play.is_some() || args.no_resume {
        false
    } else if args.resume {
        true
    } else {
        cfg.resume
    };
    let cfg = cfg;

    let _lock = state::acquire_single_instance()?;

    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async move {
        let (backend_engine, backend_events) = backend::Backend::spawn(&cfg).await?;
        if backend_engine.supports_eq()
            && let Some(chain) = cfg.active_eq_chain().filter(|c| !c.is_empty())
        {
            let _ = backend_engine.set_af(&chain).await;
        }
        tui::run(&cfg, backend_engine, backend_events, resume, args.play).await
    })
}
