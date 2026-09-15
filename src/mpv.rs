use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::Duration;

use anyhow::Context;
use futures::TryFutureExt;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::process::Command;
use tokio::sync::{Mutex, RwLock, mpsc};

use crate::config::Config;

const COMMAND_TIMEOUT: Duration = Duration::from_secs(10);

/// Live playback properties mirrored from mpv, cheap to snapshot on a tick.
#[derive(Debug, Default, Clone)]
pub struct PlaybackState {
    pub time_pos: Option<f64>,
    pub duration: Option<f64>,
    pub paused: bool,
    pub idle: bool,
    pub volume: f64,
    pub media_title: Option<String>,
}

#[derive(Debug, Clone)]
pub enum MpvEvent {
    FileLoaded,
    EndFile { reason: String },
    StateChanged,
}

/// Control handle for an mpv subprocess speaking JSON-RPC over a unix socket.
#[derive(Clone)]
pub struct Mpv {
    write: Arc<Mutex<OwnedWriteHalf>>,
    next_id: Arc<AtomicI64>,
    pending: Arc<Mutex<HashMap<i64, tokio::sync::oneshot::Sender<Value>>>>,
    state: Arc<RwLock<PlaybackState>>,
}

impl Mpv {
    pub async fn spawn(cfg: &Config) -> anyhow::Result<(Self, mpsc::UnboundedReceiver<MpvEvent>)> {
        let socket = runtime_socket()?;
        let _ = std::fs::remove_file(&socket);

        let child = Command::new(&cfg.mpv_path)
            .arg("--no-video")
            .arg("--no-terminal")
            .arg("--idle=yes")
            .arg("--keep-open=no")
            .arg("--osc=no")
            .arg("--audio-display=no")
            .arg(format!("--input-ipc-server={}", socket.display()))
            .arg(format!("--volume={}", cfg.volume))
            .arg("--volume-max=150")
            .arg("--cache=yes")
            .arg("--demuxer-max-bytes=256MiB")
            .arg("--msg-level=ipc=error,cplayer=error")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .context("failed to spawn mpv")?;

        drop(child);

        let stream = tokio::time::timeout(
            Duration::from_secs(15),
            connect_retry(&socket).map_err(|e| e.to_string()),
        )
        .await
        .map_err(|_| anyhow::anyhow!("mpv did not bring up the ipc socket in time"))?
        .map_err(|e| anyhow::anyhow!("failed to connect to mpv ipc socket: {e}"))?;
        let (read, write) = stream.into_split();

        let (event_tx, event_rx) = mpsc::unbounded_channel();
        let state = Arc::new(RwLock::new(PlaybackState {
            volume: cfg.volume as f64,
            ..Default::default()
        }));

        let handle = Self {
            write: Arc::new(Mutex::new(write)),
            next_id: Arc::new(AtomicI64::new(1)),
            pending: Arc::new(Mutex::new(HashMap::new())),
            state: Arc::clone(&state),
        };

        tokio::spawn(reader_loop(
            read,
            handle.pending.clone(),
            handle.state.clone(),
            event_tx,
        ));

        for (id, prop) in [
            (1, "time-pos"),
            (2, "duration"),
            (3, "pause"),
            (4, "idle"),
            (5, "volume"),
            (6, "media-title"),
        ] {
            let _ = handle
                .command_raw(json!(["observe_property", id, prop]))
                .await;
        }

        Ok((handle, event_rx))
    }

    pub fn state(&self) -> Arc<RwLock<PlaybackState>> {
        Arc::clone(&self.state)
    }

    pub async fn load(&self, url: &str, title: &str, paused: bool) -> anyhow::Result<()> {
        if !title.is_empty() {
            self.command_raw(json!(["set_property", "force-media-title", title]))
                .await?;
        }
        self.command_raw(json!(["loadfile", url, "replace"]))
            .await?;
        if paused {
            self.command_raw(json!(["set_property", "pause", true]))
                .await?;
        }
        Ok(())
    }

    pub async fn play_pause(&self) -> anyhow::Result<()> {
        self.command_raw(json!(["cycle", "pause"])).await?;
        Ok(())
    }

    pub async fn seek(&self, seconds: f64, absolute: bool) -> anyhow::Result<()> {
        let mode = if absolute {
            "absolute-second"
        } else {
            "relative"
        };
        self.command_raw(json!(["seek", seconds, mode])).await?;
        Ok(())
    }

    pub async fn set_volume(&self, volume: f64) -> anyhow::Result<()> {
        self.command_raw(json!(["set_property", "volume", volume.clamp(0.0, 150.0)]))
            .await?;
        Ok(())
    }

    /// Apply an audio filter chain to mpv (`af` property). An empty chain
    /// resets to clean/unfiltered playback.
    pub async fn set_af(&self, chain: &str) -> anyhow::Result<()> {
        self.command_raw(json!(["set_property", "af", chain])).await?;
        Ok(())
    }

    /// Read the currently active audio filter chain.
    pub async fn get_af(&self) -> anyhow::Result<String> {
        fn render(item: &Value) -> Option<String> {
            match item {
                Value::String(s) => Some(s.clone()),
                Value::Object(map) => {
                    let name = map.get("name").and_then(Value::as_str)?;
                    let mut out = name.to_string();
                    if let Some(params) = map.get("params").and_then(Value::as_object) {
                        for (k, v) in params {
                            out.push_str(&format!(",{}={}", k, v.as_str()?));
                        }
                    }
                    Some(out)
                }
                _ => None,
            }
        }
        let v = self.command_raw(json!(["get_property", "af"])).await?;
        Ok(match v {
            Value::String(s) => s,
            Value::Array(items) => items
                .iter()
                .filter_map(render)
                .collect::<Vec<_>>()
                .join(","),
            other => other.to_string(),
        })
    }

    /// Cycle to the next entry in `audio-device-list`; returns the new
    /// device's description (None when there's nowhere to cycle).
    pub async fn next_audio_output(&self) -> anyhow::Result<Option<String>> {
        let list = self
            .command_raw(json!(["get_property", "audio-device-list"]))
            .await?;
        let current = self
            .command_raw(json!(["get_property", "audio-device"]))
            .await?;
        let current = current.as_str().unwrap_or("auto").to_string();
        let items: Vec<Value> = list.as_array().cloned().unwrap_or_default();
        let mut names = Vec::new();
        for it in &items {
            if let Some(n) = it.get("name").and_then(Value::as_str) {
                names.push(n.to_string());
            }
        }
        if names.is_empty() {
            return Ok(None);
        }
        let idx = names.iter().position(|n| *n == current);
        let next_idx = match idx {
            Some(i) => (i + 1) % names.len(),
            None => 0,
        };
        if names[next_idx] == current {
            return Ok(None);
        }
        self.command_raw(json!(["set_property", "audio-device", names[next_idx]]))
            .await?;
        let desc = items[next_idx]
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or(&names[next_idx]);
        Ok(Some(desc.to_string()))
    }

    pub async fn shutdown(&self) {
        let _ = self.command_raw(json!(["quit", 0])).await;
    }

    async fn command_raw(&self, command: Value) -> anyhow::Result<Value> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.pending.lock().await.insert(id, tx);

        let msg = json!({ "command": command, "request_id": id });
        let mut line = serde_json::to_string(&msg)?;
        line.push('\n');

        let write = self.write.clone();
        let sent = tokio::time::timeout(COMMAND_TIMEOUT, async move {
            let mut w = write.lock().await;
            w.write_all(line.as_bytes()).await?;
            w.flush().await?;
            Ok::<(), anyhow::Error>(())
        })
        .await;

        match sent {
            Err(_) => {
                self.pending.lock().await.remove(&id);
                anyhow::bail!("send to mpv timed out")
            }
            Ok(Err(e)) => {
                self.pending.lock().await.remove(&id);
                return Err(e).context("send to mpv failed");
            }
            Ok(Ok(())) => {}
        }

        let value = tokio::time::timeout(COMMAND_TIMEOUT, rx)
            .await
            .map_err(|_| anyhow::anyhow!("mpv did not answer within {COMMAND_TIMEOUT:?}"))??;
        let error = value
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        if error != "success" {
            anyhow::bail!("mpv error: {error}");
        }
        Ok(value.get("data").cloned().unwrap_or(Value::Null))
    }
}

async fn connect_retry(socket: &PathBuf) -> anyhow::Result<UnixStream> {
    loop {
        match UnixStream::connect(socket).await {
            Ok(s) => return Ok(s),
            // The socket file may briefly exist while mpv is still accepting.
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
                ) =>
            {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            Err(e) => return Err(e.into()),
        }
    }
}

async fn reader_loop(
    mut read: OwnedReadHalf,
    pending: Arc<Mutex<HashMap<i64, tokio::sync::oneshot::Sender<Value>>>>,
    state: Arc<RwLock<PlaybackState>>,
    event_tx: mpsc::UnboundedSender<MpvEvent>,
) {
    let mut reader = BufReader::new(&mut read);
    let mut line = String::new();
    loop {
        line.clear();
        let n = match reader.read_line(&mut line).await {
            Ok(n) => n,
            Err(_) => break,
        };
        if n == 0 {
            break;
        }
        let Ok(msg) = serde_json::from_str::<Value>(&line) else {
            continue;
        };

        if let Some(id) = msg.get("request_id").and_then(Value::as_i64) {
            if let Some(tx) = pending.lock().await.remove(&id) {
                let _ = tx.send(msg);
            }
            continue;
        }

        let Some(event) = msg.get("event").and_then(Value::as_str) else {
            continue;
        };
        match event {
            "property-change" => {
                let name = msg.get("name").and_then(Value::as_str).unwrap_or("");
                let data = msg.get("data");
                let mut s = state.write().await;
                match name {
                    "time-pos" => s.time_pos = data.and_then(Value::as_f64).filter(|v| *v >= 0.0),
                    "duration" => s.duration = data.and_then(Value::as_f64),
                    "pause" => s.paused = data.and_then(Value::as_bool).unwrap_or(false),
                    "idle" => s.idle = data.and_then(Value::as_bool).unwrap_or(true),
                    "volume" => s.volume = data.and_then(Value::as_f64).unwrap_or(0.0),
                    "media-title" => {
                        s.media_title = data.and_then(Value::as_str).map(str::to_string)
                    }
                    _ => {}
                }
                let _ = event_tx.send(MpvEvent::StateChanged);
            }
            "file-loaded" => {
                let _ = event_tx.send(MpvEvent::FileLoaded);
            }
            "end-file" => {
                let reason = msg
                    .get("reason")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown")
                    .to_string();
                let _ = event_tx.send(MpvEvent::EndFile { reason });
            }
            _ => {}
        }
    }
}

fn runtime_socket() -> anyhow::Result<PathBuf> {
    let dir =
        dirs::runtime_dir().ok_or_else(|| anyhow::anyhow!("cannot resolve XDG_RUNTIME_DIR"))?;
    std::fs::create_dir_all(&dir)?;
    Ok(dir.join(format!("lastwave-{}.sock", std::process::id())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn socket_path_is_namespaced_by_pid() {
        let p = runtime_socket().unwrap();
        assert!(
            p.file_name()
                .unwrap()
                .to_string_lossy()
                .contains("lastwave-")
        );
    }

    #[test]
    fn state_defaults() {
        let s = PlaybackState::default();
        assert!(s.time_pos.is_none());
        assert!(!s.paused);
        assert!(!s.idle);
        assert_eq!(s.volume, 0.0);
    }
}
