use std::collections::VecDeque;
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use tokio::sync::mpsc;

use crate::art::Art;
use crate::config::Config;
use crate::innertube;
use crate::model::Track;
use crate::mpv::{Mpv, MpvEvent};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Search,
    NowPlaying,
    Queue,
    Help,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepeatMode {
    Off,
    All,
    One,
}

#[derive(Debug, Clone)]
pub enum AppEvent {
    SearchResults {
        seq: u64,
        result: Result<Vec<Track>, String>,
    },
    StreamResolved {
        track: Track,
        result: Result<innertube::StreamFormat, String>,
    },
    Art {
        track_video_id: String,
        art: Option<Art>,
    },
    Mpv(MpvEvent),
}

pub struct App {
    pub cfg: Config,
    pub mode: Mode,
    pub query: String,
    pub results: Vec<Track>,
    pub selected: usize,
    pub queue: VecDeque<Track>,
    pub cursor: usize,
    pub current: Option<Track>,
    pub current_art: Option<Art>,
    pub shuffle: bool,
    pub repeat: RepeatMode,
    pub toast: Option<(String, Instant)>,
    pub mpv: Mpv,
    pub events: mpsc::UnboundedSender<AppEvent>,
    search_seq: u64,
}

impl App {
    pub fn new(cfg: Config, mpv: Mpv, events: mpsc::UnboundedSender<AppEvent>) -> Self {
        Self {
            cfg,
            mode: Mode::Search,
            query: String::new(),
            results: Vec::new(),
            selected: 0,
            queue: VecDeque::new(),
            cursor: 0,
            current: None,
            current_art: None,
            shuffle: false,
            repeat: RepeatMode::Off,
            toast: None,
            mpv,
            events,
            search_seq: 0,
        }
    }

    pub fn show_toast(&mut self, message: impl Into<String>) {
        self.toast = Some((message.into(), Instant::now()));
    }

    pub fn handle_event(&mut self, event: AppEvent) {
        match event {
            AppEvent::SearchResults { seq, result } => {
                if seq != self.search_seq {
                    return;
                }
                match result {
                    Ok(tracks) => {
                        if !tracks.is_empty() {
                            self.results = tracks;
                            self.selected = 0;
                        } else {
                            self.results.clear();
                            self.show_toast("no results");
                        }
                    }
                    Err(e) => self.show_toast(format!("search failed: {e}")),
                }
            }
            AppEvent::StreamResolved { track, result } => {
                if self.current.as_ref().map(|t| &t.video_id) != Some(&track.video_id) {
                    return;
                }
                match result {
                    Ok(stream) => {
                        let title = format!("{} - {}", track.artist, track.title);
                        let url = stream.url.clone();
                        let mpv = self.mpv.clone();
                        tokio::spawn(async move {
                            let _ = mpv.load(&url, &title, false).await;
                        });
                        self.fetch_art_async(&track);
                    }
                    Err(e) => {
                        self.show_toast(format!("playback failed: {e}"));
                    }
                }
            }
            AppEvent::Art {
                track_video_id,
                art,
            } => {
                if self
                    .current
                    .as_ref()
                    .map(|t| &t.video_id)
                    .is_some_and(|id| id == &track_video_id)
                {
                    self.current_art = art;
                }
            }
            AppEvent::Mpv(MpvEvent::EndFile { reason }) => {
                if reason == "eof" || reason == "end-of-file" {
                    self.on_track_ended();
                }
            }
            AppEvent::Mpv(MpvEvent::FileLoaded) | AppEvent::Mpv(MpvEvent::StateChanged) => {}
        }
    }

    pub fn start_search_seq(&mut self, query: String) {
        self.search_seq = self.search_seq.wrapping_add(1);
        let seq = self.search_seq;
        let q = query.clone();
        let tx = self.events.clone();
        self.results.clear();
        self.selected = 0;
        tokio::spawn(async move {
            let result = search_inner(&q).await;
            let _ = tx.send(AppEvent::SearchResults {
                seq,
                result: result.map_err(|e| e.to_string()),
            });
        });
    }

    pub fn play_selection(&mut self) {
        if self.results.is_empty() {
            return;
        }
        let idx = self.selected.min(self.results.len().saturating_sub(1));
        self.queue.clear();
        for t in &self.results[idx..] {
            self.queue.push_back(t.clone());
        }
        self.cursor = 0;
        self.mode = Mode::NowPlaying;
        self.play_current();
    }

    pub fn play_queue_item(&mut self, idx: usize) {
        if idx >= self.queue.len() {
            return;
        }
        self.cursor = idx;
        self.mode = Mode::NowPlaying;
        self.play_current();
    }

    pub fn play_current(&mut self) {
        let Some(track) = self.queue.get(self.cursor).cloned() else {
            return;
        };
        self.current = Some(track.clone());
        self.current_art = None;
        let tx = self.events.clone();
        let id = track.video_id.clone();
        tokio::spawn(async move {
            let mut cfg = innertube::config::scrape().await;
            let client = innertube::http_client();
            let result = innertube::resolve_stream(&client, &mut cfg, &id).await;
            let _ = tx.send(AppEvent::StreamResolved {
                track,
                result: result.map_err(|e| e.to_string()),
            });
        });
    }

    pub fn next(&mut self) {
        if self.queue.is_empty() {
            return;
        }
        if self.shuffle && self.cursor + 1 < self.queue.len() {
            let remaining = self.cursor + 1;
            let n = self.queue.len() - remaining;
            self.cursor = remaining + (fast_rng() as usize % n);
        } else if self.cursor + 1 < self.queue.len() {
            self.cursor += 1;
        } else if self.repeat == RepeatMode::All {
            self.cursor = 0;
        } else {
            return;
        }
        self.play_current();
    }

    pub fn prev(&mut self) {
        if self.queue.is_empty() {
            return;
        }
        if self.cursor > 0 {
            self.cursor -= 1;
        } else if self.repeat == RepeatMode::All {
            self.cursor = self.queue.len().saturating_sub(1);
        } else {
            return;
        }
        self.play_current();
    }

    pub fn remove_queue_item(&mut self, idx: usize) {
        if idx >= self.queue.len() {
            return;
        }
        let removed = self.queue.remove(idx);
        let Some(removed) = removed else {
            return;
        };
        if self.current.as_ref().map(|t| &t.video_id) == Some(&removed.video_id) {
            if self.cursor >= idx {
                self.cursor = self.cursor.saturating_sub(1);
            }
        } else if idx < self.cursor {
            self.cursor -= 1;
        }
    }

    pub fn toggle_shuffle(&mut self) {
        self.shuffle = !self.shuffle;
        let state = if self.shuffle {
            "shuffle on"
        } else {
            "shuffle off"
        };
        self.show_toast(state);
    }

    pub fn cycle_repeat(&mut self) {
        self.repeat = match self.repeat {
            RepeatMode::Off => RepeatMode::All,
            RepeatMode::All => RepeatMode::One,
            RepeatMode::One => RepeatMode::Off,
        };
        let label = match self.repeat {
            RepeatMode::Off => "repeat off",
            RepeatMode::All => "repeat all",
            RepeatMode::One => "repeat one",
        };
        self.show_toast(label);
    }

    fn on_track_ended(&mut self) {
        if self.repeat == RepeatMode::One {
            self.play_current();
            return;
        }
        self.next();
    }

    fn fetch_art_async(&mut self, track: &Track) {
        let cfg = self.cfg.clone();
        let tx = self.events.clone();
        let video_id = track.video_id.clone();
        let track = track.clone();
        tokio::spawn(async move {
            let art = Art::load(&cfg, &track, 22, 11).await.ok().flatten();
            let _ = tx.send(AppEvent::Art {
                track_video_id: video_id,
                art,
            });
        });
    }

    pub async fn handle_key(&mut self, key: KeyEvent) -> Result<()> {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            anyhow::bail!("interrupted");
        }
        self.toast = None;
        match self.mode {
            Mode::Help => {
                self.mode = Mode::NowPlaying;
                return Ok(());
            }
            Mode::Search => self.handle_search_key(key).await?,
            Mode::NowPlaying => self.handle_now_playing_key(key).await?,
            Mode::Queue => self.handle_queue_key(key)?,
        }
        Ok(())
    }

    async fn handle_search_key(&mut self, key: KeyEvent) -> Result<()> {
        match key.code {
            KeyCode::Esc => self.mode = Mode::NowPlaying,
            KeyCode::Backspace => {
                self.query.pop();
                self.start_search_seq(self.query.clone());
            }
            KeyCode::Up => self.selected = self.selected.saturating_sub(1),
            KeyCode::Down => {
                self.selected = (self.selected + 1).min(self.results.len().saturating_sub(1));
            }
            KeyCode::Enter => self.play_selection(),
            KeyCode::Char('?') => self.mode = Mode::Help,
            KeyCode::Char(c) if !c.is_control() => {
                self.query.push(c);
                self.start_search_seq(self.query.clone());
            }
            _ => {}
        }
        Ok(())
    }

    async fn handle_now_playing_key(&mut self, key: KeyEvent) -> Result<()> {
        match key.code {
            KeyCode::Char('q') => anyhow::bail!("quit"),
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                anyhow::bail!("quit")
            }
            KeyCode::Char('?') => self.mode = Mode::Help,
            KeyCode::Char('s') | KeyCode::Char('/') => self.mode = Mode::Search,
            KeyCode::Char('o') => self.mode = Mode::Queue,
            KeyCode::Char(' ') => self.mpv.play_pause().await?,
            KeyCode::Left => {
                if key.modifiers.contains(KeyModifiers::SHIFT) {
                    self.mpv.seek(-60.0, false).await?;
                } else {
                    self.mpv.seek(-10.0, false).await?;
                }
            }
            KeyCode::Right => {
                if key.modifiers.contains(KeyModifiers::SHIFT) {
                    self.mpv.seek(60.0, false).await?;
                } else {
                    self.mpv.seek(10.0, false).await?;
                }
            }
            KeyCode::Char(',') => {
                let st = self.mpv.state();
                let s = st.read().await;
                let vol = s.volume - 5.0;
                self.mpv.set_volume(vol).await?;
            }
            KeyCode::Char('.') => {
                let st = self.mpv.state();
                let s = st.read().await;
                let vol = s.volume + 5.0;
                self.mpv.set_volume(vol).await?;
            }
            KeyCode::Char('n') => self.next(),
            KeyCode::Char('p') => self.prev(),
            KeyCode::Char('r') => self.cycle_repeat(),
            KeyCode::Char('z') => self.toggle_shuffle(),
            KeyCode::Char('t') => self.mode = Mode::Queue,
            KeyCode::Char('k') => self.prev(),
            KeyCode::Char('l') => self.next(),
            _ => {}
        }
        Ok(())
    }

    fn handle_queue_key(&mut self, key: KeyEvent) -> Result<()> {
        match key.code {
            KeyCode::Esc | KeyCode::Char('o') | KeyCode::Char('t') => self.mode = Mode::NowPlaying,
            KeyCode::Up | KeyCode::Char('k') => {
                self.cursor = self.cursor.saturating_sub(1);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.cursor = (self.cursor + 1).min(self.queue.len().saturating_sub(1));
            }
            KeyCode::Enter => self.play_queue_item(self.cursor),
            KeyCode::Char('d') | KeyCode::Delete => self.remove_queue_item(self.cursor),
            KeyCode::Char('z') => self.toggle_shuffle(),
            KeyCode::Char('r') => self.cycle_repeat(),
            _ => {}
        }
        Ok(())
    }

    pub fn is_toast_stale(&self) -> bool {
        self.toast
            .as_ref()
            .map(|(_, at)| at.elapsed() > Duration::from_secs(4))
            .unwrap_or(true)
    }

    pub fn snapshot_state(&self, playback: crate::mpv::PlaybackState) -> ViewData {
        ViewData {
            query: self.query.clone(),
            results: self.results.clone(),
            selected: self.selected,
            queue: self.queue.iter().cloned().collect(),
            cursor: self.cursor,
            current: self.current.clone(),
            art: self.current_art.clone(),
            shuffle: self.shuffle,
            repeat: self.repeat,
            playback,
            toast: self.toast.as_ref().map(|(m, _)| m.clone()),
        }
    }
}

pub struct ViewData {
    pub query: String,
    pub results: Vec<Track>,
    pub selected: usize,
    pub queue: Vec<Track>,
    pub cursor: usize,
    pub current: Option<Track>,
    pub art: Option<Art>,
    pub shuffle: bool,
    pub repeat: RepeatMode,
    pub playback: crate::mpv::PlaybackState,
    pub toast: Option<String>,
}

async fn search_inner(query: &str) -> Result<Vec<Track>> {
    let cfg = innertube::config::scrape().await;
    innertube::search::search_songs(&cfg, query).await
}

/// Tiny deterministic-ish rng for shuffle (no external dep).
fn fast_rng() -> u32 {
    use std::hash::{BuildHasher, Hasher};
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    std::collections::hash_map::RandomState::new()
        .build_hasher()
        .finish() as u32
        ^ now as u32
}

impl std::fmt::Debug for App {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("App").field("mode", &self.mode).finish()
    }
}
