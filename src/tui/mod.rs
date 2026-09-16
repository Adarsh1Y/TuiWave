pub mod app;
pub mod view;

use std::io;
use std::io::Write;
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

/// True when running under the kitty terminal (graphics protocol available).
fn is_kitty() -> bool {
    std::env::var("KITTY_WINDOW_ID").is_ok() || std::env::var("TERM").as_deref() == Ok("xterm-kitty")
}

/// Cell size in pixels from the terminal window, if the terminal reports it.
fn cell_size() -> Option<(f64, f64)> {
    let ws = crossterm::terminal::window_size().ok()?;
    if ws.width == 0 || ws.height == 0 || ws.columns == 0 || ws.rows == 0 {
        return None;
    }
    Some((
        ws.width as f64 / ws.columns as f64,
        ws.height as f64 / ws.rows as f64,
    ))
}

/// Raises kitty's chunk-size limit so artwork transmits in a single packet.
fn kitty_init() {
    let mut out = io::stdout();
    let _ = out.write_all(b"\x1b_Gc=p,max_chunk=1048576,q=2\x1b\\");
    let _ = out.flush();
}

/// Transmit (once) and place the current album art via the kitty protocol.
fn kitty_place(out: &mut impl Write, blit: &view::ArtBlit, cell_w: f64, cell_h: f64,
    last_key: &mut String) -> io::Result<()> {
    if last_key != &blit.key
        && let Some(img) = crate::art::kitty_payload(&blit.key)
    {
        write!(
            out,
            "\x1b_Gf=24,s={},{};{}\x1b\\",
            img.width, img.height, img.hex
        )?;
        last_key.clone_from(&blit.key);
    }
    let x = (blit.cell.x as f64 * cell_w).round() as u32;
    let y = (blit.cell.y as f64 * cell_h).round() as u32;
    let w = (blit.cell.width as f64 * cell_w).round().max(1.0) as u32;
    let h = (blit.cell.height as f64 * cell_h).round().max(1.0) as u32;
    write!(out, "\x1b_Ga=p,C=1,q=2,x={x},y={y},w={w},h={h}\x1b\\")?;
    out.flush()?;
    Ok(())
}

pub async fn run(
    cfg: &Config,
    backend: Backend,
    mut backend_events: mpsc::UnboundedReceiver<Event>,
    resume: bool,
    play: Option<String>,
) -> Result<()> {
    enable_raw_mode()?;
    execute!(io::stdout(), EnterAlternateScreen)?;
    let _guard = RestoreGuard;

    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
    terminal.hide_cursor()?;

    let (kitty, cell_w, cell_h) = match cell_size() {
        Some((cell_w, cell_h)) if is_kitty() => (true, cell_w, cell_h),
        _ => (false, 0.0, 0.0),
    };
    if kitty {
        kitty_init();
    }
    let mut art_key = String::new();
    let mut out = io::stdout();

    let mut input = EventStream::new();
    let mut tick = tokio::time::interval(Duration::from_millis(250));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    let (app_tx, mut app_rx) = mpsc::unbounded_channel();
    let (mpris, mut mpris_rx) = crate::mpris::spawn();
    let mut app_state = app::App::new(
        cfg.clone(),
        backend,
        app_tx,
        Some(mpris.clone()),
        kitty,
        cell_w,
        cell_h,
    );
    let mut error = None;

    // The MPRIS sender only closes when the service failed to start (no session
    // bus). Once closed, stop polling the channel instead of busy-spinning.
    let mut mpris_closed = false;

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

    if let Some(arg) = play {
        app_state.play_cli_arg(&arg);
    }

    let mut tick_count: u64 = 0;

    loop {
        {
            let playback = app_state.backend.state().read().await.clone();
            mpris.update(app_state.mpris_state(&playback)).await;
            let snapshot = app_state.snapshot_state(playback);
            let mut blit: Option<view::ArtBlit> = None;
            terminal.draw(|f| {
                blit = view::draw(f, &snapshot, app_state.mode);
            })?;
            if let Some(blit) = blit.as_ref()
                && kitty
            {
                kitty_place(&mut out, blit, cell_w, cell_h, &mut art_key)?;
            }

            if app_state.mode == app::Mode::Search || app_state.mode == app::Mode::Prompt {
                terminal.show_cursor()?;
            } else {
                terminal.hide_cursor()?;
            }
        }

        let ctrl_fut: std::pin::Pin<
            Box<dyn std::future::Future<Output = Option<crate::mpris::Control>> + '_>,
        > = if mpris_closed {
            Box::pin(std::future::pending())
        } else {
            Box::pin(mpris_rx.recv())
        };

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
            ctrl = ctrl_fut => {
                match ctrl {
                    Some(ctrl) => {
                        // External control must never take the app down; surface
                        // failures as a toast and keep playing.
                        if let Err(e) = app_state.handle_mpris(ctrl).await {
                            app_state.show_toast(format!("mpris: {e:#}"));
                        }
                        if app_state.exit_requested {
                            error = Some(anyhow::anyhow!("quit"));
                            break;
                        }
                    }
                    None => mpris_closed = true,
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
                let playback = app_state.backend.state().read().await.clone();
                if tick_count.is_multiple_of(40) {
                    app_state.persist_session(&playback);
                }
                // Detect stalled streams so the URL can be re-resolved and the
                // stream reloaded instead of hanging forever.
                app_state.check_stall(&playback);
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
