//! MPRIS2 service + desktop notifications.
//!
//! The TUI owns the authoritative playback state. This module exposes it on the
//! session bus as `org.mpris.MediaPlayer2.lastwave` and forwards control methods
//! back to the app as [`Control`] values over a channel. A small background task
//! polls the shared [`MprisState`] and emits `PropertiesChanged` for anything
//! that differs, plus a desktop notification on track change.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{RwLock, mpsc};
use zbus::names::InterfaceName;
use zbus::zvariant::{ObjectPath, OwnedValue, Value};
use zbus::{fdo, interface, proxy};

pub const BUS_NAME: &str = "org.mpris.MediaPlayer2.lastwave";
pub const OBJECT_PATH: &str = "/org/mpris/MediaPlayer2";

const PLAYER_IFACE: &str = "org.mpris.MediaPlayer2.Player";
const PROPS_IFACE: &str = "org.freedesktop.DBus.Properties";
const NO_TRACK: &str = "/org/mpris/MediaPlayer2/TrackList/NoTrack";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PlaybackStatus {
    #[default]
    Stopped,
    Playing,
    Paused,
}

impl PlaybackStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Stopped => "Stopped",
            Self::Playing => "Playing",
            Self::Paused => "Paused",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LoopStatus {
    #[default]
    None,
    Track,
    Playlist,
}

impl LoopStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "None",
            Self::Track => "Track",
            Self::Playlist => "Playlist",
        }
    }
}

/// Snapshot of everything the MPRIS interfaces report.
#[derive(Debug, Clone, PartialEq)]
pub struct MprisState {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub art_url: Option<String>,
    pub length_us: Option<i64>,
    pub track_id: Option<String>,
    pub status: PlaybackStatus,
    pub loop_status: LoopStatus,
    pub shuffle: bool,
    /// Normalised to the MPRIS `0.0..=1.0` range.
    pub volume: f64,
    pub can_go_next: bool,
    pub can_go_previous: bool,
    pub can_play: bool,
    pub can_pause: bool,
    pub can_seek: bool,
}

impl Default for MprisState {
    fn default() -> Self {
        Self {
            title: None,
            artist: None,
            album: None,
            art_url: None,
            length_us: None,
            track_id: None,
            status: PlaybackStatus::Stopped,
            loop_status: LoopStatus::None,
            shuffle: false,
            volume: 0.0,
            can_go_next: false,
            can_go_previous: false,
            can_play: false,
            can_pause: false,
            can_seek: true,
        }
    }
}

impl MprisState {
    fn track_path(&self) -> String {
        match &self.track_id {
            Some(key) => {
                let sanitized: String = key
                    .chars()
                    .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
                    .collect();
                format!("/org/mpris/MediaPlayer2/Track/{sanitized}")
            }
            None => NO_TRACK.to_owned(),
        }
    }

    fn metadata_map(&self) -> HashMap<String, OwnedValue> {
        let owned = |v: Value<'static>| OwnedValue::try_from(v).unwrap_or_else(|_| {
            OwnedValue::try_from(Value::from(String::new())).expect("empty string is owned")
        });
        let mut map = HashMap::new();
        let track_path = self.track_path();
        if let Ok(path) = ObjectPath::try_from(track_path) {
            map.insert("mpris:trackid".to_owned(), owned(Value::from(path)));
        }
        if let Some(len) = self.length_us {
            map.insert("mpris:length".to_owned(), owned(Value::from(len)));
        }
        if let Some(title) = &self.title {
            map.insert(
                "xesam:title".to_owned(),
                owned(Value::from(title.clone())),
            );
        }
        if let Some(artist) = &self.artist {
            map.insert(
                "xesam:artist".to_owned(),
                owned(Value::from(vec![artist.clone()])),
            );
        }
        if let Some(album) = &self.album {
            map.insert(
                "xesam:album".to_owned(),
                owned(Value::from(album.clone())),
            );
        }
        if let Some(art) = &self.art_url {
            map.insert(
                "mpris:artUrl".to_owned(),
                owned(Value::from(art.clone())),
            );
        }
        map
    }

    fn metadata_changed(&self, other: &Self) -> bool {
        self.title != other.title
            || self.artist != other.artist
            || self.album != other.album
            || self.art_url != other.art_url
            || self.length_us != other.length_us
            || self.track_id != other.track_id
    }
}

/// A command requested by an external MPRIS client.
#[derive(Debug, Clone, Copy)]
pub enum Control {
    Next,
    Previous,
    PlayPause,
    Play,
    Pause,
    Stop,
    /// Relative seek in microseconds.
    Seek(i64),
    /// Absolute position in microseconds.
    SetPosition(i64),
    /// Normalised `0.0..=1.0` volume.
    SetVolume(f64),
    SetShuffle(bool),
    SetLoop(LoopStatus),
}

/// Shared handle the TUI writes state into.
#[derive(Clone)]
pub struct MprisHandle {
    state: Arc<RwLock<MprisState>>,
}

impl MprisHandle {
    /// Publish the latest state; the background task emits changes.
    pub async fn update(&self, state: MprisState) {
        let mut guard = self.state.write().await;
        if *guard != state {
            *guard = state;
        }
    }
}

/// Start the MPRIS service. Fails silently (returning a live handle) when there
/// is no session bus, so playback keeps working in headless environments.
pub fn spawn() -> (MprisHandle, mpsc::UnboundedReceiver<Control>) {
    let state = Arc::new(RwLock::new(MprisState::default()));
    let (ctrl_tx, ctrl_rx) = mpsc::unbounded_channel();
    let shared = state.clone();
    tokio::spawn(async move {
        if let Err(err) = serve(shared, ctrl_tx).await {
            let _ = err;
        }
    });
    (MprisHandle { state }, ctrl_rx)
}

async fn serve(
    state: Arc<RwLock<MprisState>>,
    ctrl: mpsc::UnboundedSender<Control>,
) -> anyhow::Result<()> {
    let connection = zbus::connection::Builder::session()?
        .name(BUS_NAME)?
        .serve_at(OBJECT_PATH, Root)?
        .serve_at(
            OBJECT_PATH,
            Player {
                state: state.clone(),
                ctrl,
            },
        )?
        .build()
        .await?;

    let notifications = NotificationsProxy::new(&connection).await.ok();

    let mut last = MprisState::default();
    let mut last_track: Option<String> = None;

    loop {
        tokio::time::sleep(Duration::from_millis(200)).await;
        let current = state.read().await.clone();

        if current != last {
            let _ = emit_changes(&connection, &last, &current).await;
        }

        if current.track_id != last_track {
            if let (Some(title), Some(proxy)) = (&current.title, &notifications) {
                let body = current.artist.clone().unwrap_or_default();
                let hints: HashMap<String, OwnedValue> = HashMap::new();
                let _ = proxy
                    .notify("lastwave", 0, "", title, &body, &[], hints, 5000)
                    .await;
            }
            last_track = current.track_id.clone();
        }

        last = current;
    }
}

async fn emit_changes(
    connection: &zbus::Connection,
    old: &MprisState,
    new: &MprisState,
) -> zbus::Result<()> {
    let mut changed: BTreeMap<&'static str, Value<'static>> = BTreeMap::new();
    let mut invalidated: Vec<&'static str> = Vec::new();

    if old.status != new.status {
        changed.insert("PlaybackStatus", Value::from(new.status.as_str()));
    }
    if old.loop_status != new.loop_status {
        changed.insert("LoopStatus", Value::from(new.loop_status.as_str()));
    }
    if old.shuffle != new.shuffle {
        changed.insert("Shuffle", Value::from(new.shuffle));
    }
    if (old.volume - new.volume).abs() > f64::EPSILON {
        changed.insert("Volume", Value::from(new.volume));
    }
    if old.can_go_next != new.can_go_next {
        changed.insert("CanGoNext", Value::from(new.can_go_next));
    }
    if old.can_go_previous != new.can_go_previous {
        changed.insert("CanGoPrevious", Value::from(new.can_go_previous));
    }
    if old.can_play != new.can_play {
        changed.insert("CanPlay", Value::from(new.can_play));
    }
    if old.can_pause != new.can_pause {
        changed.insert("CanPause", Value::from(new.can_pause));
    }
    if old.can_seek != new.can_seek {
        changed.insert("CanSeek", Value::from(new.can_seek));
    }
    if old.metadata_changed(new) {
        invalidated.push("Metadata");
    }

    if changed.is_empty() && invalidated.is_empty() {
        return Ok(());
    }

    let iface = InterfaceName::from_static_str_unchecked(PLAYER_IFACE);
    connection
        .emit_signal(
            None::<zbus::names::BusName<'static>>,
            OBJECT_PATH,
            PROPS_IFACE,
            "PropertiesChanged",
            &(iface, &changed, invalidated.as_slice()),
        )
        .await
}

struct Root;

#[interface(name = "org.mpris.MediaPlayer2")]
impl Root {
    async fn raise(&self) -> fdo::Result<()> {
        Ok(())
    }

    async fn quit(&self) -> fdo::Result<()> {
        Ok(())
    }

    #[zbus(property)]
    async fn can_quit(&self) -> bool {
        true
    }

    #[zbus(property)]
    async fn can_raise(&self) -> bool {
        false
    }

    #[zbus(property)]
    async fn has_track_list(&self) -> bool {
        false
    }

    #[zbus(property)]
    async fn fullscreen(&self) -> bool {
        false
    }

    #[zbus(property)]
    async fn can_set_fullscreen(&self) -> bool {
        false
    }

    #[zbus(property)]
    async fn identity(&self) -> String {
        "lastwave".to_owned()
    }

    #[zbus(property)]
    async fn desktop_entry(&self) -> String {
        "lastwave".to_owned()
    }

    #[zbus(property)]
    async fn supported_uri_schemes(&self) -> Vec<String> {
        Vec::new()
    }

    #[zbus(property)]
    async fn supported_mime_types(&self) -> Vec<String> {
        Vec::new()
    }
}

struct Player {
    state: Arc<RwLock<MprisState>>,
    ctrl: mpsc::UnboundedSender<Control>,
}

impl Player {
    fn send(&self, control: Control) -> fdo::Result<()> {
        self.ctrl
            .send(control)
            .map_err(|e| fdo::Error::Failed(e.to_string()))
    }
}

#[interface(name = "org.mpris.MediaPlayer2.Player")]
impl Player {
    async fn next(&self) -> fdo::Result<()> {
        self.send(Control::Next)
    }

    async fn previous(&self) -> fdo::Result<()> {
        self.send(Control::Previous)
    }

    async fn play_pause(&self) -> fdo::Result<()> {
        self.send(Control::PlayPause)
    }

    async fn play(&self) -> fdo::Result<()> {
        self.send(Control::Play)
    }

    async fn pause(&self) -> fdo::Result<()> {
        self.send(Control::Pause)
    }

    async fn stop(&self) -> fdo::Result<()> {
        self.send(Control::Stop)
    }

    async fn seek(&self, offset: i64) -> fdo::Result<()> {
        self.send(Control::Seek(offset))
    }

    #[zbus(name = "SetPosition")]
    async fn set_position(&self, track_id: ObjectPath<'_>, position: i64) -> fdo::Result<()> {
        if track_id.as_str() != self.state.read().await.track_path() {
            return Err(fdo::Error::InvalidArgs("unknown track id".to_owned()));
        }
        self.send(Control::SetPosition(position))
    }

    #[zbus(name = "OpenUri")]
    async fn open_uri(&self, uri: String) -> fdo::Result<()> {
        let _ = uri;
        Ok(())
    }

    #[zbus(property)]
    async fn playback_status(&self) -> String {
        self.state.read().await.status.as_str().to_owned()
    }

    #[zbus(property)]
    async fn loop_status(&self) -> String {
        self.state.read().await.loop_status.as_str().to_owned()
    }

    #[zbus(property)]
    async fn set_loop_status(&self, status: String) -> fdo::Result<()> {
        let status = match status.as_str() {
            "Track" => LoopStatus::Track,
            "Playlist" => LoopStatus::Playlist,
            _ => LoopStatus::None,
        };
        self.send(Control::SetLoop(status))
    }

    #[zbus(property)]
    async fn shuffle(&self) -> bool {
        self.state.read().await.shuffle
    }

    #[zbus(property)]
    async fn set_shuffle(&self, shuffle: bool) -> fdo::Result<()> {
        self.send(Control::SetShuffle(shuffle))
    }

    #[zbus(property)]
    async fn metadata(&self) -> HashMap<String, OwnedValue> {
        self.state.read().await.metadata_map()
    }

    #[zbus(property)]
    async fn volume(&self) -> f64 {
        self.state.read().await.volume
    }

    #[zbus(property)]
    async fn set_volume(&self, volume: f64) -> fdo::Result<()> {
        self.send(Control::SetVolume(volume.clamp(0.0, 1.0)))
    }

    #[zbus(property)]
    async fn position(&self) -> i64 {
        // Position is polled by clients; the TUI owns the live value.
        let _ = self.state.read().await;
        -1
    }

    #[zbus(property)]
    async fn rate(&self) -> f64 {
        1.0
    }

    #[zbus(property)]
    async fn minimum_rate(&self) -> f64 {
        0.01
    }

    #[zbus(property)]
    async fn maximum_rate(&self) -> f64 {
        100.0
    }

    #[zbus(property)]
    async fn can_go_next(&self) -> bool {
        self.state.read().await.can_go_next
    }

    #[zbus(property)]
    async fn can_go_previous(&self) -> bool {
        self.state.read().await.can_go_previous
    }

    #[zbus(property)]
    async fn can_play(&self) -> bool {
        self.state.read().await.can_play
    }

    #[zbus(property)]
    async fn can_pause(&self) -> bool {
        self.state.read().await.can_pause
    }

    #[zbus(property)]
    async fn can_seek(&self) -> bool {
        self.state.read().await.can_seek
    }

    #[zbus(property)]
    async fn can_control(&self) -> bool {
        true
    }
}

#[proxy(
    interface = "org.freedesktop.Notifications",
    default_service = "org.freedesktop.Notifications",
    default_path = "/org/freedesktop/Notifications"
)]
trait Notifications {
    #[allow(clippy::too_many_arguments)]
    fn notify(
        &self,
        app_name: &str,
        replaces_id: u32,
        app_icon: &str,
        summary: &str,
        body: &str,
        actions: &[&str],
        hints: HashMap<String, OwnedValue>,
        expire_timeout: i32,
    ) -> zbus::Result<u32>;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> MprisState {
        MprisState {
            title: Some("Song".to_owned()),
            artist: Some("Artist".to_owned()),
            track_id: Some("abc123".to_owned()),
            ..MprisState::default()
        }
    }

    #[test]
    fn track_path_is_sanitized() {
        let mut s = base();
        s.track_id = Some("browse:MPREb_x/1".to_owned());
        assert_eq!(
            s.track_path(),
            "/org/mpris/MediaPlayer2/Track/browse_MPREb_x_1"
        );
    }

    #[test]
    fn track_path_falls_back_to_notrack() {
        assert_eq!(MprisState::default().track_path(), NO_TRACK);
    }

    #[test]
    fn metadata_contains_expected_keys() {
        let s = base();
        let map = s.metadata_map();
        assert!(map.contains_key("mpris:trackid"));
        assert!(map.contains_key("xesam:title"));
        assert!(map.contains_key("xesam:artist"));
        assert!(!map.contains_key("mpris:length"));
    }

    #[test]
    fn metadata_change_detected() {
        let a = base();
        let mut b = a.clone();
        b.title = Some("Other".to_owned());
        assert!(a.metadata_changed(&b));
        assert!(!a.metadata_changed(&a));
    }

    #[test]
    fn playback_status_strings_match_spec() {
        assert_eq!(PlaybackStatus::Playing.as_str(), "Playing");
        assert_eq!(PlaybackStatus::Paused.as_str(), "Paused");
        assert_eq!(PlaybackStatus::Stopped.as_str(), "Stopped");
        assert_eq!(LoopStatus::Playlist.as_str(), "Playlist");
    }
}
