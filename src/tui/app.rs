use std::collections::VecDeque;
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use tokio::sync::mpsc;

use crate::art::Art;
use crate::config::Config;
use crate::innertube;
use crate::innertube::StreamFormat;
use crate::library;
use crate::model::{Track, TrackSource};
use crate::mpv::{Mpv, MpvEvent};
use crate::playlists;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Search,
    NowPlaying,
    Queue,
    Help,
    Prompt,
    Playlists,
    PlaylistDetail,
    Local,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptKind {
    SaveQueue,
    LoadYtPlaylist,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepeatMode {
    Off,
    All,
    One,
}

#[derive(Debug, Clone)]
#[allow(clippy::large_enum_variant)]
pub enum AppEvent {
    SearchResults {
        seq: u64,
        result: Result<Vec<Track>, String>,
    },
    StreamResolved {
        track: Track,
        result: Result<StreamFormat, String>,
    },
    YtPlaylistLoaded {
        result: Result<(String, Vec<Track>), String>,
    },
    Art {
        track_video_id: String,
        art: Option<Art>,
    },
    Mpv(MpvEvent),
}

/// True when `key` is `Alt+<c>`.
///
/// Rule everywhere: bare letters type, `Alt+letter` acts.
fn alt(key: &KeyEvent, c: char) -> bool {
    key.modifiers.contains(KeyModifiers::ALT) && key.code == KeyCode::Char(c)
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
    pub current_codec: Option<String>,
    pub shuffle: bool,
    pub repeat: RepeatMode,
    pub toast: Option<(String, Instant)>,
    pub mpv: Mpv,
    pub events: mpsc::UnboundedSender<AppEvent>,
    search_seq: u64,

    pub liked: Vec<Track>,
    pub playlist_names: Vec<String>,
    pub playlist_cursor: usize,
    pub active_playlist: Option<String>,
    pub playlist_tracks: Vec<Track>,
    pub local_tracks: Vec<Track>,
    pub local_cursor: usize,
    pub prompt: String,
    pub prompt_kind: PromptKind,
    retry_pending: bool,
}

impl App {
    pub fn new(cfg: Config, mpv: Mpv, events: mpsc::UnboundedSender<AppEvent>) -> Self {
        let liked = playlists::load_liked().unwrap_or_default();
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
            current_codec: None,
            shuffle: false,
            repeat: RepeatMode::Off,
            toast: None,
            mpv,
            events,
            search_seq: 0,
            liked,
            playlist_names: playlists::list_playlists().unwrap_or_default(),
            playlist_cursor: 0,
            active_playlist: None,
            playlist_tracks: Vec::new(),
            local_tracks: Vec::new(),
            local_cursor: 0,
            prompt: String::new(),
            prompt_kind: PromptKind::SaveQueue,
            retry_pending: false,
        }
    }

    pub fn show_toast(&mut self, message: impl Into<String>) {
        self.toast = Some((message.into(), Instant::now()));
    }

    pub fn is_liked(&self, track: &Track) -> bool {
        self.liked.iter().any(|t| t.key() == track.key())
    }

    pub fn toggle_like(&mut self, track: &Track) {
        if let Some(pos) = self.liked.iter().position(|t| t.key() == track.key()) {
            self.liked.remove(pos);
            self.show_toast("unliked");
        } else {
            self.liked.push(track.clone());
            self.show_toast("liked ♥");
        }
        let _ = playlists::save_liked(&self.liked);
    }

    /// Like the track most relevant to the current context.
    pub fn like_context(&mut self) {
        match self.mode {
            Mode::Search => {
                if let Some(t) = self.results.get(self.selected).cloned() {
                    self.toggle_like(&t);
                }
            }
            Mode::Queue => {
                if let Some(t) = self.queue.get(self.cursor).cloned() {
                    self.toggle_like(&t);
                }
            }
            Mode::PlaylistDetail => {
                if let Some(t) = self.playlist_tracks.get(self.playlist_cursor).cloned() {
                    self.toggle_like(&t);
                }
            }
            Mode::Local => {
                if let Some(t) = self.local_tracks.get(self.local_cursor).cloned() {
                    self.toggle_like(&t);
                }
            }
            Mode::NowPlaying => {
                if let Some(t) = self.current.clone() {
                    self.toggle_like(&t);
                }
            }
            _ => {}
        }
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
                        self.current_codec = Some(stream.display());
                        let title = format!("{} - {}", track.artist, track.title);
                        let url = stream.url.clone();
                        let mpv = self.mpv.clone();
                        tokio::spawn(async move {
                            let _ = mpv.load(&url, &title, false).await;
                        });
                        self.fetch_art_async(&track);
                    }
                    Err(e) => {
                        self.retry_pending = false;
                        self.show_toast(format!("playback failed: {e}"));
                    }
                }
            }
            AppEvent::YtPlaylistLoaded { result } => {
                match result {
                    Ok((title, tracks)) => {
                        if tracks.is_empty() {
                            self.show_toast("playlist is empty");
                            return;
                        }
                        self.queue.clear();
                        self.queue.extend(tracks);
                        self.cursor = 0;
                        self.mode = Mode::NowPlaying;
                        self.play_current();
                        self.show_toast(format!("{title} — {} tracks", self.queue.len()));
                    }
                    Err(e) => self.show_toast(format!("playlist failed: {e}")),
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
                match reason.as_str() {
                    "eof" | "end-of-file" => {
                        self.retry_pending = false;
                        self.on_track_ended();
                    }
                    "error" => {
                        // A stream died mid-song. Re-resolve once with a fresh
                        // config; if the retry also fails, move to the next.
                        if !self.retry_pending {
                            self.retry_pending = true;
                            self.play_current();
                        } else {
                            self.retry_pending = false;
                            self.next();
                        }
                    }
                    _ => {}
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
        self.current_codec = None;

        // Local files play directly through mpv — no stream resolution.
        if let TrackSource::LocalFile(path) = &track.source {
            let title = format!("{} - {}", track.artist, track.title);
            let mpv = self.mpv.clone();
            let path = path.clone();
            tokio::spawn(async move {
                let _ = mpv.load(&path.to_string_lossy(), &title, false).await;
            });
            return;
        }

        let tx = self.events.clone();
        let id = track.video_id.clone();
        let codec = self.cfg.codec;
        tokio::spawn(async move {
            let mut cfg = innertube::config::scrape().await;
            let client = innertube::http_client();
            let result = innertube::resolve_stream(&client, &mut cfg, &id, codec).await;
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

    /// Toggle the active EQ preset on/off. Requires mpv's `af` filter.
    pub async fn toggle_eq(&mut self) -> Result<()> {
        let active = self.cfg.eq.preset.is_some();
        if active {
            self.cfg.eq.preset = None;
            self.mpv.set_af("").await?;
            self.show_toast("EQ: clean");
        } else if let Some(chain) = self.cfg.active_eq_chain() {
            let preset_name = self.cfg.eq.preset.clone().unwrap_or_default();
            self.mpv.set_af(&chain).await?;
            self.show_toast(format!("EQ: {preset_name}"));
        } else {
            self.show_toast("no EQ presets configured");
        }
        if let Err(e) = self.cfg.save() {
            self.show_toast(format!("config write failed: {e}"));
        }
        Ok(())
    }

    /// Force the EQ back to clean/unfiltered.
    pub async fn clear_eq(&mut self) -> Result<()> {
        self.cfg.eq.preset = None;
        self.mpv.set_af("").await?;
        self.show_toast("EQ: clean");
        if let Err(e) = self.cfg.save() {
            self.show_toast(format!("config write failed: {e}"));
        }
        Ok(())
    }

    /// Rotate which preset is active (currently the first configured one).
    pub fn eq_state(&self) -> String {
        match &self.cfg.eq.preset {
            Some(name) => format!("EQ: {name}"),
            None => "EQ: off".to_string(),
        }
    }

    pub fn enter_playlists(&mut self) {
        self.playlist_names = playlists::list_playlists().unwrap_or_default();
        self.playlist_cursor = 0;
        self.mode = Mode::Playlists;
    }

    pub fn open_playlist(&mut self, name: Option<&str>) {
        match name {
            Some(name) => {
                let Ok(tracks) = playlists::load_playlist(name) else {
                    self.show_toast("could not load playlist");
                    return;
                };
                self.active_playlist = Some(name.to_string());
                self.playlist_tracks = tracks;
            }
            None => {
                self.active_playlist = None; // liked
                self.playlist_tracks = self.liked.clone();
            }
        }
        self.playlist_cursor = 0;
        self.mode = Mode::PlaylistDetail;
    }

    pub fn play_playlist_item(&mut self, idx: usize) {
        if idx >= self.playlist_tracks.len() {
            return;
        }
        self.queue.clear();
        for t in &self.playlist_tracks[idx..] {
            self.queue.push_back(t.clone());
        }
        self.cursor = 0;
        self.mode = Mode::NowPlaying;
        self.play_current();
    }

    pub fn remove_from_playlist(&mut self, idx: usize) {
        if idx >= self.playlist_tracks.len() {
            return;
        }
        self.playlist_tracks.remove(idx);
        if let Some(name) = &self.active_playlist {
            let _ = playlists::save_playlist(name, &self.playlist_tracks);
        } else {
            self.liked = self.playlist_tracks.clone();
            let _ = playlists::save_liked(&self.liked);
        }
        self.playlist_cursor = self.playlist_cursor.min(self.playlist_tracks.len().saturating_sub(1));
    }

    pub fn delete_active_playlist(&mut self) {
        let Some(name) = self.active_playlist.clone() else {
            return;
        };
        let _ = playlists::delete_playlist(&name);
        self.mode = Mode::Playlists;
        self.enter_playlists();
        self.show_toast(format!("deleted {name}"));
    }

    pub fn refresh_local(&mut self) {
        self.local_tracks = library::scan_local_tracks(&self.cfg.local_dirs);
        self.local_cursor = 0;
    }

    pub fn play_local_item(&mut self, idx: usize) {
        if idx >= self.local_tracks.len() {
            return;
        }
        self.queue.clear();
        for t in &self.local_tracks[idx..] {
            self.queue.push_back(t.clone());
        }
        self.cursor = 0;
        self.mode = Mode::NowPlaying;
        self.play_current();
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
            Mode::Prompt => self.handle_prompt_key(key).await?,
            Mode::Playlists => self.handle_playlists_key(key)?,
            Mode::PlaylistDetail => self.handle_playlist_detail_key(key)?,
            Mode::Local => self.handle_local_key(key)?,
        }
        Ok(())
    }

    async fn handle_search_key(&mut self, key: KeyEvent) -> Result<()> {
        if alt(&key, 'l') {
            self.like_context();
            return Ok(());
        }
        if alt(&key, 'o') || alt(&key, 't') {
            self.mode = Mode::Queue;
            return Ok(());
        }
        if alt(&key, 'L') {
            self.open_playlist(None);
            return Ok(());
        }
        if alt(&key, 'P') {
            self.enter_playlists();
            return Ok(());
        }
        if alt(&key, 'y') {
            self.prompt = String::new();
            self.prompt_kind = PromptKind::LoadYtPlaylist;
            self.mode = Mode::Prompt;
            return Ok(());
        }
        if alt(&key, 'S') {
            self.prompt = String::new();
            self.prompt_kind = PromptKind::SaveQueue;
            self.mode = Mode::Prompt;
            return Ok(());
        }
        if alt(&key, 'u') {
            self.refresh_local();
            self.mode = Mode::Local;
            return Ok(());
        }
        if alt(&key, 'e') {
            self.toggle_eq().await?;
            return Ok(());
        }
        if alt(&key, 'x') {
            self.clear_eq().await?;
            return Ok(());
        }
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
            KeyCode::Char(c)
                if !c.is_control() && !key.modifiers.contains(KeyModifiers::ALT) =>
            {
                self.query.push(c);
                self.start_search_seq(self.query.clone());
            }
            _ => {}
        }
        Ok(())
    }

    async fn handle_now_playing_key(&mut self, key: KeyEvent) -> Result<()> {
        if alt(&key, 'l') {
            self.like_context();
        }
        if alt(&key, 'L') {
            self.open_playlist(None);
        }
        if alt(&key, 'P') {
            self.enter_playlists();
        }
        if alt(&key, 'o') || alt(&key, 't') {
            self.mode = Mode::Queue;
        }
        if alt(&key, 'y') {
            self.prompt = String::new();
            self.prompt_kind = PromptKind::LoadYtPlaylist;
            self.mode = Mode::Prompt;
        }
        if alt(&key, 'S') {
            self.prompt = String::new();
            self.prompt_kind = PromptKind::SaveQueue;
            self.mode = Mode::Prompt;
        }
        if alt(&key, 'u') {
            self.refresh_local();
            self.mode = Mode::Local;
        }
        if alt(&key, 'e') {
            self.toggle_eq().await?;
        }
        if alt(&key, 'x') {
            self.clear_eq().await?;
        }
        if alt(&key, 'n') {
            self.next();
        }
        if alt(&key, 'p') {
            self.prev();
        }
        if alt(&key, 'r') {
            self.cycle_repeat();
        }
        if alt(&key, 'z') {
            self.toggle_shuffle();
        }
        match key.code {
            KeyCode::Char('q') => anyhow::bail!("quit"),
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                anyhow::bail!("quit")
            }
            KeyCode::Char('?') => self.mode = Mode::Help,
            KeyCode::Char('s') | KeyCode::Char('/') => self.mode = Mode::Search,
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
            _ => {}
        }
        Ok(())
    }

    fn handle_queue_key(&mut self, key: KeyEvent) -> Result<()> {
        if alt(&key, 'l') {
            self.like_context();
        }
        if alt(&key, 'L') {
            self.open_playlist(None);
        }
        if alt(&key, 'P') {
            self.enter_playlists();
        }
        if alt(&key, 'o') || alt(&key, 't') {
            self.mode = Mode::NowPlaying;
        }
        if alt(&key, 'z') {
            self.toggle_shuffle();
        }
        if alt(&key, 'r') {
            self.cycle_repeat();
        }
        if alt(&key, 'd') {
            self.remove_queue_item(self.cursor);
        }
        match key.code {
            KeyCode::Esc => self.mode = Mode::NowPlaying,
            KeyCode::Up | KeyCode::Char('k') => {
                self.cursor = self.cursor.saturating_sub(1);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.cursor = (self.cursor + 1).min(self.queue.len().saturating_sub(1));
            }
            KeyCode::Enter => self.play_queue_item(self.cursor),
            KeyCode::Delete => self.remove_queue_item(self.cursor),
            _ => {}
        }
        Ok(())
    }

    async fn handle_prompt_key(&mut self, key: KeyEvent) -> Result<()> {
        match key.code {
            KeyCode::Esc => self.mode = Mode::NowPlaying,
            KeyCode::Enter => {
                let value = self.prompt.trim().to_string();
                self.mode = Mode::NowPlaying;
                if value.is_empty() {
                    self.show_toast("nothing entered");
                    return Ok(());
                }
                match self.prompt_kind {
                    PromptKind::SaveQueue => {
                        if self.queue.is_empty() {
                            self.show_toast("queue is empty");
                            return Ok(());
                        }
                        let tracks: Vec<Track> = self.queue.iter().cloned().collect();
                        if let Err(e) = playlists::save_playlist(&value, &tracks) {
                            self.show_toast(format!("save failed: {e}"));
                        } else {
                            self.show_toast(format!("saved playlist '{value}'"));
                        }
                    }
                    PromptKind::LoadYtPlaylist => {
                        let tx = self.events.clone();
                        tokio::spawn(async move {
                            let cfg = innertube::config::scrape().await;
                            let client = innertube::http_client();
                            let result =
                                innertube::playlist::fetch_playlist(&client, &cfg, &value).await;
                            let result = result
                                .map(|p| (p.title, p.tracks))
                                .map_err(|e| e.to_string());
                            let _ = tx.send(AppEvent::YtPlaylistLoaded { result });
                        });
                    }
                }
            }
            KeyCode::Backspace => {
                self.prompt.pop();
            }
            KeyCode::Char(c)
                if !c.is_control() && !key.modifiers.contains(KeyModifiers::ALT) =>
            {
                self.prompt.push(c)
            }
            _ => {}
        }
        Ok(())
    }

    fn handle_playlists_key(&mut self, key: KeyEvent) -> Result<()> {
        if alt(&key, 'P') {
            self.mode = Mode::NowPlaying;
            return Ok(());
        }
        if alt(&key, 'L') {
            self.open_playlist(None);
            return Ok(());
        }
        match key.code {
            KeyCode::Esc => self.mode = Mode::NowPlaying,
            KeyCode::Up | KeyCode::Char('k') => {
                self.playlist_cursor = self.playlist_cursor.saturating_sub(1);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.playlist_cursor =
                    (self.playlist_cursor + 1).min(self.playlist_names.len().saturating_sub(1));
            }
            KeyCode::Enter => {
                if let Some(name) = self.playlist_names.get(self.playlist_cursor).cloned() {
                    self.open_playlist(Some(&name));
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn handle_playlist_detail_key(&mut self, key: KeyEvent) -> Result<()> {
        if alt(&key, 'l')
            && let Some(t) = self.playlist_tracks.get(self.playlist_cursor).cloned()
        {
            self.toggle_like(&t);
        }
        if alt(&key, 'x') && self.active_playlist.is_some() {
            self.delete_active_playlist();
        } else if alt(&key, 'd') {
            self.remove_from_playlist(self.playlist_cursor);
        }
        match key.code {
            KeyCode::Esc => self.mode = Mode::Playlists,
            KeyCode::Up | KeyCode::Char('k') => {
                self.playlist_cursor = self.playlist_cursor.saturating_sub(1);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.playlist_cursor =
                    (self.playlist_cursor + 1).min(self.playlist_tracks.len().saturating_sub(1));
            }
            KeyCode::Enter => self.play_playlist_item(self.playlist_cursor),
            KeyCode::Delete => self.remove_from_playlist(self.playlist_cursor),
            _ => {}
        }
        Ok(())
    }

    fn handle_local_key(&mut self, key: KeyEvent) -> Result<()> {
        if alt(&key, 'l')
            && let Some(t) = self.local_tracks.get(self.local_cursor).cloned()
        {
            self.toggle_like(&t);
        }
        if alt(&key, 'u') {
            self.mode = Mode::NowPlaying;
            return Ok(());
        }
        match key.code {
            KeyCode::Esc => self.mode = Mode::NowPlaying,
            KeyCode::Up | KeyCode::Char('k') => {
                self.local_cursor = self.local_cursor.saturating_sub(1);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.local_cursor =
                    (self.local_cursor + 1).min(self.local_tracks.len().saturating_sub(1));
            }
            KeyCode::Enter => self.play_local_item(self.local_cursor),
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
            current_codec: self.current_codec.clone(),
            shuffle: self.shuffle,
            repeat: self.repeat,
            playback,
            toast: self.toast.as_ref().map(|(m, _)| m.clone()),
            liked_keys: self.liked.iter().map(|t| t.key()).collect(),
            playlist_names: self.playlist_names.clone(),
            playlist_cursor: self.playlist_cursor,
            active_playlist: self.active_playlist.clone(),
            playlist_tracks: self.playlist_tracks.clone(),
            local_tracks: self.local_tracks.clone(),
            local_cursor: self.local_cursor,
            prompt: self.prompt.clone(),
            prompt_kind: self.prompt_kind,
            eq: self.eq_state(),
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
    pub current_codec: Option<String>,
    pub shuffle: bool,
    pub repeat: RepeatMode,
    pub playback: crate::mpv::PlaybackState,
    pub toast: Option<String>,
    pub liked_keys: Vec<String>,
    pub playlist_names: Vec<String>,
    pub playlist_cursor: usize,
    pub active_playlist: Option<String>,
    pub playlist_tracks: Vec<Track>,
    pub local_tracks: Vec<Track>,
    pub local_cursor: usize,
    pub prompt: String,
    pub prompt_kind: PromptKind,
    pub eq: String,
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

#[cfg(test)]
mod tests {
    use super::alt;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn k(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, mods)
    }

    #[test]
    fn alt_helper_requires_alt_modifier_and_matching_char() {
        assert!(alt(&k(KeyCode::Char('l'), KeyModifiers::ALT), 'l'));
        assert!(!alt(&k(KeyCode::Char('l'), KeyModifiers::empty()), 'l'));
        assert!(!alt(&k(KeyCode::Char('r'), KeyModifiers::ALT), 'l'));
        assert!(!alt(&k(KeyCode::Char('l'), KeyModifiers::CONTROL), 'l'));
        assert!(alt(&k(KeyCode::Char('P'), KeyModifiers::ALT), 'P'));
        assert!(alt(
            &k(KeyCode::Char('R'), KeyModifiers::ALT.union(KeyModifiers::SHIFT)),
            'R'
        ));
    }
}