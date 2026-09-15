pub mod app;
pub mod view;

use std::io;
use std::time::Duration;

use anyhow::Result;
use crossterm::event::{Event as CrosstermEvent, EventStream};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use futures::StreamExt;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use tokio::sync::mpsc;

use crate::config::Config;
use crate::mpv::{Mpv, MpvEvent};

/// Restores the terminal even on panic or early return.
struct RestoreGuard;

impl Drop for RestoreGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen);
    }
}

pub async fn run(
    cfg: &Config,
    mpv: Mpv,
    mut mpv_events: mpsc::UnboundedReceiver<MpvEvent>,
) -> Result<()> {
    enable_raw_mode()?;
    execute!(io::stdout(), EnterAlternateScreen)?;
    let _guard = RestoreGuard;

    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
    terminal.hide_cursor()?;

    let mut input = EventStream::new();
    let mut tick = tokio::time::interval(Duration::from_millis(250));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    let (app_tx, mut app_rx) = mpsc::unbounded_channel();
    let mut app_state = app::App::new(cfg.clone(), mpv, app_tx);
    let mut error = None;

    loop {
        {
            let snapshot = {
                let playback = app_state.mpv.state().read().await.clone();
                app_state.snapshot_state(playback)
            };
            terminal.draw(|f| view::draw(f, &snapshot, app_state.mode))?;

            if app_state.mode == app::Mode::Search || app_state.mode == app::Mode::Prompt {
                terminal.show_cursor()?;
            } else {
                terminal.hide_cursor()?;
            }
        }

        tokio::select! {
            ev = input.next() => {
                match ev {
                    Some(Ok(CrosstermEvent::Key(key))) => {
                        if let Err(e) = app_state.handle_key(key).await {
                            error = Some(e);
                            break;
                        }
                    }
                    Some(Ok(CrosstermEvent::Resize(_, _))) => {}
                    Some(Ok(_)) => {}
                    Some(Err(e)) => {
                        error = Some(e.into());
                        break;
                    }
                    None => {
                        error = Some(anyhow::anyhow!("input stream closed"));
                        break;
                    }
                }
            }
            ev = app_rx.recv() => {
                if let Some(ev) = ev {
                    app_state.handle_event(ev);
                }
            }
            ev = mpv_events.recv() => {
                match ev {
                    Some(MpvEvent::FileLoaded) | Some(MpvEvent::EndFile { .. }) => {
                        if let Some(ev) = ev {
                            let tx = app_state.events.clone();
                            let _ = tx.send(app::AppEvent::Mpv(ev));
                        }
                    }
                    Some(MpvEvent::StateChanged) => {}
                    None => break,
                }
            }
            _ = tick.tick() => {
                if app_state.is_toast_stale() {
                    app_state.toast = None;
                }
            }
        }
    }

    app_state.mpv.shutdown().await;
    terminal.show_cursor()?;
    match error {
        Some(e) => {
            let msg = e.to_string();
            if msg == "quit" || msg == "interrupted" {
                Ok(())
            } else {
                Err(e)
            }
        }
        None => Ok(()),
    }
}
