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

use crate::backend::{Backend, Event};
use crate::config::Config;

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
    backend: Backend,
    mut backend_events: mpsc::UnboundedReceiver<Event>,
    resume: bool,
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
    let mut app_state = app::App::new(cfg.clone(), backend, app_tx);
    let mut error = None;

    if resume
        && let Some(session) = crate::state::Session::load()
    {
        if !session.queue.is_empty() {
            app_state.apply_session(&session);
            if session.volume > 0.0 {
                let _ = app_state.backend.set_volume(session.volume).await;
            }
            app_state.mode = app::Mode::NowPlaying;
            app_state.play_current();
            if !app_state.query.is_empty() {
                app_state.start_search_seq(app_state.query.clone());
            }
        } else if !session.history.is_empty()
            || session.repeat != app::RepeatMode::Off
            || session.shuffle
        {
            app_state.apply_session(&session);
        }
    }

    let mut tick_count: u64 = 0;

    loop {
        {
            let snapshot = {
                let playback = app_state.backend.state().read().await.clone();
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
            ev = backend_events.recv() => {
                match ev {
                    Some(Event::FileLoaded) | Some(Event::EndFile { .. }) => {
                        if let Some(ev) = ev {
                            let tx = app_state.events.clone();
                            let _ = tx.send(app::AppEvent::Playback(ev));
                        }
                    }
                    Some(Event::StateChanged) => {}
                    None => break,
                }
            }
            _ = tick.tick() => {
                if app_state.is_toast_stale() {
                    app_state.toast = None;
                }
                tick_count += 1;
                if tick_count.is_multiple_of(40) {
                    let playback = app_state.backend.state().read().await.clone();
                    app_state.persist_session(&playback);
                }
            }
        }
    }

    {
        let playback = app_state.backend.state().read().await.clone();
        app_state.persist_session(&playback);
    }
    app_state.backend.shutdown().await;
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
